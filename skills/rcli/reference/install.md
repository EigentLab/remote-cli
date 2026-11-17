# Install and Device Settings

These run on the machine itself, not through the Server API. Every write command here needs the user's go-ahead first; when a command asks for root (`sudo`, a Windows elevation prompt), hand it to the user to run.

## User Mode and Service Mode

| | User Mode | Service Mode |
|---|---|---|
| Installed by | `rcli install` as a normal user | `rcli install` as root/Administrator, the install packages, or `rcli service install` |
| Runs as | that user, from login to logout | root/SYSTEM, from boot |
| Device Owner (may change Device Settings) | that user | root/Administrator |
| `local` (`shot`, `act`, `batch`) | that user | root, and the user logged in at the screen |
| Lock and login screens, `--as root`, another Terminal User | refused with `service_mode_required` | available |

## Install lifecycle

| Command | Does |
|---|---|
| `install [--server <host:port> --key <Server Key>] [--name N] [--terminal-user U] [--allow-root]` | Installs the Agent, registers the Device, prints its Device ID and Device Password, and sets up the Controller side (`connect`, `mcp`). Give `--server` and `--key` together; root needs both. In User Mode they may be left out: the Device then serves only `local` until `agent server set`. `--key -` reads the key from stdin and `RCLI_SERVER_KEY` from the environment, so it stays out of the process list. `--allow-root` sets the root setting to `always` instead of `ask`. |
| `uninstall` | Undoes `install`, run as the same user. Users' remembered Device Passwords stay. |
| `service install` | Moves this user's User Mode install to Service Mode. Keeps the Device ID, Device Password and Device Settings. Asks for root. |
| `service uninstall` | Moves a Service Mode install back to User Mode for the user running it. Keeps the same three. Asks for root. |
| `update` | Replaces this binary with the Server's Latest Build for this OS/arch after checking its sha256; never downgrades. As root on an installed Device it also restarts the Agent. |
| `mcp` | Serves the Controller operations as MCP tools on stdio (`claude mcp add rcli -- rcli mcp`). |

The `--server` of `install` is the Server's Agent port (`host:port`), unlike the global `--server`, a Controller API URL.

## `rcli agent …`: Device Settings

Talks to the running Agent over its local socket (`--socket` or `RCLI_AGENT_SOCKET` to override). "Owner" is the Device Owner from the table above. Every Owner command that changes something asks for that OS account's password on the terminal (root is not asked): hand those to the user to run.

| Command | Who | Does |
|---|---|---|
| `agent info [--json]` | any user | Device ID, connection status, Server |
| `agent password show [--json]` | Owner | prints the Device Password (a secret) |
| `agent password set [--password P]` | Owner | replaces it (≥ 8 chars; reads stdin when omitted, or `RCLI_DEVICE_PASSWORD`); open sessions keep working |
| `agent id set <9 digits>` | Owner | sets a free Device ID |
| `agent server set <host:port> [--key K]` | Owner | points the Agent at another Server (key from stdin when omitted, or `RCLI_SERVER_KEY`); it reconnects at once |
| `agent terminal-user set <user>` | Owner | the account `exec`/`shell` run as |
| `agent root never\|ask\|always` | Owner | whether `--as root` is refused, asked at the Device, or allowed |
| `agent root grant <who>` | Owner | allows root to one Controller for the current session only |
| `agent accept password\|password+accept` | Owner | whether a person at the Device must accept each new connection (rejected after 30 s) |
| `agent capability mouse-keyboard\|terminal\|power on\|off [--controller <who>]` | Owner | what Controllers may do, for the whole Device or one connection; off answers `capability_off` |
| `agent general auto-update\|session-window on\|off` | Owner | auto-update, and the Session Window that shows who is connected |
| `agent release-lock` | Owner | force-releases the Control Lock, whoever holds it |
| `agent disconnect [<who>]` | any user | ends a Controller's sessions and Control Lock; all when `<who>` is omitted |
| `agent prompts [--json]` | any user | prompts waiting for the person at the Device |
| `agent prompt answer <id> accept\|reject` | reject: any user; accept: Owner | answers one |
| `agent run …` | service | the Agent entry point the service starts; leave it to the service |

`<who>` is the Controller as shown at the Device, e.g. `alice`, `alice (Dashboard)` or `dev-laptop (10.0.0.5)`.

An agent answering its own connection or root prompt (`agent prompt answer … accept`, `agent root grant`) defeats the setting: ask the user every time.
