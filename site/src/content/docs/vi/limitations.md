---
title: Giới hạn đã biết
description: Những gì rcli 1.0.0 chưa làm, hoặc chỉ làm một phần.
sidebar:
  order: 5
---

## Nền tảng

- Bản release chỉ có cho Windows x86_64, macOS trên Apple silicon và Linux x86_64. Script cài Server chỉ chạy trên Linux x86_64; nơi khác dùng [cài thủ công](/vi/server/#cài-thủ-công).
- Bản dựng chưa ký: Windows SmartScreen và macOS Gatekeeper cảnh báo lần đầu mở.
- Giao diện Dashboard chỉ có tiếng Việt.

## Device

- User Mode không tới được màn khoá, màn đăng nhập hay UAC, không chạy trước khi đăng nhập, không chạy terminal dưới user khác hay root, và không gửi được input trên Wayland. Service Mode làm được.
- Màn đăng nhập macOS xem được nhưng không gõ được: Mac phải chịu được reboot thì cần bật tự đăng nhập.
- Greeter Wayland của GDM không chụp được; chuyển nó sang Xorg để điều khiển màn đăng nhập. Chữ gõ trên Wayland theo layout bàn phím US, và lần chụp đầu cần một lần phê duyệt ở Device.
- Trên Windows, Ctrl+Alt+Del cần bật một policy một lần trên mỗi Device, và lệnh terminal không có `--as root` lỗi khi Terminal User chưa đăng nhập.
- MCP chưa có tool `shell`; dùng `exec`.
- Wake cần một Agent Online khác trong cùng subnet với Device cần bật.

## Server

- Một Server, không có HA hay cluster.
- `rcli exec` không giới hạn kích thước output của lệnh, và `--timeout` dài (tối đa một giờ) giữ HTTP request mở suốt thời gian đó.
- Khoá đăng nhập tính theo User, không theo địa chỉ IP.
- Chưa có chức năng xoay master key hay thay CA. Mất master key là mất CA: mọi Device phải cài lại.
- User mất ứng dụng xác thực TOTP thì không tự tắt TOTP được; tạo User mới và vô hiệu hoá User cũ (xem [Quên mật khẩu](/vi/server/#quên-mật-khẩu)).
- Trên địa chỉ IP, trình duyệt cảnh báo chứng chỉ của Dashboard cho tới khi nhập chứng chỉ CA, và Device Windows được cài bằng `rcli.exe` tải trong trình duyệt thay vì lệnh PowerShell.
- Live View qua TURN relay mới được thử trên mạng đơn giản; NAT khó chưa được kiểm.
- Qua Cloudflare Tunnel, giới hạn kích thước request của Cloudflare (100 MB ở gói miễn phí) có thể chặn upload lớn, như Agent Build upload qua Dashboard.
- Không có lệnh gỡ Server, và script cài không đụng tới firewall: [Hướng dẫn Server](/vi/server/) có các lệnh cần chạy.
