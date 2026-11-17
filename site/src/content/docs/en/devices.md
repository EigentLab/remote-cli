---
title: Install on a Device
description: Install rcli on Windows, macOS and Linux, in User Mode or Service Mode, and update or remove it.
sidebar:
  order: 3
---

Every machine runs the same `rcli` binary. Installing it once makes the machine a Device (others can drive it) and a Controller (it can drive others) at once.

## Release files

Each [GitHub release](https://github.com/EigentLab/remote-cli/releases) holds:

| File | For |
|---|---|
| `rcli-<version>-windows-x86_64.exe` | Windows x86_64 (save it as `rcli.exe`) |
| `rcli-<version>-macos-aarch64.dmg` | macOS on Apple silicon: `rcli.app` |
| `rcli-<version>-macos-aarch64` | macOS on Apple silicon: the bare binary |
| `rcli_<version>_amd64.deb` | Debian and Ubuntu x86_64 |
| `rcli-<version>-linux-x86_64` | Linux x86_64: the bare binary |
| `SHA256SUMS` | their checksums: `sha256sum -c SHA256SUMS --ignore-missing` |

The three bare binaries are also what a Server hands out as Agent Builds (see the [Server guide](/en/server/#install)). The builds are not code-signed, so Windows and macOS warn the first time they are opened (see [Windows](#windows) and [macOS](#macos)).

## User Mode and Service Mode

| | User Mode | Service Mode |
|---|---|---|
| Installed with | `rcli install`, as your user | `sudo rcli install --server … --key …` (an Administrator prompt on Windows) |
| Runs | as you, while you are logged in | as root or SYSTEM, from boot |
| Lock and login screens, UAC | no | yes |
| Terminal as another user or root | no | yes |
| Input on Wayland | no | yes |
| Server | optional: without one, only local control | required |
| Controller configuration | yours alone | shared by every user of the machine |

`rcli service install` moves a User Mode install to Service Mode, and `rcli service uninstall` moves it back. Both ask for root at the Device and keep the Device ID, the Device Password and the Device Settings. The rcli window (This Device → Install Mode) has the same switch.

## From the Connect Guide

The Dashboard's Connect Guide page (**Hướng dẫn kết nối**) shows the install commands with your Server's address and Server Key filled in. They install in Service Mode and download `rcli` from the Server's Latest Build for the machine's OS and architecture, so the Server must hold Agent Builds first.

- **Linux and macOS**: `curl -fsSL https://rcli.example.com/install.sh | sudo sh -s -- --key <Server Key>`. On a Server on an IP address, the command first writes the Server's CA certificate to `rcli-ca.pem` and gives it to `curl`; copy it as shown.
- **Windows**: in PowerShell, `iwr https://rcli.example.com/install.ps1 -OutFile install.ps1; .\install.ps1 -Key <Server Key>` (it asks for UAC once). On a Server on an IP address the page shows no PowerShell command: download `rcli.exe` from the page in the browser (accept the certificate warning, or [import the CA](/en/server/#the-ca-certificate) first), open it and fill in the Server Address and Server Key.

The script stops with `no rcli build for <os>-<arch> is rolled out` when the Server has no Agent Build for that target.

## With the binary

```bash
sudo rcli install --server rcli.example.com:9443 --key rk_xxxxxxxx_yyyyyyyy   # Service Mode
rcli install --server rcli.example.com:9443 --key rk_xxxxxxxx_yyyyyyyy        # User Mode
rcli install                                                                  # User Mode, local control only
```

The install prints:

```text
Device ID: 123456789
Device Password: abcdEFGH12
```

- `--name` sets the Device name (default: the host name). `--terminal-user` sets the account terminal commands run as (default: the account that ran `sudo`). `--allow-root` sets the root setting to `always` instead of `ask`. `--key -` reads the Server Key from stdin, so it never sits on a command line.
- A wrong Server Key (a wrong secret or a wrong CA fingerprint) makes the install fail at once: no Device is created and nothing is installed. The Agent also refuses a Server whose CA does not match the fingerprint in the Server Key.
- `rcli agent info` (any local user) shows the Device ID and whether the Agent is connected. `sudo rcli agent password show` shows the Device Password; in User Mode, run it without `sudo` and type your OS password.
- Running `rcli install` again keeps the Device. After `rcli uninstall`, the next install is a new Device with a new Device ID and Device Password, in nobody's Address Book.
- A User Mode install without a Server gets one later with `rcli agent server set rcli.example.com:9443 --key <Server Key>`.

## Device Settings

The person at the Device decides what Controllers may do there. In Service Mode these commands need root; in User Mode the Device Owner runs them and types their OS password for any change that opens the Device further.

```bash
sudo rcli agent terminal-user set alice           # the account terminal commands and shells run as
sudo rcli agent root ask                          # --as root: never | ask (the default) | always
sudo rcli agent accept password+accept            # a person at the Device accepts each new connection (or: password)
sudo rcli agent capability terminal off           # mouse-keyboard | terminal | power, on or off
sudo rcli agent password set                      # a new Device Password, read from stdin; open sessions keep working
sudo rcli agent id set 123456789                  # another free 9-digit Device ID
sudo rcli agent general auto-update off           # do not install Agent updates from the Server
rcli agent prompts                                # prompts waiting for the person at the Device
rcli agent disconnect                             # end every Controller's sessions and Control Lock
```

The rcli window (release builds) shows the same settings, the connected Controllers and the prompts.

## Windows

**Service Mode**, three ways with the same result:

1. Open `rcli.exe`: the rcli window asks for the Server Address and the Server Key; click Install and accept UAC once.
2. The PowerShell command from the Connect Guide.
3. In PowerShell "Run as administrator", with the downloaded binary: `.\rcli.exe install --server rcli.example.com:9443 --key <Server Key>`.

The Agent runs as the Windows service `rcli-agent` (LocalSystem, starts with Windows). The binary is in `%ProgramFiles%\rcli\rcli.exe`; the Device's data (credentials, Device Settings, Secret Store) is in `%ProgramData%\rcli`, readable only by SYSTEM and Administrators. The Server Key is in `%ProgramData%\rcli\service.json`, not on the service's command line. The Secret Store uses DPAPI with machine scope, so a copied disk or backup does not reveal the Device Password on another machine. The log is `%ProgramData%\rcli\agent.log`.

**User Mode**: `.\rcli.exe install` in a normal PowerShell puts rcli in `%LOCALAPPDATA%\Programs\rcli` and its data in `%LOCALAPPDATA%\rcli`, and starts it at your sign-in.

**Lock screen, sign-in screen and UAC.** The service runs in session 0 and starts a helper in the console session with its own SYSTEM token, which can open the Winlogon desktop and UAC's secure desktop. So `shot` and `act` work at the lock screen, the sign-in screen and UAC prompts, and the helper follows sign-ins, sign-outs and user switches. No Screen Recording permission is needed. OCR (`--text`, Wait Conditions) at the lock screen runs under the SYSTEM profile: if it reads no text there, install the OCR language pack for the machine. For Ctrl+Alt+Del, see [Ctrl+Alt+Del on Windows](/en/usage/#ctrlaltdel-on-windows).

**Live View.** Builds made with the `hwcodec` feature (see [Build from source](/en/build/)) encode H.264 on the GPU with NVENC, AMF or QSV; without a GPU encoder (a virtual machine) or without the feature, the Agent uses openh264. The Agent's log names the encoder (`live stream …: H.264 through h264_nvenc` or `… through openh264`).

**SmartScreen.** The build is not signed, so Windows may show "Windows protected your PC" when you open `rcli.exe`: click More info, then Run anyway.

**Uninstall**: `rcli uninstall` in PowerShell "Run as administrator" (Service Mode) or in a normal PowerShell (User Mode). It removes the Device from the Server (if the Server cannot be reached it only warns, and still removes the local install), stops and deletes the service, and deletes `%ProgramFiles%\rcli` and `%ProgramData%\rcli`, the Secret Store included.

## macOS

Open the `.dmg` and drag `rcli` to Applications, then open it. The build is not signed: if macOS refuses to open it, right-click the app and choose Open, or run `xattr -d com.apple.quarantine /Applications/rcli.app`. The rcli window asks for the Server Address and the Server Key; Install asks for an administrator's password and installs in Service Mode. The Connect Guide's `curl` command does the same from a terminal. `rcli install` without `sudo` installs in User Mode: the app goes to `~/Applications/rcli.app` and `rcli` to `~/.local/bin/rcli`.

**Permissions.** Allow rcli **Screen Recording** and **Accessibility** in System Settings → Privacy & Security; the rcli window has buttons that open both panes. Without Screen Recording, `rcli shot` fails with `grant rcli Screen Recording in System Settings`; without Accessibility, input fails and Marks fall back to OCR.

**Service Mode** runs the Agent as a root LaunchDaemon, with a helper in the login window and in each user's session. The login window can be seen but not typed into, so a Mac that must come back after a reboot needs automatic login. The log is `/Library/Logs/rcli-agent.log` (User Mode: `~/Library/Logs/rcli-agent.log`).

**Uninstall**: `sudo rcli uninstall` (Service Mode) or `rcli uninstall` (User Mode). It removes the Device from the Server, the launchd jobs, the app and the Device's local data.

## Linux

**Requirements:** systemd, and for the screen a graphical session on **X11 (Xorg) or Wayland** (GNOME, KDE, wlroots). X11 needs `xauth`. Wayland needs `xdg-desktop-portal` with a ScreenCast backend (`xdg-desktop-portal-gnome`, `-kde` or `-wlr`) running in the user's session. Install `tesseract-ocr` for OCR. `systemd-creds` (in current systemd) protects the Device Password with the TPM when there is one.

### Service Mode

```bash
chmod +x rcli-1.0.1-linux-x86_64
sudo ./rcli-1.0.1-linux-x86_64 install --server rcli.example.com:9443 --key rk_<fingerprint>_<secret>
```

```text
installed rcli-agent.service and /usr/local/lib/rcli/rcli
Device ID: 123456789
Device Password: abcdEFGH12
```

The Agent runs as the root service `rcli-agent.service`, and `rcli` is linked at `/usr/local/bin/rcli`. The service finds the graphical session on seat0 (X11 or Wayland, the lock screen, or the GDM, LightDM or SDDM greeter) and runs its helper `rcli agent session` as that session's user, so Controllers can see and drive the lock screen and the login screen as well as a logged-in session. The Server Key is in `/etc/rcli/agent.env` (mode `0600`, root only), not on the service's command line. The Terminal User defaults to the account that ran `sudo` (`SUDO_USER`).

Check it:

```bash
systemctl status rcli-agent.service --no-pager   # active (running)
rcli agent info                                  # Device ID and "connected"
sudo journalctl -u rcli-agent.service -n 30 --no-pager
```

The service is enabled, so it comes back after a reboot.

### User Mode

`rcli install` without `sudo` puts rcli in `~/.local/lib/rcli/rcli`, links it at `~/.local/bin/rcli` (put that directory on your `PATH`), and starts it at login through the systemd user unit `~/.config/systemd/user/rcli-agent.service`.

### The .deb package

The package installs `/usr/bin/rcli`, and its install hook runs `rcli install` in Service Mode with the Server Address and Server Key from `/etc/rcli/agent.conf`. Write that file first:

```bash
sudo install -d /etc/rcli
sudo install -m 600 /dev/null /etc/rcli/agent.conf
sudo tee /etc/rcli/agent.conf >/dev/null <<'CONF'
RCLI_SERVER=rcli.example.com:9443
RCLI_SERVER_KEY=rk_<fingerprint>_<secret>
# optional:
# RCLI_TERMINAL_USER=alice
# RCLI_ALLOW_ROOT=1
CONF
sudo apt install ./rcli_1.0.1_amd64.deb
systemctl status rcli-agent.service --no-pager
```

Without `/etc/rcli/agent.conf` the package still installs and prints what to do; then run the install step yourself:

```bash
sudo rcli install --server "$(sudo sed -n 's/^RCLI_SERVER=//p' /etc/rcli/agent.conf)" \
     --key "$(sudo sed -n 's/^RCLI_SERVER_KEY=//p' /etc/rcli/agent.conf)"
```

Removing the package runs `rcli uninstall` first, so the Device is removed from the Server and its local data is deleted:

```bash
sudo apt remove rcli
sudo apt purge rcli         # also deletes /etc/rcli/agent.conf
```

### Wayland

A Wayland user session (GNOME, KDE, Sway, Hyprland) can be driven without switching to Xorg. The screen is captured through the XDG desktop portal (`org.freedesktop.portal.ScreenCast`) and PipeWire; input is written by the root service to `/dev/uinput`, so input on Wayland needs Service Mode.

- **Approve once:** the first time the Agent captures the screen, a screen-sharing dialog appears at the Device. Choose the screens (several if you like) and click Share. The portal returns a restore token, which the Agent keeps in `/var/lib/rcli/screencast.restore_token` (root only), so it does not ask again, even after a reboot.
- **No portal:** `rcli shot` fails with "screen capture on Wayland needs the XDG desktop portal's ScreenCast". Install `xdg-desktop-portal` and a backend, then `systemctl --user restart xdg-desktop-portal` and restart `rcli-agent.service`.
- **No `/dev/uinput`:** input fails with a clear error. Load the module with `sudo modprobe uinput`, and add `uinput` to `/etc/modules-load.d/uinput.conf` to keep it after a reboot.
- **Several displays:** only the screens chosen in the portal dialog can be captured, and `rcli displays` lists exactly those.
- **Keyboard:** text is typed with the US layout (evdev key codes); with another layout only single keys are right. Named keys (`enter`, `ctrl`, `f5`…) use evdev codes.
- **Lock screen:** it is the same Wayland session, so the same stream is used. If the compositor stops the stream while locked, `rcli shot` reports "the screen cast stream sent no frame".
- **GDM's Wayland greeter** has no portal, so `rcli shot` reports "screen capture at a Wayland login screen is not available". To drive the login screen, set `WaylandEnable=false` in `/etc/gdm3/custom.conf` and `sudo systemctl restart gdm`: the greeter then runs on Xorg.

### Headless servers

The same `sudo rcli install --server … --key …` works without a display: the service runs and reconnects after a reboot, and `rcli device show`, `rcli exec`, `rcli shell` and `rcli power` work. `rcli shot`, `rcli act` and Live View fail with "no X11 session on the console".

## Updating

- `sudo rcli update` (`rcli update` in User Mode) replaces the binary with the Server's Latest Build after checking its sha256, and restarts the Agent. It never downgrades.
- With the `.deb`: `sudo apt install ./rcli_<version>_amd64.deb` installs the new binary and restarts `rcli-agent.service`; the Device ID stays.
- Agents below the Server's Minimum Version update themselves (see [Upgrading](/en/server/#upgrading)).

## Uninstalling

`rcli uninstall`, as the same user who installed (`sudo` for Service Mode, Administrator on Windows). It removes the Device from the Server, the Agent and the Controller configuration, and the Device's local data. Each user's remembered Device Passwords stay.
