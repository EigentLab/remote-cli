---
title: Hướng dẫn sử dụng
description: Điều khiển Device bằng CLI rcli, bằng AI agent qua MCP và bằng Dashboard.
sidebar:
  order: 1
---

rcli cho người, script và AI agent nhìn và điều khiển máy từ xa. Có ba cách điều khiển, cùng gọi một Controller API nên cùng quyền và cùng Control Lock:

- **`rcli`**: CLI, công cụ chính. Mọi lệnh có `--json` cho script và AI agent.
- **`rcli mcp`**: cùng các thao tác đó dưới dạng MCP tools cho Claude Code và các AI agent khác.
- **Dashboard** (trình duyệt): Address Book của bạn, Live View, web terminal, nút power/Wake và các trang quản lý Server. Giao diện Dashboard bằng tiếng Việt.

Từ ngữ (Device, Action, Observation, Mark, Control Lock, Wake Relay…) theo [GLOSSARY.md](https://github.com/EigentLab/remote-cli/blob/main/GLOSSARY.md). Cách dựng Server xem [Hướng dẫn Server](/vi/server/). Cách cài rcli lên một máy xem [Cài trên Device](/vi/devices/).

## Thử trên một máy

Không cần Server để thử rcli. Cài cho chính user của bạn rồi điều khiển máy này như **Local Device**:

```bash
rcli install                          # User Mode: không cần root, chạy khi đăng nhập; không có --server thì chỉ điều khiển cục bộ
rcli shot local --marks               # ảnh màn hình máy này, có đánh số Marks
rcli act local click --text "OK"      # click vào Mark có nhãn OK
```

`local` thay cho Device ID, đi qua Agent đang chạy trên máy này. Không cần Server, Device Password hay Control Lock. Chỉ Device Owner (OS user chạy Agent) hoặc root được dùng. Trên macOS, trước hết cấp cho rcli quyền Screen Recording và Accessibility (xem [macOS](/vi/devices/#macos)).

## Cài rcli và kết nối Device

Mỗi máy chỉ cài một lần: máy đó thành một Device và một Controller cùng lúc. Trang **Hướng dẫn kết nối** của Dashboard hiện sẵn các lệnh cài với Server Address và Server Key điền sẵn. [Cài trên Device](/vi/devices/) nói chi tiết từng OS.

```bash
sudo rcli install --server rcli.example.com:9443 --key rk_xxxxxxxx_yyyyyyyy   # một lần cho mỗi máy
rcli connect 123456789                                                        # hỏi Device Password một lần
```

`rcli install` cài Agent, đăng ký Device và in Device ID cùng Device Password. Lệnh cũng ghi cấu hình Controller: Server Address, Server Key, API Address và chứng chỉ CA của Server. Chạy bằng `sudo` (Service Mode) thì cấu hình dùng chung cho mọi user trên máy (`/etc/rcli/controller.json`, trên Windows `%ProgramData%\rcli\controller.json`): mọi user đọc được nhưng không sửa được, nên ai cũng chạy được `rcli connect` và `rcli mcp` mà không cài lại. Chạy không có `sudo` (User Mode) thì cấu hình là của riêng bạn.

Server Address là `host:port` của cổng Agent trên Server (mặc định 9443). Không cần nhập URL nào khác: Server gửi API Address (URL `https://` của Dashboard) lúc đăng ký và mỗi lần Agent kết nối. Server Key sai thì lệnh cài thất bại ngay và không để lại gì.

API Address phải là `https://`; `http://` chỉ được nhận khi trỏ tới chính máy này (`localhost`, cho test hoặc Server chạy cùng máy), không có cờ nào cho phép HTTP thường tới máy khác. Server chưa có API Address thì các lệnh Controller báo `the Server has no API Address yet`; đặt nó ở trang Server Settings hoặc bằng `rcli api-address set`.

`rcli connect` kiểm tra Device ID và Device Password, thêm Device vào Address Book của OS user (`--name` và `--tag`, lặp lại được, giúp phân biệt các Device) rồi nhớ mật khẩu trong keychain của OS: Keychain trên macOS, Credential Manager trên Windows, Secret Service trên Linux, hoặc file `0600` khi không có keychain. Các lệnh sau chỉ cần Device ID hoặc tên. Với `--no-remember`, mật khẩu không được nhớ và mỗi lệnh trên Device đó hỏi lại trên terminal. `rcli forget <Device ID>` xoá Device khỏi Address Book cùng mật khẩu đã nhớ.

`rcli uninstall` (bằng chính user đã cài) gỡ Device khỏi Server, gỡ Agent và cấu hình Controller. Device Password đã nhớ của từng user được giữ cho lần cài sau.

## Không có vai trò

Mọi User làm được mọi việc: điều khiển Device trong Address Book của mình và quản lý Server (Users, Server Key, Server Address, Agent Build, force-release Control Lock). Controller được làm gì trên một Device do Device Settings của chính Device đó quyết định (xem [Device Settings](/vi/devices/#device-settings)). Không User nào tự vô hiệu hoá chính mình hay vô hiệu hoá User cuối cùng còn hoạt động.

## Xem fleet

```bash
rcli devices                         # Address Book của bạn cùng Device Status (Online / Offline)
rcli device show demo-pc             # inventory, tài nguyên đọc từ Agent ngay lúc gọi, màn khoá / màn đăng nhập
rcli device processes demo-pc        # danh sách process lấy ngay lúc gọi
rcli displays demo-pc                # các màn hình: độ phân giải, scale, vị trí
```

Device nhận cả Device ID lẫn tên trong Address Book của bạn; hai Device trùng tên thì dùng Device ID. Server chỉ kết nối Controller với Device: không lưu lịch sử tài nguyên, inventory hay audit log, và không liệt kê Device nào bạn chưa thêm bằng Device ID và Device Password.

## Nhìn và điều khiển màn hình

**Nhìn** không cần Control Lock:

```bash
rcli shot demo-pc --out ./shots                 # ảnh màn hình (JPEG, cạnh dài 1280)
rcli shot demo-pc --display all --out ./shots   # một ảnh cho mỗi màn hình
rcli shot demo-pc --marks                       # đánh số Marks lên ảnh và in bảng Mark
```

Màn hình không đổi so với ảnh trước thì nhận `unchanged` thay vì ảnh mới.

Mỗi Mark có `number`, `text` (nhãn), `role`, `display` và khung. Container (`window`, `dialog`, `menu`, `list`) cũng là Mark: có số, được vẽ lên ảnh và nhắm được theo số hoặc chữ. Ba field nữa chỉ có khi không rỗng:

- `states`: các Mark State khác mặc định, trong `disabled`, `focused`, `checked`, `selected`, `expanded`, `collapsed`.
- `value`: nội dung (chữ trong ô, giá trị slider), tách khỏi nhãn `text`. Ô mật khẩu không bao giờ có `value`. Nhắm theo chữ chỉ khớp `text`.
- `parent`: số của Mark container gần nhất chứa nó, để phân biệt hai nút "OK" ở hai dialog.

Bảng Mark của CLI có thêm cột `PARENT`, `STATES`, `VALUE` (value dài bị cắt, `--json` có đủ). Mark từ OCR (màn khoá, không có cây accessibility) không có ba field này.

Trên macOS, Marks lấy từ cửa sổ đang focus, menu bar, menu đang mở và sheet của app đó; cửa sổ khác của app nằm phía sau không cho Mark. Ô nhập mà nhãn duy nhất là nội dung của nó có `text` rỗng, nội dung nằm ở `value`, nên nhắm ô đó theo số Mark. Chữ tĩnh vẫn lấy nội dung làm `text`.

**Wait Condition** chờ tới khi một chữ hiện ra hoặc biến mất rồi mới chụp, thay vì đoán thời gian chờ:

```bash
rcli shot demo-pc --until-appears "Saved" --marks          # chờ chữ "Saved"
rcli shot demo-pc --until-disappears "Loading" --timeout-ms 20000
rcli shot demo-pc --until-appears "OK" --role button       # chỉ tính Mark có role button
```

API: thêm `"until": {"appears": {"text": "Saved", "role": "button"}, "timeout_ms": 5000}` (hoặc `"disappears"`) vào request `actions` với observe `shot`. Agent đọc lại Marks khoảng mỗi 100 ms (OCR khi không có cây accessibility, nên dùng được ở màn khoá và màn đăng nhập) và trả Observation khi hai lần đọc liên tiếp đều thoả. Chữ khớp như khi nhắm theo chữ: không phân biệt hoa thường, bằng trước rồi mới tới chứa. `timeout_ms` mặc định 5000, tối đa 30000. Hết giờ thì lỗi `wait_timeout`, và object `error` vẫn có `images` (cùng `observation`, và `marks` nếu có xin) của lần đọc cuối. Chờ không cần Control Lock.

Cũng chờ được theo value hoặc Mark State của một Mark (Mark tìm theo chữ, thêm `--role` nếu cần):

```bash
rcli shot demo-pc --until-value "Name" --equals "An"             # value của ô Name đúng bằng "An"
rcli shot demo-pc --until-value "Status" --contains "xong"       # value chứa "xong"
rcli shot demo-pc --until-state "Remember me" --has checked      # checkbox đã được tick
rcli shot demo-pc --until-state "Save" --role button --lacks disabled   # nút Save đã bật
```

API: `"until": {"value": {"text": "Name", "role": "field", "equals": "An"}}` (hoặc `"contains"`), và `"until": {"state": {"text": "Remember me", "has": "checked"}}` (hoặc `"lacks"`), cùng `timeout_ms` như trên. So value có phân biệt hoa thường. Hai điều kiện này cần cây accessibility: màn chỉ đọc được bằng OCR (màn khoá, màn đăng nhập) thì lỗi ngay `no_accessibility_tree`. Chữ khớp nhiều Mark thì lỗi ngay `ambiguous_text` kèm danh sách Mark khớp; thêm `role` để chỉ còn một.

**Điều khiển** cần giữ Control Lock:

```bash
rcli lock acquire demo-pc
rcli act demo-pc click 640 360 --out ./shots          # toạ độ trong ảnh Observation, Agent tự đổi sang pixel thật
rcli act demo-pc click --text "OK"                    # click theo chữ trên màn hình
rcli act demo-pc click --mark 3 --observation <id>    # click theo số Mark của lần --marks trước
rcli act demo-pc type "xin chao" --observe none       # không cần ảnh trả về
rcli act demo-pc type "matkhau" --secret              # mật khẩu
rcli act demo-pc key ctrl+shift+t
rcli act demo-pc scroll 640 360 --dy 3
rcli act demo-pc set-value --mark 4 --observation <id> "matkhau"   # đặt thẳng giá trị ô, không gõ phím
rcli lock release demo-pc
```

Click, double click, right click vào một Mark lấy từ cây accessibility thì Agent thử accessibility action trước (bấm nút, focus ô nhập, mở item, mở menu ngữ cảnh), nên vẫn trúng khi phần tử bị che hay đã dời chỗ; không được thì mới dùng chuột giả lập. Toạ độ và Mark từ OCR luôn dùng chuột giả lập. `type` luôn gõ phím thật. `set-value` (API: `{"type":"set_value", TARGET, "value":".."}`) chỉ nhận Mark từ cây accessibility (macOS, Windows, Linux).

Chế độ Observation (`--observe`):

| Mode | Trả về | Dùng khi |
|---|---|---|
| `shot` (mặc định) | 1 ảnh, chờ màn hình đứng yên tối đa khoảng 1 s | hầu hết trường hợp |
| `none` | không ảnh | gõ liên tục, không cần nhìn |
| `frames` | chỉ các frame thay đổi rõ, có mốc thời gian (tối đa 10; field `max_frames` của API nhận 1–30) | theo dõi animation |
| `clip` | MP4 H.264 tối đa 5 s, lưu thành `clip.mp4` (chỉ `rcli act`, không có ở `rcli batch`) | người xem lại thao tác |

**Action Batch**: nhiều Action một lượt, một Observation:

```bash
echo '[{"type":"key","keys":"ctrl+a"},{"type":"wait","ms":200},{"type":"type","text":"hi"}]' \
  | rcli batch demo-pc - --observe frames --out ./shots
```

Bước `wait` nhận thêm Wait Condition (cùng dạng `until` ở trên), kiểm sau `ms`. Click Save, chờ "Saved", rồi chụp, trong một request:

```bash
echo '[{"type":"click","text":"Save"},{"type":"wait","until":{"appears":{"text":"Saved"},"timeout_ms":10000}}]' \
  | rcli batch demo-pc -
rcli act demo-pc wait --until-disappears "Loading" --timeout-ms 20000
```

Bước nào lỗi thì batch dừng ở đó và báo `step` (đếm từ 0); Wait Condition hết giờ thì báo `wait_timeout`, `step` và Observation của lần đọc cuối. Tổng thời gian chờ của một request (các bước `wait` cộng `timeout_ms` của Wait Condition) tối đa 30000 ms.

**Lỗi hay gặp:**

- `control_lock_held`: người khác đang giữ lock; lỗi kèm tên và giờ hết hạn. Chờ rồi thử lại, không có hàng đợi.
- `stale_mark`: số Mark từ Observation cũ; chụp lại với `--marks`.
- `ambiguous_text` / `text_not_found`: chữ khớp nhiều chỗ hoặc không khớp chỗ nào.
- `wait_timeout`: Wait Condition không thoả trong `timeout_ms`; lỗi kèm Observation của lần đọc cuối.
- `no_accessibility_tree`: Wait Condition theo value hoặc Mark State trên màn không có cây accessibility (chỉ có OCR).
- `agent_too_old`: Agent của Device cũ hơn 0.4.0, chưa hiểu Wait Condition. Cập nhật Agent rồi thử lại.

**Control Lock** tự hết sau 10 phút không có Action (mọi request của người giữ, kể cả chỉ chụp, đều gia hạn). `rcli mcp` và Dashboard còn mất lock sau 60 s nếu mất kết nối; CLI thường không giữ kết nối nên chỉ tính 10 phút. User nào cũng kết thúc được lock của người khác trên Device trong Address Book của mình bằng `rcli lock force-release demo-pc`.

## Màn khoá và màn đăng nhập

rcli không lưu mật khẩu OS ở đâu cả. Controller tự gõ mật khẩu bằng `type --secret`:

```bash
rcli lock acquire demo-pc
rcli act demo-pc type '...' --secret --observe none
rcli act demo-pc key enter --out ./shots
```

`rcli device show` cho biết `session`: `Active`, `LockScreen` hoặc `LoginScreen`. Màn khoá và màn đăng nhập chỉ điều khiển được ở Service Mode (xem [Cài trên Device](/vi/devices/)). Màn đăng nhập macOS chỉ xem được, không gõ được, nên Mac cần bật tự đăng nhập nếu phải chịu được reboot.

### Ctrl+Alt+Del trên Windows

Windows chỉ cho tiến trình SYSTEM gửi secure attention sequence (`SendSAS`), và chỉ sau khi bật một policy một lần trên mỗi Device (mặc định là tắt):

```powershell
# PowerShell as administrator, một lần:
Set-ItemProperty -Path 'HKLM:\SOFTWARE\Microsoft\Windows\CurrentVersion\Policies\System' `
  -Name SoftwareSASGeneration -Type DWord -Value 1
# 0 = tắt (mặc định), 1 = cho service (đủ cho rcli), 2 = cho service và ứng dụng
```

Rồi từ Controller: `rcli lock acquire <Device ID>` và `rcli act <Device ID> key ctrl+alt+del`. Không bật policy thì Windows bỏ qua `SendSAS` và không có gì xảy ra.

## Terminal

```bash
rcli exec demo-pc -- echo hello                     # in stdout/stderr, thoát với exit code của lệnh (124 nếu hết giờ)
rcli exec demo-pc --timeout 300 -- ./long-job.sh    # tính bằng giây, mặc định 60, tối đa 3600
rcli exec demo-pc --as root -- whoami               # chạy dưới root/SYSTEM nếu cài đặt root của Device cho phép
rcli shell demo-pc                                  # shell tương tác như SSH
rcli root on demo-pc                                # mặc định --as root cho exec và shell trên Device này
```

Terminal không cần Control Lock. Lệnh chạy dưới Terminal User của Device. `--as root` theo cài đặt root của Device: `never`, `ask` (người ở Device chấp nhận; mặc định) hoặc `always`. Trên Windows, lệnh không có `--as root` chạy trong phiên đã đăng nhập của Terminal User, và lỗi `terminal_user_not_signed_in` khi user đó chưa đăng nhập.

## Power và Wake

```bash
rcli power demo-pc lock         # khoá màn hình, không cần Control Lock
rcli power demo-pc restart      # restart, shutdown, sleep: bị từ chối khi người khác giữ Control Lock
rcli wake demo-pc               # Wake qua một Wake Relay; Device Online lại khi nó dậy
```

Address Book nhớ MAC và subnet của Device từ lần cuối thấy nó Online (CLI: lúc `rcli connect` và `rcli devices`; Dashboard: lúc Agent kết nối). Wake gửi chúng lên Server, Server chọn một Agent Online trong subnet đó làm Wake Relay. Device chưa từng Online kể từ khi thêm vào Address Book thì báo `no_wake_address`; không có Agent Online nào trong subnet thì báo `no_wake_relay`.

## Quản lý Server

Các lệnh này cần một phiên User: `rcli login <tên>` (mật khẩu Dashboard, thêm mã TOTP nếu User đã bật TOTP), `rcli logout` để kết thúc. Kết nối bằng Device Password thì không chạy được, chúng trả `user_session_required`.

```bash
rcli login an                                 # hỏi mật khẩu Dashboard (và mã TOTP), lưu phiên
rcli logout                                   # thu hồi và xoá phiên đã lưu
rcli user create an
rcli user set-password an --password '...'   # đặt mật khẩu Dashboard (TOTP của User giữ nguyên)
rcli user list
rcli user disable an
rcli server-key show                          # Server Key (rk_<CA fingerprint>_<secret>) cho bản cài mới
rcli server-key rotate                        # đổi secret: Device đã đăng ký vẫn Online, bản cài mới cần key mới
rcli server-address show                      # Server Address Agent dùng
rcli server-address set rcli.example.com:9443 # cấp lại chứng chỉ TLS của cổng Agent cho tên mới
rcli api-address show                         # API Address
rcli api-address set https://rcli.example.com # sửa API Address; Agent nhận ở lần kết nối sau
rcli build list                               # Agent Build, mới nhất trước, đánh dấu Latest Build
rcli build upload ./rcli-1.0.0-linux-x86_64 --version 1.0.0 --target linux-x86_64   # Server tự tính sha256
rcli build delete 1.0.0 linux-x86_64          # bỏ target thì xoá mọi target của phiên bản đó
rcli package publish rcli_1.0.0_amd64.deb --arch x86_64   # .dmg, .deb hoặc .rpm cho mục Tải xuống của Hướng dẫn kết nối
```

Cập nhật một máy lên Latest Build của OS và kiến trúc của nó (trên Device Service Mode thì chạy bằng `sudo` hoặc Administrator để Agent service cũng được restart):

```bash
sudo rcli update
```

Lệnh in phiên bản đang chạy, Latest Build, tiến độ tải và kết quả kiểm sha256, rồi restart Agent. Không bao giờ hạ phiên bản. `rcli upgrade` vẫn chạy như alias ẩn.

**Minimum Version** ở trang Server Settings: Device dưới mức này hiện "update needed", Controller dưới mức này in `Server recommends rcli ≥ <version>, run: rcli update`, và Agent dưới mức này tự cập nhật lên Latest Build. Thứ tự nâng cấp xem [Nâng cấp](/vi/server/#nâng-cấp).

## Dashboard

Mở URL `https://` của Dashboard (lệnh cài Server in ra) và đăng nhập bằng tên và mật khẩu, thêm mã TOTP nếu đã bật. Mỗi User tự bật hoặc tắt TOTP ở trang User: quét mã QR rồi nhập một mã để xác nhận. Sai 5 lần liên tiếp thì User bị khoá 15 phút.

- **Device**: Address Book của bạn (thêm Device bằng Device ID và Device Password) cùng Device Status. Menu `⋯` trên mỗi dòng đổi tên Device trong Address Book hoặc bỏ nó khỏi Address Book.
- **Trang một Device**: inventory, tài nguyên hiện tại (đọc từ Agent), màn khoá hoặc màn đăng nhập, người giữ Control Lock và giờ hết hạn, các nút Wake, Lock screen, Sleep, Restart, Shut down.
- **Live View**: xem màn hình trực tiếp qua WebRTC, chọn màn hình nếu Device có nhiều. Video ở độ phân giải gốc của màn hình; chọn Live View Quality trên thanh công cụ (Quality: nét nhất, Balanced: mặc định, Speed: mượt nhất), đổi giữa chừng không phải kết nối lại. Khi bạn giữ Control Lock thì click và gõ thẳng lên video. Người khác giữ thì vẫn xem được và thấy ai giữ đến lúc nào.
- **Web terminal**: ngay dưới Live View, có ô "as root".
- **Hướng dẫn kết nối**: Server Address, Server Key, các lệnh cài và mục Tải xuống.
- **Quản lý** (mọi User): User, Cập nhật Agent (upload và xoá Agent Build) và Server Settings (Server Key, Server Address, API Address, Minimum Version).

## Dùng với Claude Code (MCP)

```bash
rcli connect 123456789                        # trên máy đã cài rcli
claude mcp add rcli -- rcli mcp
```

`rcli mcp` dùng cấu hình của bản cài và Device Password đã nhớ, nên không cần biến môi trường hay bước thiết lập nào khác. Nếu `rcli` không nằm trong `PATH` mà Claude Code thấy, ghi đường dẫn đầy đủ.

Tools: `devices`, `device`, `lock_acquire`, `lock_release`, `lock_status`, `observe` (shot, frames, clip, Marks và Wait Condition `until`), `act`, `batch`, `exec`, `wake`, `power`. Ảnh trả về inline, bảng Mark nằm trong kết quả có cấu trúc. `device: "local"` điều khiển Local Device.

Ở màn khoá hoặc màn đăng nhập, AI agent tự gõ mật khẩu OS mà người dùng đưa cho nó bằng `type` có `"secret": true`; rcli không lưu mật khẩu này. MCP chưa có tool `shell`; dùng `exec`.
