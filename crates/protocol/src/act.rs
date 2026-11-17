//! The JSON body of an Action request (`POST /api/v1/devices/{id}/actions`, and local control over the Agent IPC)
//! and the Observation JSON of its result, shared by the Server and the Agent.

use std::time::Duration;

use base64::Engine;
use serde::Deserialize;
use serde_json::{json, Value};

use crate::agent::{self as proto, action::Kind};

/// Upper bound of `max_frames`.
const MAX_FRAMES: u32 = 30;
/// Upper bound of all waits in one request (fixed waits and Wait Condition timeouts); the Agent request timeout grows by them.
const MAX_TOTAL_WAIT_MS: u64 = 30_000;
const DEFAULT_WAIT_TIMEOUT_MS: u32 = 5_000;

/// Why a request body was refused: not the request's shape (`bad_request`), or an invalid Action (`invalid_action`).
#[derive(Debug)]
pub enum Refused {
    BadRequest(String),
    Invalid(String),
}

/// A checked request body as the Agent takes it. `act.controller` is left for the caller to fill in.
pub struct Parsed {
    pub act: proto::ActRequest,
    /// `step_of[i]` = the API step of protocol Action `i` (one API Action can expand to several).
    pub step_of: Vec<u32>,
    /// All waits of the request: the Agent request timeout grows by them.
    pub waits: Duration,
    pub has_wait_condition: bool,
}

/// Checks `body` and turns it into the Agent's `ActRequest`.
pub fn parse(body: Value) -> Result<Parsed, Refused> {
    let body: ActBody = serde_json::from_value(body).map_err(|e| Refused::BadRequest(e.to_string()))?;
    let invalid = |m: String| Err(Refused::Invalid(m));
    let until_ms = |u: &Option<Until>| u.as_ref().map_or(0, |u| u64::from(u.timeout_ms()));
    let steps: u64 = body.actions.iter().map(|a| if let Action::Wait { ms, until } = a { u64::from(*ms) + until_ms(until) } else { 0 }).sum();
    let waits = steps + until_ms(&body.until);
    if waits > MAX_TOTAL_WAIT_MS {
        return invalid(format!("waits add up to {waits} ms; at most {MAX_TOTAL_WAIT_MS} ms per request"));
    }
    let max_frames = body.max_frames.unwrap_or(10);
    if !(1..=MAX_FRAMES).contains(&max_frames) {
        return invalid(format!("max_frames must be 1 to {MAX_FRAMES}"));
    }
    if body.until.is_some() && body.observe != Observe::Shot {
        return invalid("until needs observe \"shot\"".into());
    }
    let step_untils = body.actions.iter().filter_map(|a| if let Action::Wait { until, .. } = a { until.as_ref() } else { None });
    for until in body.until.iter().chain(step_untils) {
        until.check().map_err(Refused::Invalid)?;
    }
    let has_wait_condition = body.has_wait_condition();
    let (mut step_of, mut actions) = (vec![], vec![]);
    for (step, action) in body.actions.into_iter().enumerate() {
        for a in action.into_proto() {
            step_of.push(step as u32);
            actions.push(a);
        }
    }
    let act = proto::ActRequest {
        request_id: 0,
        actions,
        observe: match body.observe {
            Observe::Shot => proto::ObserveMode::Shot,
            Observe::None => proto::ObserveMode::None,
            Observe::Frames => proto::ObserveMode::Frames,
            Observe::Clip => proto::ObserveMode::Clip,
        }
        .into(),
        display: match body.display {
            Some(DisplaySel::Index(i)) => Some(i),
            _ => None,
        },
        all_displays: matches!(body.display, Some(DisplaySel::All(_))),
        controller: String::new(),
        max_frames,
        marks: body.marks,
        media_session: body.media_session,
        until: body.until.map(Into::into),
    };
    Ok(Parsed { act, step_of, waits: Duration::from_millis(waits), has_wait_condition })
}

impl ActBody {
    fn has_wait_condition(&self) -> bool {
        self.until.is_some() || self.actions.iter().any(|a| matches!(a, Action::Wait { until: Some(_), .. }))
    }
}

/// `{"actions": [...], "observe": "shot" | "none" | "frames" | "clip", "display": N | "all", "max_frames": N}`.
/// No Actions = observe only (no lock). Several Actions = an Action Batch, run in order.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ActBody {
    #[serde(default)]
    actions: Vec<Action>,
    #[serde(default)]
    observe: Observe,
    /// Unset = the display last acted on.
    display: Option<DisplaySel>,
    /// `frames`: at most this many per display (default 10).
    max_frames: Option<u32>,
    /// Detect Marks: annotated images plus `observation` id and `marks` table in the reply.
    #[serde(default)]
    marks: bool,
    /// The caller's open media channel (`POST .../media`): large images and clips go over it peer-to-peer.
    #[serde(default)]
    media_session: String,
    /// Wait Condition: wait (after the Actions) until it holds, then observe.
    until: Option<Until>,
}

/// `{"appears" | "disappears" | "value" | "state": {"text": "..", "role": "..", ..}, "timeout_ms": N}`.
#[derive(Deserialize)]
struct Until {
    #[serde(flatten)]
    condition: Condition,
    timeout_ms: Option<u32>,
    /// Keys left after the one condition: a second condition or an unknown key, refused by `check`.
    #[serde(flatten)]
    rest: serde_json::Map<String, Value>,
}

#[derive(Deserialize)]
#[serde(rename_all = "snake_case")]
enum Condition {
    Appears(WaitTarget),
    Disappears(WaitTarget),
    Value(ValueTarget),
    State(StateTarget),
}

/// The Marks a Wait Condition is about: text matched as for text targets, optionally narrowed by role.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct WaitTarget {
    text: String,
    role: Option<String>,
}

/// `{"text", "role"?, "equals" | "contains"}`; `Until::check` refuses anything but exactly one of the two.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ValueTarget {
    text: String,
    role: Option<String>,
    equals: Option<String>,
    contains: Option<String>,
}

/// `{"text", "role"?, "has" | "lacks": "<Mark State>"}`; `Until::check` refuses anything but exactly one of the two.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct StateTarget {
    text: String,
    role: Option<String>,
    has: Option<String>,
    lacks: Option<String>,
}

impl Until {
    fn timeout_ms(&self) -> u32 {
        self.timeout_ms.unwrap_or(DEFAULT_WAIT_TIMEOUT_MS)
    }

    /// `invalid_action` unless it names exactly one condition, and a value or state exactly one test.
    fn check(&self) -> Result<(), String> {
        if let Some(key) = self.rest.keys().next() {
            return Err(format!("a Wait Condition names one condition and timeout_ms, not also {key:?}"));
        }
        match &self.condition {
            Condition::Value(v) if v.equals.is_some() == v.contains.is_some() => {
                Err("a value Wait Condition needs exactly one of equals or contains".into())
            }
            Condition::State(s) if s.has.is_some() == s.lacks.is_some() => {
                Err("a state Wait Condition needs exactly one of has or lacks".into())
            }
            _ => Ok(()),
        }
    }
}

impl From<Until> for proto::WaitCondition {
    fn from(u: Until) -> Self {
        use proto::wait_condition::Condition as C;
        let timeout_ms = u.timeout_ms();
        let (text, role, condition) = match u.condition {
            Condition::Appears(t) => (t.text, t.role, C::Appears(true)),
            Condition::Disappears(t) => (t.text, t.role, C::Disappears(true)),
            Condition::Value(v) => (v.text, v.role, C::Value(proto::ValueCondition { equals: v.equals, contains: v.contains })),
            Condition::State(s) => (s.text, s.role, C::State(proto::StateCondition { has: s.has, lacks: s.lacks })),
        };
        Self { text, role, timeout_ms, condition: Some(condition) }
    }
}

#[derive(Deserialize, Default, PartialEq)]
#[serde(rename_all = "snake_case")]
enum Observe {
    #[default]
    Shot,
    None,
    Frames,
    Clip,
}

#[derive(Deserialize)]
#[serde(untagged)]
enum DisplaySel {
    Index(u32),
    All(AllDisplays),
}

#[derive(Deserialize)]
#[serde(rename_all = "snake_case")]
enum AllDisplays {
    All,
}

/// A target on `display` (default 0): a point in Observation image space (real pixels with `native`, as Live View
/// direct control sends), a Mark number from Observation `observation`, or a Mark's visible text.
#[derive(Deserialize)]
struct Point {
    #[serde(default)]
    display: u32,
    #[serde(default)]
    native: bool,
    #[serde(flatten)]
    target: Target,
}

#[derive(Deserialize)]
#[serde(untagged)]
enum Target {
    Xy { x: u32, y: u32 },
    Mark { mark: u32, observation: u64 },
    Text { text: String },
}

#[derive(Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum Action {
    Move(Point),
    Click(Point),
    DoubleClick(Point),
    RightClick(Point),
    Drag {
        #[serde(flatten)]
        from: Point,
        to: Point,
    },
    Scroll {
        #[serde(flatten)]
        at: Point,
        #[serde(default)]
        dx: i32,
        #[serde(default)]
        dy: i32,
    },
    /// Types `text`, first clicking `into` if given.
    Type {
        text: String,
        into: Option<Point>,
        // `"secret": true` (a password field) is read only by `mask_secrets`.
    },
    /// Sets a field's value through the accessibility tree, without keystrokes; `value` is never audited.
    SetValue {
        #[serde(flatten)]
        at: Point,
        value: String,
    },
    /// Key combination, e.g. "ctrl+shift+t".
    Key {
        keys: String,
    },
    /// Action Batch pause, in milliseconds, then (if given) until a Wait Condition holds.
    Wait {
        #[serde(default)]
        ms: u32,
        until: Option<Until>,
    },
}

impl From<Point> for proto::Point {
    fn from(p: Point) -> Self {
        let display = p.display;
        match p.target {
            Target::Xy { x, y } => Self { display, x, y, native: p.native, ..Default::default() },
            Target::Mark { mark, observation } => Self { display, mark: Some(mark), observation, ..Default::default() },
            Target::Text { text } => Self { display, text: Some(text), ..Default::default() },
        }
    }
}

impl Action {
    /// The protocol Actions this one becomes (typing into a target is a click, then the text; a wait with a
    /// Wait Condition is the fixed wait, then the Wait Condition).
    fn into_proto(self) -> Vec<proto::Action> {
        let kind = match self {
            Action::Type { text, into: Some(at) } => {
                return [Kind::Click(at.into()), Kind::TypeText(text)].map(|k| proto::Action { kind: Some(k) }).to_vec()
            }
            Action::Wait { ms, until: Some(u) } => {
                return [Kind::WaitMs(ms), Kind::WaitUntil(u.into())].map(|k| proto::Action { kind: Some(k) }).to_vec()
            }
            Action::Move(p) => Kind::Move(p.into()),
            Action::Click(p) => Kind::Click(p.into()),
            Action::DoubleClick(p) => Kind::DoubleClick(p.into()),
            Action::RightClick(p) => Kind::RightClick(p.into()),
            Action::Drag { from, to } => Kind::Drag(proto::Drag { from: Some(from.into()), to: Some(to.into()) }),
            Action::Scroll { at, dx, dy } => Kind::Scroll(proto::Scroll { at: Some(at.into()), dx, dy }),
            Action::Type { text, into: None } => Kind::TypeText(text),
            Action::SetValue { at, value } => Kind::SetValue(proto::SetValue { at: Some(at.into()), value }),
            Action::Key { keys } => Kind::Key(keys),
            Action::Wait { ms, until: None } => Kind::WaitMs(ms),
        };
        vec![proto::Action { kind: Some(kind) }]
    }
}

/// The Observation JSON of an Agent's ActResult.
pub fn observation(result: proto::ActResult) -> Value {
    let b64 = |data: &[u8]| base64::engine::general_purpose::STANDARD.encode(data);
    // Relayed, never stored (spec: no media in the audit log).
    // Bytes sent peer-to-peer are only referenced (`p2p`); the client puts them back into `data`.
    let p2p = |v: &mut Value, id: u64| {
        if id != 0 {
            v["data"] = Value::Null;
            v["p2p"] = id.into();
        }
    };
    let clip = result.clip.map(|c| {
        let mut v = json!({ "display": c.display, "width": c.width, "height": c.height, "mime": c.mime, "duration_ms": c.duration_ms, "data": b64(&c.data) });
        p2p(&mut v, c.p2p);
        v
    });
    let images: Vec<Value> = result
        .images
        .into_iter()
        .map(|i| {
            let data = (!i.unchanged).then(|| b64(&i.data));
            let mut v = json!({ "display": i.display, "width": i.width, "height": i.height, "unchanged": i.unchanged, "mime": i.mime, "data": data, "at_ms": i.at_ms });
            p2p(&mut v, i.p2p);
            v
        })
        .collect();
    let mut reply = json!({ "images": images });
    if result.observation != 0 {
        reply["observation"] = result.observation.into();
        reply["marks"] = result
            .marks
            .iter()
            .map(|m| {
                let mut v = json!({ "number": m.number, "text": m.text, "role": m.role, "display": m.display, "x": m.x, "y": m.y, "width": m.width, "height": m.height });
                if !m.states.is_empty() {
                    v["states"] = json!(m.states);
                }
                if !m.value.is_empty() {
                    v["value"] = json!(m.value);
                }
                if let Some(parent) = m.parent {
                    v["parent"] = json!(parent);
                }
                v
            })
            .collect();
    }
    if let Some(clip) = clip {
        reply["clip"] = clip;
    }
    reply
}
