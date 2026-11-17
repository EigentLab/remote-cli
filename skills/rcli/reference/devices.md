# Device commands

`<dev>` is a Device ID or the name you gave it in your Address Book; when two Devices share a name, use the ID. None of these need a User session.

## Reach and inspect

| Command | Does |
|---|---|
| `connect <id> [--password P] [--name N] [--tag T]… [--no-remember]` | Checks the Device Password and adds the Device to your Address Book; the password goes to the OS keychain. Prompts when `--password` is omitted. With `--no-remember` every later Device command asks for the password on the terminal, so an agent cannot use that Device unattended. |
| `forget <dev>` | Removes the Device from your Address Book, with its remembered Device Password. |
| `devices` | Your Address Book: ID, name, Device Status (`Online`, `Offline`, `Waking`, `Offline (Wake failed)`), last seen, tags, Agent version. |
| `device show <dev>` | Inventory, current resources, `session` (`Active`, `LockScreen`, `LoginScreen`), read from the Agent now. |
| `device processes <dev>` | Process list fetched from the Agent now. |
| `displays <dev>` | Displays: index, resolution, scale, position, primary. |

The Address Book belongs to the OS user running `rcli`; a Dashboard user has a separate one.

## Observe: `shot`

No Control Lock needed. `rcli shot <dev> [--display N|all] [--marks] [--out DIR] [Wait Condition]`

- Saves `DIR/display-<N>.jpg` (long edge 1280) and prints one line per file. A display that did not change since your last Observation prints `unchanged` and no file: reuse the previous image.
- `--display` defaults to the display last acted on.
- `--marks` numbers the detected Marks on the image and prints `observation <ID>` plus a table: `MARK DISPLAY ROLE BOX PARENT STATES VALUE TEXT`. `VALUE` is cut at 20 chars; `--json` has the full value. Containers (`window`, `dialog`, `menu`, `list`) are Marks too; `PARENT` tells two "OK" buttons in two dialogs apart. Password fields never show a value. Marks read by OCR (lock and login screens) have no parent, states or value.

## Wait Conditions

Wait for the screen instead of guessing a sleep. Flags on `shot` and `act … wait`; `until` objects in `batch`.

| Flag | Holds when |
|---|---|
| `--until-appears T` | text T is on screen |
| `--until-disappears T` | text T is gone |
| `--until-value T --equals V` / `--contains V` | the Mark with text T has that value (case-sensitive) |
| `--until-state T --has S` / `--lacks S` | the Mark with text T has / lacks Mark State S (`disabled`, `focused`, `checked`, `selected`, `expanded`, `collapsed`) |

Add `--role R` to count only Marks with that role; `--timeout-ms` defaults to 5000, at most 30000. Text matches case-insensitively, equal or contained. `value` and `state` need an accessibility tree: on OCR-only screens they fail at once with `no_accessibility_tree`.

## Control: `lock`, `act`, `batch`

**Control Lock** (`lock acquire|release|status <dev>`): needed for `act` and `batch`, except on `local`. It ends after 10 minutes without an Action from the holder (any request from the holder, a shot included, extends it) or on release. Someone else's lock fails with `control_lock_held` plus holder and expiry; there is no queue, so wait and retry, or ask the user.

**`act <dev> <action>`** sends one Action and returns an Observation. Common flags (after the action): `--display N` (the display the coordinates refer to, default 0), `--observe shot|frames|none|clip`, `--observe-display N|all`, `--marks`, `--out DIR`.

| Action | Form |
|---|---|
| `move`, `click`, `double-click`, `right-click` | `X Y` in Observation image pixels, or `--mark N --observation ID`, or `--text T` |
| `drag` | `X Y TO_X TO_Y` |
| `scroll` | `X Y [--dx N] [--dy N]` in wheel notches, at most 50 (positive `--dy` scrolls down) |
| `type` | `TEXT [--secret]` |
| `set-value` | `VALUE` with `--mark N --observation ID` or `--text T`: sets a field's value through the accessibility tree, without keystrokes |
| `key` | `KEYS`, e.g. `enter`, `ctrl+shift+t`, `ctrl+alt+del` |
| `wait` | `[--ms N] [Wait Condition]` |

Coordinates are in the image you were shown; the Agent maps them to real pixels.

| `--observe` | Returns | Use for |
|---|---|---|
| `shot` (default) | one image after the screen settles (≤ ~1 s) | most steps |
| `none` | no image | typing, steps you will check later |
| `frames` | up to 10 changed frames, `display-<N>-<ms>ms.jpg` | animations |
| `clip` | `clip.mp4`, up to 5 s (`act` only) | a human reviewing the step |

**`batch <dev> <file|->`** sends several Actions in one request with one Observation at the end. Flags: `--observe shot|frames|none`, `--display N|all`, `--out DIR`. Input is a JSON array of Actions, or a whole request body sent as-is.

```json
[
  {"type": "click", "text": "Save"},
  {"type": "wait", "until": {"appears": {"text": "Saved"}, "timeout_ms": 10000}},
  {"type": "key", "keys": "ctrl+a"},
  {"type": "wait", "ms": 200},
  {"type": "type", "text": "hello", "secret": false}
]
```

Action objects: `move` / `click` / `double_click` / `right_click` with `{"display": 0, "x": .., "y": ..}`, `{"mark": N, "observation": ID}` or `{"text": ".."}`; `drag` with `x`, `y` and `"to": {"display": 0, "x": .., "y": ..}`; `scroll` with `x`, `y`, `dx`, `dy`; `type` with `text`, `secret`; `set_value` with `value` and a Mark or text target; `key` with `keys`; `wait` with `ms` and an optional `until` (`appears`/`disappears`: `{"text", "role"}`; `value`: `{"text", "role", "equals"|"contains"}`; `state`: `{"text", "role", "has"|"lacks"}`; plus `timeout_ms`). A failing step stops the batch and reports `step` (0-based). All waits in one request add up to at most 30000 ms.

## Terminal: `exec`, `shell`, `root`

No Control Lock needed.

- `exec <dev> [--as root] [--timeout SECS] -- <command>`: the words after `--` are joined with spaces and run through the Device's shell (`sh -c` on Linux, `zsh -c` on macOS, PowerShell on Windows), so quote pipes and redirects as one argument. Prints stdout/stderr and exits with the remote exit code; 124 when it timed out (default 60 s). With `--json` it exits 0 and prints `stdout`, `stderr`, `exit_code`, `timed_out`.
- Commands run as the Device's Terminal User. `--as root` runs as root/SYSTEM only when the Device's root setting allows it (`never` / `ask` at the Device / `always`) and the Device is in Service Mode.
- `shell <dev> [--as root]`: interactive PTY, like SSH. Agents use `exec`.
- `root on|off <dev>`: makes `--as root` the default for `exec`/`shell` on that Device in this Controller's config.

## Power and Wake

- `power <dev> lock` locks the screen; no Control Lock needed.
- `power <dev> restart|shutdown|sleep`: refused while someone else holds the Control Lock. After `shutdown` or `sleep` the Device goes Offline; only `wake` brings it back.
- `wake <dev>`: wakes an Offline Device through a Wake Relay (another Online Device on its LAN). Device Status goes `Waking`, then `Online`, or `Offline (Wake failed)` after 3 minutes. `no_wake_address` means your Address Book never saw the Device Online: it must be powered on by hand once.

## Errors

Every error has a `code` and a `hint` with the next step. These need a decision:

| Code | Meaning, next step |
|---|---|
| `control_lock_held` | someone else holds the lock; wait for expiry or ask the user |
| `stale_mark` | Mark number from an old Observation; shoot again with `--marks` |
| `ambiguous_text` | text matches several Marks (list included); add `--role` or use a Mark number |
| `wait_timeout` | Wait Condition not met; the error carries the last Observation (and `step` in a batch): look at it before retrying |
| `no_accessibility_tree` | `value`/`state` Wait Condition or `set-value` on an OCR-only screen; use `appears`/`disappears` and `type` |
| `capability_off` | the Device Owner switched off mouse and keyboard, terminal or power for Controllers; report it to the user |
| `connect_denied`, `root_denied` | the person at the Device refused or did not answer in time; ask the user before retrying |
| `service_mode_required` | a User Mode Device cannot show lock/login screens, run as root or as another account; report it to the user |
| `agent_too_old` | the Device's Agent predates the feature the message names; it needs `update` |
| `user_session_required` | admin command without a User session; `rcli login` first |
