---
name: rcli
description: Drive the `rcli` CLI. Use when asked to see, click or type on a remote Device's screen or on this machine's own screen, run a command on a Device, restart, sleep, lock or wake a Device, administer the rcli Server (users, Server Key, addresses, Agent Builds), or install rcli and change a Device's settings.
---

# rcli

`rcli` is one binary that is both Controller (sends Actions) and Agent (runs on a Device and carries them out). A Controller reaches a Device through the Server, with the Device's 9-digit Device ID and its Device Password; `rcli connect` saves both in your Address Book, and later commands take the ID or the name you gave. `rcli <command> --help` is the source of truth for flags; the reference files below carry what `--help` does not say.

## Rules

1. **Ask first** before any command that changes state outside the screen you are driving: `power restart|shutdown|sleep`, `exec --as root`, `root on`, and every write command in [admin](reference/admin.md) and [install](reference/install.md). Read commands (`show`, `list`, `status`, `devices`, `displays`, `shot`, `agent info`) and `power lock` run without asking.
2. **Lock bracket**: `rcli lock acquire <dev>` before `act`/`batch`, and `rcli lock release <dev>` when done, also when a step failed. The CLI keeps no watch open, so a forgotten Control Lock blocks every other Controller for 10 minutes.
3. **Secrets**: type an OS password the user gave you with `act … type '<pw>' --secret`. Keep it, Device Passwords and the Server Key out of files, scripts, logs and your replies; rcli never stores an OS password.
4. **One-shot terminal**: use `exec`. `shell` is interactive and needs a human at the keyboard.
5. **Parse with `--json`**: add `--json` when you read a value (Device ID, Device Status, lock holder, exit code). Exception: `shot`/`act`/`batch` with `--json` print base64 images instead of saving files; use the plain output there (JPEG under `--out` plus a Mark table) and `--json` only to read full Mark values.
6. **Errors carry a `code` and a `hint`**: follow the hint. The codes that need a decision from you are in [devices](reference/devices.md#errors).

## Setup check

- `rcli devices --json` lists your Address Book with each Device Status. `[]` means no Device was added yet; a connection error or a missing API Address means the install is not ready: report it, change nothing.
- Add a Device with `rcli connect <Device ID> --name <name>`. Ask the user for the Device Password; pass it with `--password` only if they agree to it showing in the process list, otherwise let `rcli` prompt.
- Admin commands answer `user_session_required` until `rcli login <name>` (Dashboard password, plus the TOTP code when that user turned TOTP on; both from the user). Each AI agent should use its own Dashboard user.

## The Local Device

`local` as `<dev>` is the machine `rcli` is installed on: `shot`, `act` and `batch` go straight to its running Agent, with no Server, no `connect` and no lock bracket. It works only for the Device Owner (the OS user running the Agent) or root; anyone else, and every other command, uses the Device ID like for a remote Device.

## Recipes

**Click a control by its Mark**

```bash
rcli shot dev --marks --out ./shots        # Read ./shots/display-0.jpg; note "observation N" and the Mark table
rcli lock acquire dev
rcli act dev click --mark 7 --observation N --out ./shots   # or: click --text "OK" / click 640 360
# Read the returned image to confirm the result
rcli lock release dev
```

`stale_mark` means the screen changed since that Observation: shoot again with `--marks`.

**Fill a field**: `rcli act dev set-value 'text' --mark 7 --observation N` writes the value through the accessibility tree, with no focus or keystrokes. On `no_accessibility_tree`, click the field and use `type`.

**Unlock a lock or login screen** (`rcli device show dev --json` → `session` is `LockScreen` or `LoginScreen`)

```bash
rcli lock acquire dev
rcli act dev type '<password the user gave>' --secret --observe none
rcli act dev key enter --out ./shots       # check the desktop appeared
rcli lock release dev
```

Windows may need `key ctrl+alt+del` first. `service_mode_required` means the Device runs in User Mode, which cannot reach these screens: tell the user, who can switch it with `rcli service install` at the Device.

**Run a command**

```bash
rcli exec dev --timeout 300 -- 'df -h / | tail -1'
```

Exit code is the remote one; 124 means it timed out. No Control Lock needed.

**Wake an Offline Device**

```bash
rcli wake dev                              # Offline → Waking
rcli devices --json                        # repeat every ~15 s until Online; after 3 min it shows Wake failed
```

## Reference

- [reference/devices.md](reference/devices.md): `connect`, `forget`, `devices`, `device`, `displays`, `shot`, `lock`, `act`, `batch` (JSON), Wait Conditions, `exec`, `shell`, `root`, `power`, `wake`, error codes. Read before your first `act`/`batch` or Wait Condition.
- [reference/admin.md](reference/admin.md): `login`, `logout`, `user`, `server-key`, `server-address`, `api-address`, `lock force-release`, `build`, `package`.
- [reference/install.md](reference/install.md): `install`, `uninstall`, `service` (User Mode and Service Mode), `update`, `agent …` (Device Settings, run on the Device itself), `mcp`.
