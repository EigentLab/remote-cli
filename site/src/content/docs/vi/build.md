---
title: Build từ mã nguồn
description: Tự build rcli, các package và image của Server.
sidebar:
  order: 4
---

Cần Rust (stable) và, cho Dashboard và trang này, Node.js với pnpm.

```bash
git clone https://github.com/EigentLab/remote-cli
cd remote-cli
cargo build --release -p rcli                     # target/release/rcli: Controller và Agent trong cùng một binary
cargo build --release -p rcli --features gui      # có cửa sổ rcli (link thư viện hiển thị)
```

Build dev và `cargo test` không cần gì thêm: Live View mã hoá bằng openh264.

## Live View trên GPU (`hwcodec`)

Bản release bật feature `hwcodec`: Live View mã hoá H.264 trên GPU qua `hwcodec` của RustDesk (VideoToolbox trên macOS, VA-API trên Linux, NVENC, AMF hoặc QSV trên Windows). Không có GPU encoder (máy ảo) hoặc encoder không khởi động được thì Agent tự chuyển sang openh264; log Agent ghi tên encoder đang dùng (`live stream …: H.264 through h264_videotoolbox` hoặc `… through openh264`).

`hwcodec` cần FFmpeg tĩnh của RustDesk, build bằng vcpkg một lần cho mỗi máy build (cần `pkg-config`, `nasm` và Python 3.10 trở lên trong `PATH`):

```bash
git clone https://github.com/microsoft/vcpkg ~/vcpkg && ~/vcpkg/bootstrap-vcpkg.sh
git clone https://github.com/rustdesk/rustdesk ~/rustdesk          # lấy overlay ports trong res/vcpkg
```

- **macOS**: `~/vcpkg/vcpkg install ffmpeg --triplet arm64-osx --overlay-ports=$HOME/rustdesk/res/vcpkg` (`x64-osx` trên Mac Intel), rồi `VCPKG_ROOT=~/vcpkg cargo build --release -p rcli --features gui,hwcodec`.
- **Linux**: cài thêm `clang`, `libva-dev`, `libdrm-dev`, `libx11-dev`, `cmake`, `ninja-build`, rồi `~/vcpkg/vcpkg install 'ffmpeg[amf,nvcodec]' --triplet x64-linux --overlay-ports=$HOME/rustdesk/res/vcpkg` (`arm64-linux` trên ARM, kèm `VCPKG_FORCE_SYSTEM_BINARIES=1`). FFmpeg nạp libva lúc chạy: máy không có libva, driver VA-API hoặc GPU thì dùng openh264, kể cả ở greeter và màn khoá.
- **Windows**: `vcpkg install ffmpeg[amf,nvcodec,qsv] --triplet x64-windows-static --overlay-ports=<rustdesk>\res\vcpkg`, rồi trong PowerShell `$env:VCPKG_ROOT='C:\vcpkg'; $env:RUSTFLAGS='-C target-feature=+crt-static'; cargo build --release -p rcli --features gui,hwcodec` (CRT tĩnh vì `hwcodec` và triplet `x64-windows-static` dùng `/MT`). FFmpeg của RustDesk không có Media Foundation (`h264_mf`).

## Package

```bash
VCPKG_ROOT=~/vcpkg packaging/macos/build-dmg.sh   # trên Mac: target/macos/rcli-<version>-macos-<arch>.dmg
packaging/linux/build-deb.sh                      # trên Linux, cần dpkg-dev: packaging/linux/out/rcli_<version>_<arch>.deb
packaging/linux/build-rpm.sh                      # trên Linux, cần rpm-build: packaging/linux/out/rcli-<version>-1.<arch>.rpm
```

Cả ba build với `gui,hwcodec`, nên cần `VCPKG_ROOT` như trên. App macOS chỉ được ký ad-hoc.

## Image của Server

`compose.yaml` kéo image đã phát hành, và vẫn giữ đường build:

```bash
docker compose build              # build ghcr.io/eigentlab/remote-cli-server và -dashboard từ cây mã này
docker compose up -d --build      # hoặc build rồi chạy luôn
```

Image nhận tag trong `RCLI_VERSION` (mặc định `latest`), nên đặt biến này để tách bản build từ mã nguồn khỏi bản đã phát hành.

## Bộ contract của platform

`rcli-contract` kiểm màn hình, input và accessibility của một Device thật trong từng phiên (đã đăng nhập, màn khoá, màn đăng nhập). Trên Linux, bằng root, qua service đang chạy:

```bash
cargo build --release -p rcli-agent --bin rcli-contract
sudo ./target/release/rcli-contract --session-socket /run/rcli/session.sock
```

Làm theo hướng dẫn trên terminal (focus một ô nhập, khoá, mở khoá, đăng xuất, đăng nhập); kết quả là một bảng Markdown, mọi ô ở cột "Logged in", "Lock screen" và "Login screen" phải là ✅. Không có `--session-socket` thì suite chạy trong chính phiên của người gọi. Trên Wayland cột "Login screen" sẽ lỗi, vì greeter của GDM không có portal.
