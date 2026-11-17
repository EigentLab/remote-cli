---
title: Known limitations
description: What rcli 1.0.0 does not do, or does only in part.
sidebar:
  order: 5
---

## Platforms

- Release builds exist for Windows x86_64, macOS on Apple silicon and Linux x86_64 only. The Server's install script runs on Linux x86_64 only; elsewhere use the [manual install](/en/server/#manual-install).
- The builds are not code-signed: Windows SmartScreen and macOS Gatekeeper warn the first time they are opened.
- The Dashboard's text is in Vietnamese only.

## Devices

- User Mode cannot reach the lock and login screens or UAC, work before login, run the terminal as another user or root, or send input on Wayland. Service Mode can.
- The macOS login window can be seen but not typed into: a Mac that must come back after a reboot needs automatic login.
- GDM's Wayland greeter cannot be captured; switch it to Xorg to drive the login screen. Text typed on Wayland follows the US keyboard layout, and the first capture needs one approval at the Device.
- On Windows, Ctrl+Alt+Del needs a policy turned on once per Device, and a terminal command that is not `--as root` fails while the Terminal User is not signed in.
- MCP has no `shell` tool; use `exec`.
- Wake needs another Online Agent on the same subnet as the Device to wake.

## Server

- One Server, no high availability or cluster.
- `rcli exec` does not limit the size of a command's output, and a long `--timeout` (at most an hour) keeps the HTTP request open that long.
- Sign-in lockouts count per User, not per IP address.
- There is no rotation of the master key and no way to replace the CA. Losing the master key loses the CA: every Device must be installed again.
- A User who loses their TOTP authenticator cannot turn TOTP off; create a new User and disable the old one (see [Lost password](/en/server/#lost-password)).
- On an IP address, browsers warn about the Dashboard's certificate until the CA certificate is imported, and Windows Devices are installed from `rcli.exe` downloaded in the browser rather than a PowerShell command.
- Live View through the TURN relay has been tried only on simple networks; strict NATs have not been checked.
- Through a Cloudflare Tunnel, Cloudflare's limit on request size (100 MB on its free plans) may refuse large uploads, such as an Agent Build uploaded through the Dashboard.
- There is no uninstall command for the Server, and the install script does not change the firewall: the [Server guide](/en/server/) gives the commands.
