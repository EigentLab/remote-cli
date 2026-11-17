---
title: Using rcli
description: Drive Devices from the rcli CLI, from an AI agent through MCP, and from the Dashboard.
sidebar:
  order: 1
---

rcli lets people, scripts and AI agents see and drive remote machines. There are three ways in, and all three call the same Controller API, so they have the same rights and share the same Control Lock:

- **`rcli`**, the CLI and the main tool. Every command takes `--json` for scripts and AI agents.
- **`rcli mcp`**, the same operations as MCP tools for Claude Code and other AI agents.
- **The Dashboard**, in a browser: your Address Book, Live View, a web terminal, power and Wake buttons, and the Server's administration pages. The Dashboard's text is in Vietnamese.

The words used here (Device, Action, Observation, Mark, Control Lock, Wake Relay…) are defined in [GLOSSARY.md](https://github.com/EigentLab/remote-cli/blob/main/GLOSSARY.md). To set up a Server, see the [Server guide](/en/server/). To install rcli on a machine, see [Install on a Device](/en/devices/).

## Try it on one machine

You do not need a Server to try rcli. Install it for your own user, then drive this machine as the **Local Device**:

```bash
rcli install                          # User Mode: no root, starts at login; without --server it serves only local control
rcli shot local --marks               # a screenshot of this machine, with numbered Marks
rcli act local click --text "OK"      # click the Mark labelled OK
```

`local` stands in for a Device ID and goes through the Agent running on this machine. It needs no Server, no Device Password and no Control Lock. Only the Device Owner (the OS user running the Agent) or root may use it. On macOS, first allow rcli Screen Recording and Accessibility (see [macOS](/en/devices/#macos)).

## Install rcli and connect to a Device

One install per machine makes it a Device and a Controller at once. The Dashboard's Connect Guide page (**Hướng dẫn kết nối**) shows the install commands with the Server Address and Server Key filled in. [Install on a Device](/en/devices/) covers each OS.

```bash
sudo rcli install --server rcli.example.com:9443 --key rk_xxxxxxxx_yyyyyyyy   # once per machine
rcli connect 123456789                                                        # asks for the Device Password once
```

`rcli install` installs the Agent, registers the Device and prints its Device ID and Device Password. It also writes the Controller configuration: the Server Address, the Server Key, the API Address and the Server's CA certificate. Run with `sudo` (Service Mode), the configuration is shared by every user of the machine (`/etc/rcli/controller.json`, on Windows `%ProgramData%\rcli\controller.json`). Every user can read it but not change it, so every user can run `rcli connect` and `rcli mcp` without installing again. Run without `sudo` (User Mode), it is yours alone.

The Server Address is the `host:port` of the Server's Agent port (9443 by default). You type no other URL: the Server sends its API Address (the Dashboard's `https://` URL) at registration and every time the Agent connects. A wrong Server Key makes the install fail at once and leaves nothing behind.

The API Address must be `https://`. `http://` is accepted only for this machine itself (`localhost`), for tests or a Server on the same machine; no flag allows plain HTTP to another machine. If the Server has no API Address yet, Controller commands answer `the Server has no API Address yet`; set it on the Server Settings page or with `rcli api-address set`.

`rcli connect` checks the Device ID and Device Password, adds the Device to your OS user's Address Book (`--name` and `--tag`, which can be repeated, help you tell Devices apart) and remembers the password in the OS keychain: Keychain on macOS, Credential Manager on Windows, Secret Service on Linux, or a `0600` file when there is no keychain. Later commands then need only the Device ID or the name. With `--no-remember` the password is not kept, and each command on that Device asks for it on the terminal. `rcli forget <Device ID>` removes a Device from your Address Book with its remembered password.

`rcli uninstall` (as the user who installed) removes the Device from the Server and removes the Agent and the Controller configuration. Each user's remembered Device Passwords stay for the next install.

## No roles

Every User can do everything: drive the Devices in their own Address Book and run the Server (Users, Server Key, Server Address, Agent Builds, force-releasing a Control Lock). What a Controller may do on a Device is decided by that Device's own Device Settings (see [Device Settings](/en/devices/#device-settings)). No User can disable themselves or the last enabled User.

## The fleet

```bash
rcli devices                         # your Address Book with each Device's Device Status (Online / Offline)
rcli device show demo-pc             # inventory, current resources read from the Agent now, lock or login screen
rcli device processes demo-pc        # the process list, read now
rcli displays demo-pc                # displays: resolution, scale, position
```

A Device can be named by its Device ID or by its name in your Address Book; if two Devices share a name, use the Device ID. The Server only connects Controllers to Devices: it keeps no resource history, inventory or audit log, and it lists no Device you have not added with its Device ID and Device Password.

## See and control the screen

**Seeing** needs no Control Lock:

```bash
rcli shot demo-pc --out ./shots                 # a screenshot (JPEG, long side 1280)
rcli shot demo-pc --display all --out ./shots   # one image per display
rcli shot demo-pc --marks                       # number the Marks on the image and print the Mark table
```

When the screen has not changed since the last image, you get `unchanged` instead of a new image.

Each Mark has a `number`, a `text` (its label), a `role`, a `display` and a box. Containers (`window`, `dialog`, `menu`, `list`) are Marks too: they are numbered, drawn on the image and can be targeted by number or text. Three more fields appear only when they are not empty:

- `states`: the Mark States that differ from the default, among `disabled`, `focused`, `checked`, `selected`, `expanded` and `collapsed`.
- `value`: the content (the text in a field, a slider's value), apart from the `text` label. A password field never has a `value`. Targeting by text matches `text` only.
- `parent`: the number of the nearest container Mark, to tell two "OK" buttons in two dialogs apart.

The CLI's Mark table has `PARENT`, `STATES` and `VALUE` columns (long values are cut; `--json` has them whole). Marks found by OCR (at the lock screen, where there is no accessibility tree) have none of the three.

On macOS, Marks come from the focused window, the menu bar, open menus and that app's sheets. Other windows of the app behind it give no Marks. A field whose only label is its own content has an empty `text` and its content in `value`, so target it by Mark number. Static text keeps its content as `text`.

**Wait Conditions** wait until some text appears or disappears before taking the screenshot, instead of guessing a delay:

```bash
rcli shot demo-pc --until-appears "Saved" --marks          # wait for "Saved"
rcli shot demo-pc --until-disappears "Loading" --timeout-ms 20000
rcli shot demo-pc --until-appears "OK" --role button       # only Marks with role button count
```

In the API, add `"until": {"appears": {"text": "Saved", "role": "button"}, "timeout_ms": 5000}` (or `"disappears"`) to an `actions` request with observe `shot`. The Agent reads the Marks about every 100 ms (by OCR when there is no accessibility tree, so it works at the lock and login screens) and returns the Observation when two readings in a row agree. Text matches as it does for text targets: case-insensitive, exact before contained. `timeout_ms` is 5000 by default and at most 30000. On timeout the call fails with `wait_timeout`, and its `error` object still carries the last reading's `images` (with `observation`, and `marks` if you asked for them). Waiting needs no Control Lock.

You can also wait on a Mark's value or Mark State (the Mark is found by its text; add `--role` if needed):

```bash
rcli shot demo-pc --until-value "Name" --equals "An"             # the Name field's value is exactly "An"
rcli shot demo-pc --until-value "Status" --contains "done"       # the value contains "done"
rcli shot demo-pc --until-state "Remember me" --has checked      # the checkbox is ticked
rcli shot demo-pc --until-state "Save" --role button --lacks disabled   # the Save button is enabled
```

In the API: `"until": {"value": {"text": "Name", "role": "field", "equals": "An"}}` (or `"contains"`), and `"until": {"state": {"text": "Remember me", "has": "checked"}}` (or `"lacks"`), with `timeout_ms` as above. Values are compared case-sensitively. These two need an accessibility tree: on a screen read only by OCR (the lock and login screens) they fail at once with `no_accessibility_tree`. When the text matches several Marks they fail at once with `ambiguous_text` and the list of matches; add `role` to narrow it to one.

**Controlling** needs the Control Lock:

```bash
rcli lock acquire demo-pc
rcli act demo-pc click 640 360 --out ./shots          # coordinates in the Observation image; the Agent maps them to real pixels
rcli act demo-pc click --text "OK"                    # click by on-screen text
rcli act demo-pc click --mark 3 --observation <id>    # click a Mark from an earlier --marks Observation
rcli act demo-pc type "hello" --observe none          # no image back
rcli act demo-pc type "secret" --secret               # a password
rcli act demo-pc key ctrl+shift+t
rcli act demo-pc scroll 640 360 --dy 3
rcli act demo-pc set-value --mark 4 --observation <id> "secret"   # set a field's value without keystrokes
rcli lock release demo-pc
```

A click, double click or right click on a Mark from the accessibility tree first tries the accessibility action (press the button, focus the field, open the item, open the context menu), so it still lands when the element is covered or has moved; only if that fails does the Agent use a simulated mouse. Coordinates and Marks from OCR always use the simulated mouse. `type` always sends real keystrokes. `set-value` (API: `{"type":"set_value", TARGET, "value":".."}`) takes only Marks from the accessibility tree (macOS, Windows, Linux).

Observation modes (`--observe`):

| Mode | Returns | Use it for |
|---|---|---|
| `shot` (default) | one image, after waiting up to about 1 s for the screen to settle | most things |
| `none` | no image | typing in a row without looking |
| `frames` | only the frames that changed visibly, with timestamps (at most 10; the API's `max_frames` takes 1–30) | following an animation |
| `clip` | an H.264 MP4 of up to 5 s, saved as `clip.mp4` (`rcli act` only, not `rcli batch`) | a person reviewing what happened |

**Action Batch**: several Actions in one request, one Observation:

```bash
echo '[{"type":"key","keys":"ctrl+a"},{"type":"wait","ms":200},{"type":"type","text":"hi"}]' \
  | rcli batch demo-pc - --observe frames --out ./shots
```

A `wait` step also takes a Wait Condition (the same `until` as above), checked after `ms`. Click Save, wait for "Saved", then take the screenshot, in one request:

```bash
echo '[{"type":"click","text":"Save"},{"type":"wait","until":{"appears":{"text":"Saved"},"timeout_ms":10000}}]' \
  | rcli batch demo-pc -
rcli act demo-pc wait --until-disappears "Loading" --timeout-ms 20000
```

When a step fails, the batch stops there and reports its `step` (counted from 0); a Wait Condition that times out reports `wait_timeout`, the `step` and the last reading's Observation. The total waiting in one request (the `wait` steps plus the `timeout_ms` of their Wait Conditions) is at most 30000 ms.

**Common errors:**

- `control_lock_held`: someone else holds the lock; the error names them and says when it expires. Wait and try again; there is no queue.
- `stale_mark`: the Mark number comes from an old Observation; take a new one with `--marks`.
- `ambiguous_text` / `text_not_found`: the text matches several places, or none.
- `wait_timeout`: the Wait Condition did not hold within `timeout_ms`; the error carries the last reading's Observation.
- `no_accessibility_tree`: a Wait Condition on a value or Mark State, on a screen with no accessibility tree (OCR only).
- `agent_too_old`: the Device's Agent is older than 0.4.0 and does not know Wait Conditions. Update the Agent and try again.

**The Control Lock** ends by itself after 10 minutes without an Action (every request from the holder, even a screenshot, extends it). `rcli mcp` and the Dashboard also lose it 60 s after their connection drops; the plain CLI keeps no connection open, so for it only the 10 minutes count. Any User can end someone else's lock on a Device in their Address Book with `rcli lock force-release demo-pc`.

## Lock and login screens

rcli stores no OS password anywhere. The Controller types the password itself, with `type --secret`:

```bash
rcli lock acquire demo-pc
rcli act demo-pc type '...' --secret --observe none
rcli act demo-pc key enter --out ./shots
```

`rcli device show` reports the `session`: `Active`, `LockScreen` or `LoginScreen`. Lock and login screens are reached only in Service Mode (see [Install on a Device](/en/devices/)). On macOS the login window can be seen but not typed into, so a Mac that must survive a reboot needs automatic login.

### Ctrl+Alt+Del on Windows

Windows lets only a SYSTEM process send the secure attention sequence (`SendSAS`), and only after a policy has been turned on once on each Device (it is off by default):

```powershell
# PowerShell as administrator, once:
Set-ItemProperty -Path 'HKLM:\SOFTWARE\Microsoft\Windows\CurrentVersion\Policies\System' `
  -Name SoftwareSASGeneration -Type DWord -Value 1
# 0 = off (default), 1 = services (enough for rcli), 2 = services and apps
```

Then, from a Controller: `rcli lock acquire <Device ID>` and `rcli act <Device ID> key ctrl+alt+del`. Without the policy, Windows ignores `SendSAS` and nothing happens.

## Terminal

```bash
rcli exec demo-pc -- echo hello                     # prints stdout/stderr, exits with the command's exit code (124 on timeout)
rcli exec demo-pc --timeout 300 -- ./long-job.sh    # seconds, 60 by default, at most 3600
rcli exec demo-pc --as root -- whoami               # as root/SYSTEM, if the Device's root setting allows it
rcli shell demo-pc                                  # an interactive shell, like SSH
rcli root on demo-pc                                # make --as root the default for exec and shell on this Device
```

The terminal needs no Control Lock. Commands run as the Device's Terminal User. `--as root` follows the Device's root setting: `never`, `ask` (a person at the Device accepts; the default) or `always`. On Windows, a command that is not `--as root` runs in the Terminal User's signed-in session, and fails with `terminal_user_not_signed_in` when that user is not signed in.

## Power and Wake

```bash
rcli power demo-pc lock         # lock the screen; needs no Control Lock
rcli power demo-pc restart      # restart, shutdown or sleep: refused while someone else holds the Control Lock
rcli wake demo-pc               # Wake through a Wake Relay; the Device comes Online when it is up
```

Your Address Book keeps each Device's MAC address and subnet from the last time it was seen Online (CLI: at `rcli connect` and `rcli devices`; Dashboard: when the Agent connects). Wake sends them to the Server, which picks an Online Agent on that subnet as the Wake Relay. A Device never seen Online since you added it answers `no_wake_address`; with no Online Agent on its subnet, `no_wake_relay`.

## Server administration

These commands need a User session: `rcli login <name>` (the Dashboard password, plus the TOTP code if the User turned TOTP on), and `rcli logout` to end it. A connection made with a Device Password cannot run them; they answer `user_session_required`.

```bash
rcli login an                                 # asks for the Dashboard password (and the TOTP code); saves the session
rcli logout                                   # revokes and forgets the saved session
rcli user create an
rcli user set-password an --password '...'   # set the Dashboard password (the User's TOTP stays)
rcli user list
rcli user disable an
rcli server-key show                          # the Server Key (rk_<CA fingerprint>_<secret>) for new installs
rcli server-key rotate                        # new secret: registered Devices stay Online, new installs need the new key
rcli server-address show                      # the Server Address Agents use
rcli server-address set rcli.example.com:9443 # reissues the Agent port's TLS certificate for the new name
rcli api-address show                         # the API Address
rcli api-address set https://rcli.example.com # correct it; Agents get it on their next connection
rcli build list                               # Agent Builds, newest first, the Latest Build marked
rcli build upload ./rcli-1.0.0-linux-x86_64 --version 1.0.0 --target linux-x86_64   # the Server computes the sha256
rcli build delete 1.0.0 linux-x86_64          # without a target, deletes every target of that version
rcli package publish rcli_1.0.0_amd64.deb --arch x86_64   # a .dmg, .deb or .rpm for the Connect Guide's downloads
```

To update a machine to the Latest Build for its OS and architecture (on a Service Mode Device, with `sudo` or as Administrator, so the Agent service is restarted too):

```bash
sudo rcli update
```

It prints the running version, the Latest Build, the download progress and the sha256 check, then restarts the Agent. It never downgrades. `rcli upgrade` still works as a hidden alias.

The **Minimum Version** on the Server Settings page marks Devices below it "update needed", makes Controllers below it print `Server recommends rcli ≥ <version>, run: rcli update`, and makes Agents below it update themselves to the Latest Build. The order to follow is in [Upgrading](/en/server/#upgrading).

## Dashboard

Open the Dashboard's `https://` URL (the install command prints it) and sign in with your name and password, plus a TOTP code if you turned TOTP on. Each User turns TOTP on or off for themselves on the Users page: scan the QR code, then type a code to confirm. Five wrong sign-ins in a row lock the User out for 15 minutes.

- **Devices**: your Address Book (add a Device with its Device ID and Device Password) with each Device Status. The `⋯` menu on each row renames a Device in your Address Book or removes it.
- **A Device's page**: inventory, current resources (read from the Agent), lock or login screen, who holds the Control Lock and until when, and the Wake, Lock screen, Sleep, Restart and Shut down buttons.
- **Live View**: the screen live over WebRTC, with a display picker when the Device has several. The video comes at the display's own resolution; pick a Live View Quality on the toolbar (Quality: sharpest, Balanced: the default, Speed: smoothest), which changes without reconnecting. While you hold the Control Lock, click and type straight on the video. When someone else holds it, you can still watch and see who holds it until when.
- **Web terminal**: under Live View, with an "as root" box.
- **Hướng dẫn kết nối** (Connect Guide): the Server Address, the Server Key, the install commands and the downloads.
- **Quản lý** (administration, every User): Users, Agent updates (upload and delete Agent Builds) and Server Settings (Server Key, Server Address, API Address, Minimum Version).

## Claude Code (MCP)

```bash
rcli connect 123456789                        # on a machine where rcli is installed
claude mcp add rcli -- rcli mcp
```

`rcli mcp` uses the install's configuration and the Device Passwords you remembered, so it needs no environment variables or other setup. If `rcli` is not on the `PATH` Claude Code sees, give its full path.

Tools: `devices`, `device`, `lock_acquire`, `lock_release`, `lock_status`, `observe` (shot, frames, clip, Marks and the Wait Condition `until`), `act`, `batch`, `exec`, `wake` and `power`. Images come back inline and the Mark table is in the structured result. `device: "local"` drives the Local Device.

At a lock or login screen, the AI agent types the OS password the user gave it, with `type` and `"secret": true`; rcli does not store it. MCP has no `shell` tool; use `exec`.
