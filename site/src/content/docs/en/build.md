---
title: Build from source
description: Build rcli, its packages and the Server images yourself.
sidebar:
  order: 4
---

You need Rust (stable) and, for the Dashboard and this site, Node.js with pnpm.

```bash
git clone https://github.com/EigentLab/remote-cli
cd remote-cli
cargo build --release -p rcli                     # target/release/rcli: Controller and Agent in one binary
cargo build --release -p rcli --features gui      # with the rcli window (links the display libraries)
```

A development build and `cargo test` need nothing more: Live View encodes with openh264.

## Live View on the GPU (`hwcodec`)

Release builds turn on the `hwcodec` feature: Live View encodes H.264 on the GPU through RustDesk's `hwcodec` (VideoToolbox on macOS, VA-API on Linux, NVENC, AMF or QSV on Windows). Without a GPU encoder (a virtual machine), or when it fails to start, the Agent falls back to openh264 by itself; the Agent's log names the encoder in use (`live stream …: H.264 through h264_videotoolbox` or `… through openh264`).

`hwcodec` needs RustDesk's static FFmpeg, built with vcpkg once per build machine (with `pkg-config`, `nasm` and Python 3.10 or later on the `PATH`):

```bash
git clone https://github.com/microsoft/vcpkg ~/vcpkg && ~/vcpkg/bootstrap-vcpkg.sh
git clone https://github.com/rustdesk/rustdesk ~/rustdesk          # for its res/vcpkg overlay ports
```

- **macOS**: `~/vcpkg/vcpkg install ffmpeg --triplet arm64-osx --overlay-ports=$HOME/rustdesk/res/vcpkg` (`x64-osx` on an Intel Mac), then `VCPKG_ROOT=~/vcpkg cargo build --release -p rcli --features gui,hwcodec`.
- **Linux**: also install `clang`, `libva-dev`, `libdrm-dev`, `libx11-dev`, `cmake` and `ninja-build`, then `~/vcpkg/vcpkg install 'ffmpeg[amf,nvcodec]' --triplet x64-linux --overlay-ports=$HOME/rustdesk/res/vcpkg` (`arm64-linux` on ARM, with `VCPKG_FORCE_SYSTEM_BINARIES=1`). FFmpeg loads libva at run time: a machine without libva, a VA-API driver or a GPU uses openh264, at the greeter and the lock screen too.
- **Windows**: `vcpkg install ffmpeg[amf,nvcodec,qsv] --triplet x64-windows-static --overlay-ports=<rustdesk>\res\vcpkg`, then in PowerShell `$env:VCPKG_ROOT='C:\vcpkg'; $env:RUSTFLAGS='-C target-feature=+crt-static'; cargo build --release -p rcli --features gui,hwcodec` (a static CRT, because `hwcodec` and the `x64-windows-static` triplet use `/MT`). RustDesk's FFmpeg has no Media Foundation (`h264_mf`).

## Packages

```bash
VCPKG_ROOT=~/vcpkg packaging/macos/build-dmg.sh   # on a Mac: target/macos/rcli-<version>-macos-<arch>.dmg
packaging/linux/build-deb.sh                      # on Linux, needs dpkg-dev: packaging/linux/out/rcli_<version>_<arch>.deb
packaging/linux/build-rpm.sh                      # on Linux, needs rpm-build: packaging/linux/out/rcli-<version>-1.<arch>.rpm
```

All three build with `gui,hwcodec`, so they need `VCPKG_ROOT` as above. The macOS app is only ad-hoc signed.

## Server images

`compose.yaml` pulls the published images, and keeps a build path:

```bash
docker compose build              # builds ghcr.io/eigentlab/remote-cli-server and -dashboard from this tree
docker compose up -d --build      # or build and start
```

The images get the tag in `RCLI_VERSION` (default `latest`), so set it to keep a source build apart from the published ones.

## The platform contract suite

`rcli-contract` checks a real Device's screen, input and accessibility in each session (logged in, lock screen, login screen). On Linux, as root, against the running service:

```bash
cargo build --release -p rcli-agent --bin rcli-contract
sudo ./target/release/rcli-contract --session-socket /run/rcli/session.sock
```

Follow the prompts on the terminal (focus a text field, lock, unlock, log out, log in); it prints a Markdown table where every cell of the "Logged in", "Lock screen" and "Login screen" columns should be ✅. Without `--session-socket` it runs in the caller's own session. On Wayland the "Login screen" column fails, because GDM's greeter has no portal.
