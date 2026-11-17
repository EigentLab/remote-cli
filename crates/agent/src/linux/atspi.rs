//! Targetable elements from the AT-SPI accessibility tree, over the session's accessibility D-Bus.

use std::time::Duration;

use zbus::{blocking::Connection, zvariant::OwnedObjectPath};

use crate::platform::{DisplayInfo, UiAction, UiElement};

const ACCESSIBLE: &str = "org.a11y.atspi.Accessible";
const ACTION: &str = "org.a11y.atspi.Action";
/// Bounds one walk: a big browser page has thousands of nodes, each a few D-Bus calls.
const MAX_NODES: usize = 3000;
/// shortcut: a field's value is cut at this many characters so a big document stays cheap; raise it if Controllers need more.
const MAX_VALUE_CHARS: i32 = 10_000;
/// AtspiStateType bits.
const CHECKED: u32 = 4;
const COLLAPSED: u32 = 5;
const EDITABLE: u32 = 7;
const ENABLED: u32 = 8;
const EXPANDABLE: u32 = 9;
const EXPANDED: u32 = 10;
const FOCUSED: u32 = 12;
const SELECTED: u32 = 23;
const SHOWING: u32 = 25;
const VISIBLE: u32 = 30;

/// `None` when there is no accessibility bus or nothing targetable is showing on `d`: the Agent then uses OCR.
pub fn elements(d: &DisplayInfo) -> anyhow::Result<Option<Vec<UiElement>>> {
    let Ok(bus) = connect() else { return Ok(None) };
    let found = walk(&bus, d);
    Ok(if found.is_empty() { None } else { Some(found.into_iter().map(|(e, _)| e).collect()) })
}

/// Performs `action` on the element of `d`'s tree that `target` names (see `Platform::ui_action`): a click does the
/// default action (action 0), else focuses a field; a double click does an "open" action; a right click a menu
/// action; `SetValue` goes through EditableText, else Value. `Ok(false)` when the element has no such action.
pub fn perform(d: &DisplayInfo, target: &UiElement, action: &UiAction) -> anyhow::Result<bool> {
    let bus = connect()?;
    let tree = walk(&bus, d);
    let (found, (dest, node)) = UiElement::find(&tree, target)
        .ok_or_else(|| anyhow::anyhow!("no {} {:?} on screen any more; observe again", target.role, target.text))?;
    let dest = Some(dest.as_str());
    let ok = |reply: zbus::Result<zbus::Message>| reply.and_then(|m| m.body().deserialize::<bool>()).unwrap_or(false);
    // Action names differ by toolkit ("click", "press", "ShowMenu"): compared lowercased, without spaces or underscores.
    let actions: Vec<String> = bus
        .call_method(dest, node, Some(ACTION), "GetActions", &())
        .and_then(|m| m.body().deserialize::<Vec<(String, String, String)>>())
        .unwrap_or_default()
        .into_iter()
        .map(|(name, _, _)| name.to_lowercase().replace([' ', '_'], ""))
        .collect();
    let do_action = |i: usize| ok(bus.call_method(dest, node, Some(ACTION), "DoAction", &(i as i32)));
    let named = |names: &[&str]| actions.iter().position(|a| names.contains(&a.as_str())).is_some_and(do_action);
    Ok(match action {
        // A GTK entry's default action is "activate", which is Enter: a field is focused instead.
        UiAction::Press if found.role == "field" => ok(bus.call_method(dest, node, Some("org.a11y.atspi.Component"), "GrabFocus", &())),
        UiAction::Press => !actions.is_empty() && do_action(0),
        UiAction::Open => named(&["open"]),
        UiAction::ShowMenu => named(&["showmenu", "menu", "popup", "showcontextmenu", "contextmenu"]),
        UiAction::SetValue(value) => {
            let interfaces: Vec<String> = bus.call_method(dest, node, Some(ACCESSIBLE), "GetInterfaces", &())?.body().deserialize()?;
            let has = |i: &str| interfaces.iter().any(|x| x == i);
            if has("org.a11y.atspi.EditableText") {
                let set = ok(bus.call_method(dest, node, Some("org.a11y.atspi.EditableText"), "SetTextContents", &(value.as_str())));
                anyhow::ensure!(set, "cannot set the value of {:?}", found.text);
                true
            } else if has("org.a11y.atspi.Value") {
                let v: f64 = value.trim().parse().map_err(|_| anyhow::anyhow!("{:?} takes a number, not {value:?}", found.text))?;
                let set = ("org.a11y.atspi.Value", "CurrentValue", zbus::zvariant::Value::from(v));
                bus.call_method(dest, node, Some("org.freedesktop.DBus.Properties"), "Set", &set)?;
                true
            } else {
                false
            }
        }
    })
}

/// The targetable elements of `d` with their bus name and object path.
fn walk(bus: &Connection, d: &DisplayInfo) -> Vec<(UiElement, (String, OwnedObjectPath))> {
    let mut found: Vec<(UiElement, (String, OwnedObjectPath))> = vec![];
    // Each node carries the index in `found` of the nearest container above it.
    let mut stack = vec![("org.a11y.atspi.Registry".to_owned(), path("/org/a11y/atspi/accessible/root"), 0, None)];
    let mut visited = 0;
    while let Some((dest, node, depth, mut parent)) = stack.pop() {
        visited += 1;
        if visited > MAX_NODES {
            break;
        }
        let call = |iface: &str, method: &str| bus.call_method(Some(dest.as_str()), &node, Some(iface), method, &());
        // The root and the applications carry no states; below them, hidden subtrees are skipped whole.
        if depth >= 2 {
            let Ok(states) = call(ACCESSIBLE, "GetState").and_then(|m| m.body().deserialize::<Vec<u32>>()) else { continue };
            if !(has(&states, SHOWING) && has(&states, VISIBLE)) {
                continue;
            }
            if let Some(mut element) = element(bus, &dest, &node, &states, d) {
                element.parent = parent;
                if element.is_container() {
                    parent = Some(found.len());
                }
                found.push((element, (dest.clone(), node.clone())));
            }
        }
        if let Ok(children) = call(ACCESSIBLE, "GetChildren").and_then(|m| m.body().deserialize::<Vec<(String, OwnedObjectPath)>>()) {
            stack.extend(children.into_iter().rev().map(|(dest, node)| (dest, node, depth + 1, parent)));
        }
    }
    found
}

/// The accessibility bus (separate from the session bus). Asking for it also tells toolkits that an assistive
/// technology is running, so Qt, Firefox and Chromium start exposing their trees (apps started before may not).
fn connect() -> anyhow::Result<Connection> {
    let session = Connection::session()?;
    let status = ("org.a11y.Status", "IsEnabled", zbus::zvariant::Value::from(true));
    let _ = session.call_method(Some("org.a11y.Bus"), "/org/a11y/bus", Some("org.freedesktop.DBus.Properties"), "Set", &status);
    let address: String = session.call_method(Some("org.a11y.Bus"), "/org/a11y/bus", Some("org.a11y.Bus"), "GetAddress", &())?.body().deserialize()?;
    // A hung application must not hang the Observation.
    Ok(zbus::blocking::connection::Builder::address(address.as_str())?.method_timeout(Duration::from_millis(500)).build()?)
}

/// A targetable node as an element of `d`, in its real pixels; `None` for other roles and off-display nodes.
fn element(bus: &Connection, dest: &str, node: &OwnedObjectPath, states: &[u32], d: &DisplayInfo) -> Option<UiElement> {
    let role: String = bus.call_method(Some(dest), node, Some(ACCESSIBLE), "GetRoleName", &()).ok()?.body().deserialize().ok()?;
    let password = role == "password text";
    let role = match role.as_str() {
        "push button" | "toggle button" => "button",
        "entry" | "password text" | "spin button" => "field",
        // GTK entries and editors are role "text" with the editable state.
        "text" if has(states, EDITABLE) => "field",
        "link" => "link",
        "check box" | "check menu item" => "checkbox",
        "radio button" | "radio menu item" => "radio",
        "menu item" => "menuitem",
        "combo box" => "combobox",
        "page tab" => "tab",
        "slider" => "slider",
        "label" | "heading" | "static" | "text" => "text",
        "frame" | "window" => "window",
        // GTK message boxes are "alert" and file dialogs "file chooser".
        "dialog" | "alert" | "file chooser" => "dialog",
        "menu" | "popup menu" => "menu",
        "list" | "list box" => "list",
        _ => return None,
    };
    let name = bus.call_method(Some(dest), node, Some("org.freedesktop.DBus.Properties"), "Get", &(ACCESSIBLE, "Name")).ok()?;
    let name: String = name.body().deserialize::<zbus::zvariant::OwnedValue>().ok()?.try_into().ok()?;
    if role == "text" && name.trim().is_empty() {
        return None;
    }
    // Coordinates relative to the screen (ATSPI_COORD_TYPE_SCREEN).
    let extents = bus.call_method(Some(dest), node, Some("org.a11y.atspi.Component"), "GetExtents", &(0u32)).ok()?;
    let (x, y, w, h): (i32, i32, i32, i32) = extents.body().deserialize().ok()?;
    // Clip to the display, in its pixels.
    let (x1, y1) = ((x - d.x).max(0), (y - d.y).max(0));
    let (x2, y2) = ((x + w - d.x).min(d.width as i32), (y + h - d.y).min(d.height as i32));
    if x2 <= x1 || y2 <= y1 {
        return None;
    }
    // A password field's content is never read.
    let value = if matches!(role, "field" | "slider") && !password { field_value(bus, dest, node) } else { None };
    Some(UiElement {
        role: role.into(),
        text: name.trim().into(),
        x: x1 as u32,
        y: y1 as u32,
        width: (x2 - x1) as u32,
        height: (y2 - y1) as u32,
        states: mark_states(states),
        value,
        password,
        parent: None,
    })
}

/// A field's or slider's content from its Text interface, else (none or empty) its numeric Value (spin buttons, sliders).
fn field_value(bus: &Connection, dest: &str, node: &OwnedObjectPath) -> Option<String> {
    if let Ok(text) = bus.call_method(Some(dest), node, Some("org.a11y.atspi.Text"), "GetText", &(0i32, MAX_VALUE_CHARS)) {
        if let Some(text) = text.body().deserialize::<String>().ok().filter(|t| !t.is_empty()) {
            return Some(text);
        }
    }
    let value = bus.call_method(Some(dest), node, Some("org.freedesktop.DBus.Properties"), "Get", &("org.a11y.atspi.Value", "CurrentValue")).ok()?;
    let value: f64 = value.body().deserialize::<zbus::zvariant::OwnedValue>().ok()?.try_into().ok()?;
    Some(value.to_string())
}

fn path(p: &str) -> OwnedObjectPath {
    OwnedObjectPath::try_from(p).expect("valid object path")
}

fn has(states: &[u32], bit: u32) -> bool {
    states.first().is_some_and(|s| s & (1 << bit) != 0)
}

/// The non-default Mark States in an AT-SPI state set (two 32-bit words; every bit used here is in the first).
fn mark_states(states: &[u32]) -> Vec<String> {
    let has = |bit| has(states, bit);
    let expanded = has(EXPANDED);
    [
        ("disabled", !has(ENABLED)),
        ("focused", has(FOCUSED)),
        ("checked", has(CHECKED)),
        ("selected", has(SELECTED)),
        ("expanded", expanded),
        ("collapsed", !expanded && (has(COLLAPSED) || has(EXPANDABLE))),
    ]
    .into_iter()
    .filter_map(|(name, on)| on.then(|| name.to_owned()))
    .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn state_bits_become_mark_states() {
        // ENABLED(8) | FOCUSED(12) | CHECKED(4): only focused and checked differ from the default.
        assert_eq!(mark_states(&[1 << 8 | 1 << 12 | 1 << 4, 0]), ["focused", "checked"]);
        // Not ENABLED, SELECTED(23), EXPANDED(10) on an EXPANDABLE(9) node.
        assert_eq!(mark_states(&[1 << 23 | 1 << 9 | 1 << 10, 0]), ["disabled", "selected", "expanded"]);
        // EXPANDABLE but not EXPANDED reads as collapsed.
        assert_eq!(mark_states(&[1 << 8 | 1 << 9, 0]), ["collapsed"]);
    }
}
