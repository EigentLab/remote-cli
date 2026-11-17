# Server administration

All of these need a User session (`rcli login`); without one they answer `user_session_required`. There are no roles: any User can run them. Every write command here needs the user's go-ahead first.

## Session

| Command | Does |
|---|---|
| `login <name> [--password P] [--totp CODE]` | Signs in with a Dashboard account and saves the session; prompts for what is omitted. Ask the user for the password, and for the current 6-digit TOTP code when their account has TOTP on. |
| `logout` | Revokes and forgets the saved session. |

Global flags: `--server URL` points at another Controller API; `--token T` uses a session token instead of the saved one.

## Users

| Command | Does |
|---|---|
| `user list` | read |
| `user create <name>` | write |
| `user set-password <name> --password P` | write; the user's TOTP setting is kept. `RCLI_NEW_PASSWORD` in the environment replaces `--password` and keeps the password out of the process list. |
| `user disable <name>` | write; all of that user's sessions stop. |

## Server Key and addresses

| Command | Does |
|---|---|
| `server-key show` | prints `rk_<CA fingerprint>_<secret>` (a secret: show it only to the user who asked) |
| `server-key rotate` | **warn first**: registered Devices stay Online, but every new install and every Controller install configured with the old key needs the new one |
| `server-address show` | the `host:port` Agents connect to |
| `server-address set <host[:port],…>` | reissues the TLS certificate for the new names |
| `api-address show` | the Dashboard's public `https://` URL; empty until a User opens the Dashboard over https |
| `api-address set <https://…>` | Agents get it on their next connection |

## Control Lock

`lock force-release <dev>`: write; ends someone else's Control Lock. Ask the user, and tell them who held it (`lock status <dev>`).

## Agent Builds and packages

| Command | Does |
|---|---|
| `build list` | Agent Builds, newest first; the highest version per target is the Latest Build, which `rcli update` installs |
| `build upload <file> --version X.Y.Z --target <os>-<arch>` | write; the Server computes the sha256. Targets look like `linux-x86_64`. |
| `build delete <version> [target]` | write; without a target it deletes every target of that version |
| `package publish <file.dmg\|.deb\|.rpm> --arch x86_64\|aarch64` | write; replaces the Connect guide's download for that kind and arch |
