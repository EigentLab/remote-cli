---
title: Cài trên Device
description: Cài rcli trên Windows, macOS và Linux, ở User Mode hoặc Service Mode, cập nhật và gỡ nó.
sidebar:
  order: 3
---

Mọi máy chạy cùng một binary `rcli`. Cài một lần là máy đó thành một Device (người khác điều khiển được nó) và một Controller (nó điều khiển được máy khác) cùng lúc.

## File phát hành

Mỗi [bản phát hành trên GitHub](https://github.com/EigentLab/remote-cli/releases) có:

| File | Dùng cho |
|---|---|
| `rcli-<version>-windows-x86_64.exe` | Windows x86_64 (lưu thành `rcli.exe`) |
| `rcli-<version>-macos-aarch64.dmg` | macOS trên Apple silicon: `rcli.app` |
| `rcli-<version>-macos-aarch64` | macOS trên Apple silicon: binary trần |
| `rcli_<version>_amd64.deb` | Debian và Ubuntu x86_64 |
| `rcli-<version>-linux-x86_64` | Linux x86_64: binary trần |
| `SHA256SUMS` | checksum của chúng: `sha256sum -c SHA256SUMS --ignore-missing` |

Ba binary trần cũng là thứ Server phát ra làm Agent Build (xem [Hướng dẫn Server](/vi/server/#cài-đặt)). Bản dựng chưa được ký, nên Windows và macOS cảnh báo lần đầu mở (xem [Windows](#windows) và [macOS](#macos)).

## User Mode và Service Mode

| | User Mode | Service Mode |
|---|---|---|
| Cài bằng | `rcli install`, bằng user của bạn | `sudo rcli install --server … --key …` (hộp thoại Administrator trên Windows) |
| Chạy | dưới user của bạn, khi bạn đã đăng nhập | dưới root hoặc SYSTEM, từ lúc khởi động |
| Màn khoá, màn đăng nhập, UAC | không | có |
| Terminal dưới user khác hoặc root | không | có |
| Input trên Wayland | không | có |
| Server | tuỳ chọn: không có thì chỉ điều khiển cục bộ | bắt buộc |
| Cấu hình Controller | của riêng bạn | dùng chung cho mọi user trên máy |

`rcli service install` chuyển bản cài User Mode sang Service Mode, `rcli service uninstall` chuyển ngược lại. Cả hai hỏi quyền root ngay tại Device và giữ nguyên Device ID, Device Password và Device Settings. Cửa sổ rcli (This Device → Install Mode) có cùng nút chuyển.

## Từ trang Hướng dẫn kết nối

Trang **Hướng dẫn kết nối** của Dashboard hiện các lệnh cài với địa chỉ và Server Key của Server điền sẵn. Chúng cài ở Service Mode và tải `rcli` từ Latest Build của Server cho đúng OS và kiến trúc của máy, nên Server phải có Agent Build trước.

- **Linux và macOS**: `curl -fsSL https://rcli.example.com/install.sh | sudo sh -s -- --key <Server Key>`. Với Server trên địa chỉ IP, lệnh ghi chứng chỉ CA của Server ra `rcli-ca.pem` trước rồi đưa cho `curl`; chép nguyên như hiện trên trang.
- **Windows**: trong PowerShell, `iwr https://rcli.example.com/install.ps1 -OutFile install.ps1; .\install.ps1 -Key <Server Key>` (hỏi UAC một lần). Với Server trên địa chỉ IP, trang không hiện lệnh PowerShell: tải `rcli.exe` từ trang trong trình duyệt (chấp nhận cảnh báo chứng chỉ, hoặc [nhập CA](/vi/server/#chứng-chỉ-ca) trước), mở nó rồi điền Server Address và Server Key.

Script dừng với `no rcli build for <os>-<arch> is rolled out` khi Server chưa có Agent Build cho target đó.

## Với binary có sẵn

```bash
sudo rcli install --server rcli.example.com:9443 --key rk_xxxxxxxx_yyyyyyyy   # Service Mode
rcli install --server rcli.example.com:9443 --key rk_xxxxxxxx_yyyyyyyy        # User Mode
rcli install                                                                  # User Mode, chỉ điều khiển cục bộ
```

Lệnh cài in ra:

```text
Device ID: 123456789
Device Password: abcdEFGH12
```

- `--name` đặt tên Device (mặc định: host name). `--terminal-user` đặt tài khoản chạy lệnh terminal (mặc định: tài khoản đã chạy `sudo`). `--allow-root` đặt cài đặt root thành `always` thay vì `ask`. `--key -` đọc Server Key từ stdin, để key không nằm trên command line.
- Server Key sai (sai secret hoặc sai fingerprint CA) thì lệnh cài thất bại ngay: không tạo Device, không cài gì. Agent cũng từ chối Server có CA không khớp fingerprint trong Server Key.
- `rcli agent info` (mọi user trên máy) cho biết Device ID và Agent đã kết nối chưa. `sudo rcli agent password show` hiện Device Password; ở User Mode, chạy không có `sudo` và gõ mật khẩu OS của bạn.
- Chạy lại `rcli install` thì giữ nguyên Device. Sau `rcli uninstall`, lần cài sau là một Device mới với Device ID và Device Password mới, không nằm trong Address Book của ai.
- Bản cài User Mode chưa có Server thì thêm sau bằng `rcli agent server set rcli.example.com:9443 --key <Server Key>`.

## Device Settings

Người ở Device quyết định Controller được làm gì trên đó. Ở Service Mode các lệnh này cần root; ở User Mode Device Owner tự chạy và gõ mật khẩu OS cho mọi thay đổi mở rộng quyền vào Device.

```bash
sudo rcli agent terminal-user set alice           # tài khoản chạy lệnh terminal và shell
sudo rcli agent root ask                          # --as root: never | ask (mặc định) | always
sudo rcli agent accept password+accept            # người ở Device chấp nhận từng kết nối mới (hoặc: password)
sudo rcli agent capability terminal off           # mouse-keyboard | terminal | power, on hoặc off
sudo rcli agent password set                      # Device Password mới, đọc từ stdin; phiên đang mở vẫn chạy
sudo rcli agent id set 123456789                  # một Device ID 9 chữ số khác, còn trống
sudo rcli agent general auto-update off           # không cài bản cập nhật Agent từ Server
rcli agent prompts                                # các yêu cầu đang chờ người ở Device
rcli agent disconnect                             # kết thúc mọi phiên và Control Lock của các Controller
```

Cửa sổ rcli (bản release) hiện cùng các cài đặt này, các Controller đang kết nối và các yêu cầu đang chờ.

## Windows

**Service Mode**, ba cách, cùng một kết quả:

1. Mở `rcli.exe`: cửa sổ rcli hỏi Server Address và Server Key; bấm Install và chấp nhận UAC một lần.
2. Lệnh PowerShell ở trang Hướng dẫn kết nối.
3. Trong PowerShell "Run as administrator", với binary đã tải: `.\rcli.exe install --server rcli.example.com:9443 --key <Server Key>`.

Agent chạy như Windows service `rcli-agent` (LocalSystem, tự chạy cùng Windows). Binary ở `%ProgramFiles%\rcli\rcli.exe`; dữ liệu Device (credentials, Device Settings, Secret Store) ở `%ProgramData%\rcli`, chỉ SYSTEM và Administrators đọc được. Server Key nằm trong `%ProgramData%\rcli\service.json`, không nằm trên command line của service. Secret Store dùng DPAPI machine scope, nên ổ đĩa hay bản backup chép sang máy khác không lộ Device Password. Log ở `%ProgramData%\rcli\agent.log`.

**User Mode**: `.\rcli.exe install` trong PowerShell thường đặt rcli vào `%LOCALAPPDATA%\Programs\rcli`, dữ liệu vào `%LOCALAPPDATA%\rcli`, và chạy nó mỗi lần bạn đăng nhập.

**Màn khoá, màn đăng nhập và UAC.** Service chạy trong session 0 và chạy một helper trong phiên console bằng token SYSTEM của chính nó, mở được desktop Winlogon và secure desktop của UAC. Vì vậy `shot` và `act` chạy được ở màn khoá, màn đăng nhập và hộp thoại UAC, và helper theo kịp đăng nhập, đăng xuất, chuyển user. Không cần quyền Screen Recording. OCR (`--text`, Wait Condition) ở màn khoá chạy dưới profile SYSTEM: nếu ở đó không đọc được chữ, cài gói ngôn ngữ OCR cho máy. Ctrl+Alt+Del xem [Ctrl+Alt+Del trên Windows](/vi/usage/#ctrlaltdel-trên-windows).

**Live View.** Bản build có feature `hwcodec` (xem [Build từ mã nguồn](/vi/build/)) mã hoá H.264 trên GPU bằng NVENC, AMF hoặc QSV; không có GPU encoder (máy ảo) hoặc không có feature thì Agent dùng openh264. Log Agent ghi tên encoder (`live stream …: H.264 through h264_nvenc` hoặc `… through openh264`).

**SmartScreen.** Bản dựng chưa ký, nên Windows có thể hiện "Windows protected your PC" khi mở `rcli.exe`: bấm More info rồi Run anyway.

**Gỡ cài đặt**: `rcli uninstall` trong PowerShell "Run as administrator" (Service Mode) hoặc PowerShell thường (User Mode). Lệnh gỡ Device khỏi Server (Server không tới được thì chỉ cảnh báo, bản cài cục bộ vẫn bị xoá), dừng và xoá service, xoá `%ProgramFiles%\rcli` và `%ProgramData%\rcli`, kể cả Secret Store.

## macOS

Mở file `.dmg`, kéo `rcli` vào Applications rồi mở nó. Bản dựng chưa ký: nếu macOS không cho mở, bấm chuột phải vào app rồi chọn Open, hoặc chạy `xattr -d com.apple.quarantine /Applications/rcli.app`. Cửa sổ rcli hỏi Server Address và Server Key; Install hỏi mật khẩu administrator và cài ở Service Mode. Lệnh `curl` ở trang Hướng dẫn kết nối làm cùng việc đó từ terminal. `rcli install` không có `sudo` cài ở User Mode: app vào `~/Applications/rcli.app`, `rcli` vào `~/.local/bin/rcli`.

**Quyền.** Cấp cho rcli quyền **Screen Recording** và **Accessibility** ở System Settings → Privacy & Security; cửa sổ rcli có nút mở thẳng hai mục này. Thiếu Screen Recording thì `rcli shot` báo `grant rcli Screen Recording in System Settings`; thiếu Accessibility thì không gửi được input và Marks chuyển sang OCR.

**Service Mode** chạy Agent như một LaunchDaemon root, có helper ở màn đăng nhập và trong phiên của từng user. Màn đăng nhập xem được nhưng không gõ được, nên Mac phải chịu được reboot thì cần bật tự đăng nhập. Log ở `/Library/Logs/rcli-agent.log` (User Mode: `~/Library/Logs/rcli-agent.log`).

**Gỡ cài đặt**: `sudo rcli uninstall` (Service Mode) hoặc `rcli uninstall` (User Mode). Lệnh gỡ Device khỏi Server, gỡ các launchd job, app và dữ liệu cục bộ của Device.

## Linux

**Yêu cầu:** systemd, và để có màn hình thì một phiên đồ hoạ **X11 (Xorg) hoặc Wayland** (GNOME, KDE, wlroots). X11 cần `xauth`. Wayland cần `xdg-desktop-portal` với một backend ScreenCast (`xdg-desktop-portal-gnome`, `-kde` hoặc `-wlr`) đang chạy trong phiên người dùng. Cài `tesseract-ocr` để có OCR. `systemd-creds` (có sẵn trong systemd hiện nay) bảo vệ Device Password bằng TPM nếu máy có.

### Service Mode

```bash
chmod +x rcli-1.0.0-linux-x86_64
sudo ./rcli-1.0.0-linux-x86_64 install --server rcli.example.com:9443 --key rk_<fingerprint>_<secret>
```

```text
installed rcli-agent.service and /usr/local/lib/rcli/rcli
Device ID: 123456789
Device Password: abcdEFGH12
```

Agent chạy như service root `rcli-agent.service`, và `rcli` được link ở `/usr/local/bin/rcli`. Service tìm phiên đồ hoạ trên seat0 (X11 hoặc Wayland, màn khoá, hoặc greeter của GDM, LightDM, SDDM) và chạy helper `rcli agent session` dưới đúng user của phiên đó, nên Controller xem và điều khiển được cả màn khoá lẫn màn đăng nhập, không chỉ phiên đã đăng nhập. Server Key nằm trong `/etc/rcli/agent.env` (mode `0600`, chỉ root đọc), không nằm trên command line của service. Terminal User mặc định là tài khoản đã chạy `sudo` (`SUDO_USER`).

Kiểm tra:

```bash
systemctl status rcli-agent.service --no-pager   # active (running)
rcli agent info                                  # Device ID và "connected"
sudo journalctl -u rcli-agent.service -n 30 --no-pager
```

Service được enable nên tự chạy lại sau khi reboot.

### User Mode

`rcli install` không có `sudo` đặt rcli vào `~/.local/lib/rcli/rcli`, link ở `~/.local/bin/rcli` (thêm thư mục này vào `PATH`), và chạy nó khi đăng nhập qua systemd user unit `~/.config/systemd/user/rcli-agent.service`.

### Package .deb

Package cài `/usr/bin/rcli`, và hook cài đặt của nó chạy `rcli install` ở Service Mode với Server Address và Server Key lấy từ `/etc/rcli/agent.conf`. Viết file đó trước:

```bash
sudo install -d /etc/rcli
sudo install -m 600 /dev/null /etc/rcli/agent.conf
sudo tee /etc/rcli/agent.conf >/dev/null <<'CONF'
RCLI_SERVER=rcli.example.com:9443
RCLI_SERVER_KEY=rk_<fingerprint>_<secret>
# tuỳ chọn:
# RCLI_TERMINAL_USER=alice
# RCLI_ALLOW_ROOT=1
CONF
sudo apt install ./rcli_1.0.0_amd64.deb
systemctl status rcli-agent.service --no-pager
```

Chưa có `/etc/rcli/agent.conf` thì package vẫn cài và in hướng dẫn; sau đó tự chạy bước cài:

```bash
sudo rcli install --server "$(sudo sed -n 's/^RCLI_SERVER=//p' /etc/rcli/agent.conf)" \
     --key "$(sudo sed -n 's/^RCLI_SERVER_KEY=//p' /etc/rcli/agent.conf)"
```

Gỡ package thì hook chạy `rcli uninstall` trước, nên Device bị xoá khỏi Server và dữ liệu cục bộ bị xoá:

```bash
sudo apt remove rcli
sudo apt purge rcli         # xoá luôn /etc/rcli/agent.conf
```

### Wayland

Phiên người dùng Wayland (GNOME, KDE, Sway, Hyprland) điều khiển được mà không cần chuyển sang Xorg. Chụp màn hình đi qua XDG desktop portal (`org.freedesktop.portal.ScreenCast`) và PipeWire; input do service root ghi vào `/dev/uinput`, nên input trên Wayland cần Service Mode.

- **Phê duyệt một lần:** lần đầu Agent chụp màn hình, một hộp thoại chia sẻ màn hình hiện ở Device. Chọn màn hình (chọn nhiều được) rồi bấm Share. Portal trả về một restore token, Agent lưu ở `/var/lib/rcli/screencast.restore_token` (chỉ root đọc), nên không hỏi lại, kể cả sau reboot.
- **Không có portal:** `rcli shot` báo "screen capture on Wayland needs the XDG desktop portal's ScreenCast". Cài `xdg-desktop-portal` và một backend, rồi `systemctl --user restart xdg-desktop-portal` và restart `rcli-agent.service`.
- **Không có `/dev/uinput`:** input báo lỗi rõ ràng. Nạp module bằng `sudo modprobe uinput`, và thêm `uinput` vào `/etc/modules-load.d/uinput.conf` để còn sau reboot.
- **Nhiều màn hình:** chỉ các màn hình đã chọn trong hộp thoại portal mới chụp được, và `rcli displays` liệt kê đúng chúng.
- **Bàn phím:** chữ được gõ theo layout US (mã phím evdev); layout khác chỉ đúng với phím đơn. Phím có tên (`enter`, `ctrl`, `f5`…) dùng mã evdev.
- **Màn khoá:** vẫn là phiên Wayland đang chạy nên dùng cùng stream. Nếu compositor dừng stream khi khoá, `rcli shot` báo "the screen cast stream sent no frame".
- **Greeter Wayland của GDM** không có portal, nên `rcli shot` báo "screen capture at a Wayland login screen is not available". Muốn điều khiển màn đăng nhập, đặt `WaylandEnable=false` trong `/etc/gdm3/custom.conf` rồi `sudo systemctl restart gdm`: greeter khi đó chạy Xorg.

### Máy chủ không màn hình

Cùng lệnh `sudo rcli install --server … --key …` chạy được khi không có màn hình: service vẫn chạy và tự kết nối lại sau reboot, `rcli device show`, `rcli exec`, `rcli shell` và `rcli power` hoạt động. `rcli shot`, `rcli act` và Live View báo "no X11 session on the console".

## Cập nhật

- `sudo rcli update` (`rcli update` ở User Mode) thay binary bằng Latest Build của Server sau khi kiểm sha256, rồi restart Agent. Không bao giờ hạ phiên bản.
- Với `.deb`: `sudo apt install ./rcli_<version>_amd64.deb` cài binary mới và restart `rcli-agent.service`; Device ID giữ nguyên.
- Agent thấp hơn Minimum Version của Server tự cập nhật (xem [Nâng cấp](/vi/server/#nâng-cấp)).

## Gỡ cài đặt

`rcli uninstall`, bằng chính user đã cài (`sudo` với Service Mode, Administrator trên Windows). Lệnh gỡ Device khỏi Server, gỡ Agent và cấu hình Controller, và xoá dữ liệu cục bộ của Device. Device Password đã nhớ của từng user được giữ lại.
