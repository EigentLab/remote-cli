---
title: Hướng dẫn Server
description: Cài, vận hành, sao lưu, nâng cấp và gỡ một rcli Server.
sidebar:
  order: 2
---

Server kết nối Controller với Device. Nó chạy thành một project Docker Compose: Server, Postgres, kho file (SeaweedFS), Dashboard và coturn (TURN relay cho Live View), thêm Caddy hoặc cloudflared tuỳ chế độ. Server và `rcli` dùng chung một số phiên bản: Server và các Device của nó nên chạy cùng phiên bản.

## Cài đặt

Trên một máy chủ Linux x86_64 (Debian và Ubuntu là mục tiêu đầu tiên), chạy:

```bash
curl -fsSL https://rcli.eigentlab.com/install.sh | sudo sh
```

Script này cũng nằm trong repo: `https://raw.githubusercontent.com/EigentLab/remote-cli/main/install.sh`.

Script không hỏi gì. Nó cài Docker bằng script của Docker nếu máy chưa có, ghi mọi thứ vào `/opt/rcli/`, sinh mọi secret vào `/opt/rcli/.env`, kéo image Server và Dashboard đúng phiên bản của nó từ `ghcr.io/eigentlab/`, khởi động chúng và tạo User đầu tiên `admin` với mật khẩu ngẫu nhiên. Cuối cùng nó in ra:

```text
rcli 1.0.1 is running.

  Dashboard:       https://203.0.113.10:3443
  CA fingerprint:  4f1c…
  Name:            admin
  Password:        …   (shown once and not saved: write it down)

The browser warns about the certificate until you import the Server's CA, https://203.0.113.10:3443/api/v1/server-ca
(check its SHA-256 fingerprint against the one above).
Open these ports: 3443/tcp (Dashboard), 9443/tcp (Agents), 3478/tcp and 3478/udp, 49160-49200/udp (TURN relay).
Back up /opt/rcli/.env: its RCLI_MASTER_KEY seals the Server's CA, and the database cannot be used without it.
```

Mật khẩu chỉ in một lần và không lưu ở đâu. Trên OS hoặc kiến trúc khác, script dừng trước khi thay đổi gì; khi đó làm theo [Cài thủ công](#cài-thủ-công).

Sau đó:

1. Mở các [cổng](#cổng) mà script nêu.
2. Đăng nhập Dashboard bằng `admin` và đổi mật khẩu ở trang User (hoặc tạo User của riêng bạn ở đó rồi vô hiệu hoá `admin`).
3. Upload các file `rcli` của [bản phát hành trên GitHub](https://github.com/EigentLab/remote-cli/releases) ở trang **Cập nhật Agent**. File tên kiểu `rcli-1.0.1-linux-x86_64` tự điền phiên bản và target. Device mới tải `rcli` từ các Agent Build này, nên lệnh cài ở trang Hướng dẫn kết nối chỉ chạy được sau bước này. Package `.dmg` và `.deb` upload bằng `rcli package publish` (xem [Quản lý Server](/vi/usage/#quản-lý-server)).
4. Thêm Device: xem [Cài trên Device](/vi/devices/).

## Năm chế độ

Mọi chế độ đều phục vụ Dashboard qua HTTPS; không chế độ nào phục vụ HTTP thường ra mạng. Ở mọi chế độ, bản thân Dashboard chỉ nghe trên `127.0.0.1:3080`, sau proxy.

| Lệnh | Dashboard | Chứng chỉ |
|---|---|---|
| không tham số, hoặc `--ip <IPv4>` và/hoặc `--port <cổng>` | `https://<IP>:3443` | do CA riêng của Server cấp |
| `--domain <tên>` | `https://<tên>` (cổng 80 và 443) | Let's Encrypt |
| `--domain <tên> --cert <file> --key <file> [--port <cổng>]` | `https://<tên>:8443` | của bạn |
| `--tunnel <token>` | hostname công khai của tunnel | của Cloudflare |
| `--behind-proxy` | URL của proxy của bạn | của proxy của bạn |

Tham số đặt sau `sh -s --`, ví dụ:

```bash
curl -fsSL https://rcli.eigentlab.com/install.sh | sudo sh -s -- --domain rcli.example.com
```

### Địa chỉ IP (mặc định)

Cho máy chủ không có tên miền. Script tự tìm địa chỉ IPv4 của máy và in ra; sau NAT thì đưa địa chỉ công khai bằng `--ip`. Cổng là 3443, hoặc `--port`. Cổng 80 và 443 để trống cho phần mềm khác.

```bash
curl -fsSL https://rcli.eigentlab.com/install.sh | sudo sh -s -- --ip 203.0.113.10 --port 3443
```

Mỗi lần khởi động, Server cấp từ CA của nó một chứng chỉ cho địa chỉ đó (hạn 800 ngày) và Caddy đi kèm phục vụ chứng chỉ này. Mọi bản cài rcli vốn đã tin CA này: Server Key mang fingerprint của nó, và bản cài giữ chứng chỉ CA sau khi kiểm. Vì vậy `rcli` và `rcli mcp` kết nối qua HTTPS đã xác thực, và lệnh cài Linux/macOS ở trang Hướng dẫn kết nối mang theo chứng chỉ CA cho `curl`. Trình duyệt không biết CA này: chúng cảnh báo chứng chỉ cho tới khi bạn [nhập chứng chỉ CA](#chứng-chỉ-ca). Với Windows, trang Hướng dẫn kết nối bảo tải `rcli.exe` trong trình duyệt (chấp nhận cảnh báo, hoặc nhập CA trước) rồi mở nó.

Nếu đổi API Address ở trang Server Settings, restart cả Server lẫn Caddy để chứng chỉ theo địa chỉ mới: `cd /opt/rcli && docker compose restart server caddy` (bằng root; Caddy chỉ đọc chứng chỉ lúc khởi động).

### Tên miền

```bash
curl -fsSL https://rcli.eigentlab.com/install.sh | sudo sh -s -- --domain rcli.example.com
```

Caddy đi kèm giữ cổng 80 và 443 và lấy chứng chỉ Let's Encrypt khi `rcli.example.com` đã trỏ về máy này và cả hai cổng đã mở. Agent và TURN dùng cùng tên đó.

### Chứng chỉ của bạn

```bash
curl -fsSL https://rcli.eigentlab.com/install.sh | sudo sh -s -- \
  --domain rcli.example.com --cert /etc/ssl/rcli/fullchain.pem --key /etc/ssl/rcli/privkey.pem --port 8443
```

Caddy phục vụ chứng chỉ của bạn trên cổng 8443 (hoặc `--port`). Script chép hai file vào `/opt/rcli/tls/`, nên sau khi gia hạn, chạy lại lệnh không tham số: nó chép lại từ đúng các đường dẫn cũ và restart Caddy.

### Cloudflare Tunnel

```bash
curl -fsSL https://rcli.eigentlab.com/install.sh | sudo sh -s -- --tunnel <tunnel token>
```

`cloudflared` đi kèm kết nối tunnel, nên máy không mở cổng nào cho Dashboard. Trong Cloudflare dashboard, tạo tunnel, chép token, và trỏ hostname công khai của nó tới `http://dashboard:3000`. Agent và TURN vẫn tới máy trực tiếp qua địa chỉ IP (tự tìm, hoặc `--ip`), nên cổng của chúng phải mở. API Address được ghi lại lần đầu một User đăng nhập qua hostname đó bằng `https://`; kiểm lại ở trang Server Settings.

### Sau reverse proxy của bạn

```bash
curl -fsSL https://rcli.eigentlab.com/install.sh | sudo sh -s -- --behind-proxy
```

Không có gì đi kèm. Trỏ proxy của bạn tới `http://127.0.0.1:3080`. Proxy phải:

- thêm địa chỉ client vào `X-Forwarded-For`, đặt `X-Forwarded-Proto: https`, và chuyển nguyên `Host` mà trình duyệt dùng;
- không buffer hay nén response: shell và Control Lock là luồng stream;
- cho request body tới 200 MB (Agent Build) và response kéo dài tới một giờ (`rcli exec --timeout 3600`).

Caddy làm sẵn hai điều đầu:

```text
rcli.example.com {
	reverse_proxy 127.0.0.1:3080 {
		flush_interval -1
	}
}
```

Với nginx:

```nginx
location / {
    proxy_pass http://127.0.0.1:3080;
    proxy_set_header Host $host;
    proxy_set_header X-Forwarded-For $proxy_add_x_forwarded_for;
    proxy_set_header X-Forwarded-Proto https;
    proxy_buffering off;
    proxy_request_buffering off;
    proxy_read_timeout 3600s;
    client_max_body_size 200m;
    gzip off;
}
```

`--ip` nêu địa chỉ mà Agent và TURN dùng, như với `--tunnel`. API Address được ghi lại ở lần đăng nhập `https://` đầu tiên qua proxy.

### Đổi chế độ và chạy lại

Chạy lại lệnh với chế độ khác để đổi: secret, dữ liệu, User và Device đều giữ nguyên, nhưng lần đổi làm Server Address thay đổi (xem bên dưới) khiến các Agent đã cài offline cho tới khi được trỏ sang địa chỉ mới, và script sẽ báo điều đó. Chạy không tham số thì giữ chế độ đang cài. Lần chạy bị từ chối tham số thì không thay đổi gì. Lần chạy sau không bao giờ sinh lại secret và không tạo User khi đã có User đăng nhập (output khi đó ghi `Users: kept`); trước đó, mỗi lần chạy đặt lại password của `admin` và in ra.

Chỉ `--ip` hoặc chỉ `--port` không bao giờ chuyển Server đã cài sang chế độ địa chỉ IP: đưa cả hai để chuyển. Ở chế độ chứng chỉ, chỉ `--port` thì đổi cổng của nó. Lần chạy đưa địa chỉ khác (`--ip` khác, hoặc chuyển sang hay rời tên miền) đổi luôn Server Address; Agent đã cài giữ địa chỉ cũ cho tới khi `sudo rcli agent server set <host>:<cổng> --key <Server Key>` trỏ nó sang địa chỉ mới.

## Cổng

| Cổng | Giao thức | Dùng cho |
|---|---|---|
| 3443 (chế độ IP), 80 và 443 (`--domain`), 8443 (`--cert`) | TCP | Dashboard và Controller API |
| 9443 | TCP | Agent (TLS có chứng chỉ client); không bao giờ đặt proxy trước cổng này |
| 3478 | TCP và UDP | TURN/STUN (coturn) |
| 49160–49200 | UDP | dải cổng relay của coturn |

Script không đụng tới firewall. Với `ufw`, ở chế độ IP:

```bash
sudo ufw allow 3443/tcp
sudo ufw allow 9443/tcp
sudo ufw allow 3478
sudo ufw allow 49160:49200/udp
```

Docker tự mở trong iptables các cổng nó publish (cổng Agent, và cổng Dashboard của Caddy đi kèm), vượt qua `ufw`; coturn chạy trên mạng của host nên `ufw` có tác dụng với nó. Firewall hoặc security group của nhà cung cấp cloud phải cho phép tất cả các cổng này.

Controller API (cổng 8080 trong mạng Docker) không bao giờ được publish: `rcli` và `rcli mcp` tới nó qua URL của Dashboard.

**Đổi cổng Agent.** Nếu 9443 đã bị dùng, thêm `RCLI_AGENT_PORT=<cổng>` vào `/opt/rcli/.env`, chạy `docker compose up -d` trong `/opt/rcli` (bằng root), rồi đặt Server Address thành `<host>:<cổng>` ở trang Server Settings hoặc bằng `rcli server-address set`. Làm việc này trước khi thêm Device: Agent đã cài giữ địa chỉ lúc cài, cho tới khi `sudo rcli agent server set <host>:<cổng> --key <Server Key>` trỏ nó sang địa chỉ mới. `RCLI_DASHBOARD_PORT` đặt cổng host của Dashboard. Script cài ghi `127.0.0.1:3080` khi chưa đặt, và giữ cả hai cổng khi bạn đã đặt chúng trong `/opt/rcli/.env`.

## Chứng chỉ CA

Mỗi Server có CA riêng. CA ký chứng chỉ cho Agent, Server Key mang fingerprint của nó, và ở chế độ IP nó ký chứng chỉ của Dashboard. Lệnh cài in fingerprint (`CA fingerprint`); chứng chỉ nằm ở `<URL Dashboard>/api/v1/server-ca`.

Tải về và kiểm fingerprint trước khi tin nó (`-k` vì chứng chỉ phía trình duyệt chưa được tin; chính bước kiểm fingerprint chứng minh đây là CA của bạn):

```bash
curl -fsSk https://203.0.113.10:3443/api/v1/server-ca -o rcli-ca.pem
openssl x509 -in rcli-ca.pem -outform der | sha256sum      # macOS: shasum -a 256
```

Kết quả phải bằng `CA fingerprint` mà lệnh cài đã in. Rồi nhập nó để trình duyệt hết cảnh báo:

- **macOS**: `sudo security add-trusted-cert -d -r trustRoot -k /Library/Keychains/System.keychain rcli-ca.pem` (Safari và Chrome dùng).
- **Windows**: trong cửa sổ lệnh administrator, `certutil -addstore -f Root rcli-ca.pem` (Edge và Chrome dùng).
- **Debian và Ubuntu**: `sudo cp rcli-ca.pem /usr/local/share/ca-certificates/rcli-ca.crt && sudo update-ca-certificates`. Chrome trên Linux có kho riêng: Settings → Privacy and security → Security → Manage certificates → Authorities → Import.
- **Firefox**, trên mọi OS: Settings → Privacy & Security → Certificates → View Certificates → Authorities → Import, rồi tick "Trust this CA to identify websites".

CA đã nhập có thể bảo lãnh cho bất kỳ website nào, nên chỉ nhập CA của Server do chính bạn chạy.

## Sao lưu

`/opt/rcli` và file `.env` thuộc root, nên các lệnh dưới đây chạy bằng root (`sudo -i`). Sao lưu ba thứ:

- **`/opt/rcli/.env`**, một lần. `RCLI_MASTER_KEY` trong đó mã hoá private key của CA và TOTP secret của các User trong database. Mất nó thì Server không khởi động được trên database đó, và CA mất theo: mọi Device phải cài lại với Server Key mới. Giữ file này ở nơi an toàn: cùng với một bản dump database, nó là toàn bộ Server.
- **Database**, thường xuyên.
- **File** (Agent Build và package) trong volume `rcli_filedata`. Chúng cũng upload lại được từ bản phát hành.

```bash
cd /opt/rcli
cp .env /root/rcli.env
docker compose exec -T postgres pg_dump -U rcli -Fc rcli > /root/rcli-db.dump
docker run --rm -v rcli_filedata:/data -v /root:/backup alpine tar czf /backup/rcli-files.tgz -C /data .
```

Rồi chép ba file ra khỏi máy.

Khôi phục trên máy mới: đặt ba file vào `/root`, rồi bằng root:

```bash
mkdir -p /opt/rcli && install -m 600 /root/rcli.env /opt/rcli/.env
curl -fsSL https://rcli.eigentlab.com/install.sh | sh     # cùng phiên bản; giữ .env (bỏ qua User nó tạo)
cd /opt/rcli
docker compose stop server dashboard storage
docker compose exec -T postgres pg_restore -U rcli -d rcli --clean --if-exists < /root/rcli-db.dump
docker run --rm -v rcli_filedata:/data -v /root:/backup alpine tar xzf /backup/rcli-files.tgz -C /data
curl -fsSL https://rcli.eigentlab.com/install.sh | sh     # khởi động lại mọi thứ
```

Device tới Server qua Server Address, nên giữ cho máy mới cùng địa chỉ IP hoặc tên miền (ở lần chạy đầu, đưa tham số của chế độ khi chúng đổi, như `--ip`), hoặc trỏ từng Device sang địa chỉ mới bằng `sudo rcli agent server set`.

## Quên mật khẩu

Nếu còn User khác đăng nhập được, họ đặt mật khẩu mới ở trang User hoặc bằng `rcli user set-password <tên> --password '<mật khẩu mới>'`.

Nếu không ai đăng nhập được, dùng bootstrap token trong `/opt/rcli/.env`. Token có hiệu lực lại khi không còn User nào đang bật có phiên còn hạn; một phiên kéo dài 12 giờ sau khi đăng nhập (`rcli logout` kết thúc sớm hơn). Trên Server, bằng root:

```bash
cd /opt/rcli
TOKEN=$(sed -n 's/^RCLI_BOOTSTRAP_ADMIN_TOKEN=//p' .env)
curl -fsS -H "Authorization: Bearer $TOKEN" -H 'Content-Type: application/json' \
  -d '{"password":"<mật khẩu mới>"}' http://127.0.0.1:3080/api/v1/users/admin/credentials
```

Thay `admin` bằng tên của User. Cài đặt TOTP của User giữ nguyên. Không muốn chờ phiên hết hạn thì kết thúc tất cả trước:

```bash
docker compose exec postgres psql -U rcli rcli -c "UPDATE api_tokens SET revoked_at = now() WHERE revoked_at IS NULL"
```

User mất ứng dụng xác thực TOTP thì không tự tắt TOTP được. Tạo User mới theo cách trên (`-d '{"name":"<tên>"}'` tới `http://127.0.0.1:3080/api/v1/users`, rồi đặt mật khẩu như trên), đăng nhập bằng User đó và vô hiệu hoá User cũ. Sai 5 lần đăng nhập liên tiếp thì User bị khoá 15 phút.

## Nâng cấp

Nâng Server trước, rồi tới các Device:

1. **Server.** Sao lưu, rồi chạy lại lệnh cài của phiên bản mới (`curl -fsSL https://rcli.eigentlab.com/install.sh | sudo sh`). Nó kéo image của phiên bản đó và khởi động lại, giữ chế độ, `.env`, dữ liệu và User.
2. **Agent Build.** Upload các file `rcli` của phiên bản mới ở trang Cập nhật Agent (hoặc bằng `rcli build upload`), và package mới bằng `rcli package publish`. Phiên bản cao nhất của mỗi target là Latest Build.
3. **Device.** Hoặc chạy `sudo rcli update` trên từng máy (`rcli update` ở User Mode), hoặc đặt **Minimum Version** ở trang Server Settings bằng phiên bản mới. Khi đó mọi Agent đang kết nối mà thấp hơn Minimum Version tải Latest Build, kiểm sha256 và khởi động lại; Agent nào lỗi thì thử lại mỗi giờ, và Device Offline cập nhật khi kết nối lại. Device Owner tắt được việc này bằng `rcli agent general auto-update off`.

Thứ tự này quan trọng vì Device chỉ cập nhật lên Latest Build mà Server đang giữ: Server chỉ gửi bản cập nhật cho Agent khi Agent thấp hơn Minimum Version và Latest Build cao hơn phiên bản của Agent. Đặt Minimum Version trước khi upload bản mới thì không Device nào được cập nhật. Khi có Minimum Version, Device dưới mức đó hiện "update needed" và Controller dưới mức đó in `Server recommends rcli ≥ <version>, run: rcli update`.

## Gỡ cài đặt

Bằng root:

```bash
cd /opt/rcli
docker compose down -v        # dừng rcli và xoá các volume: database, file, chứng chỉ
cd / && rm -rf /opt/rcli      # file compose, file proxy và .env (cùng master key)
docker image rm ghcr.io/eigentlab/remote-cli-server:1.0.1 ghcr.io/eigentlab/remote-cli-dashboard:1.0.1
```

Docker vẫn được giữ. Đóng các cổng đã mở (`sudo ufw delete allow 9443/tcp`, v.v.). Các Device còn cài vẫn cố kết nối: chạy `rcli uninstall` trên từng máy (lệnh cảnh báo khi không tới được Server và vẫn gỡ bản cài cục bộ).

## Cài thủ công

Dùng khi script cài không phù hợp: OS hoặc kiến trúc khác, cấu hình Compose của riêng bạn, hoặc image build từ mã nguồn. Cần Docker với Compose v2 và một reverse proxy TLS.

1. Lấy file compose của phiên bản bạn chạy:

   ```bash
   mkdir rcli && cd rcli
   curl -fsSLO https://raw.githubusercontent.com/EigentLab/remote-cli/v1.0.1/compose.yaml
   ```

2. Viết `.env` cạnh nó:

   ```bash
   cat > .env <<EOF
   RCLI_VERSION=1.0.1
   RCLI_BOOTSTRAP_ADMIN_TOKEN=$(openssl rand -hex 16)
   RCLI_MASTER_KEY=$(openssl rand -hex 32)
   RCLI_TURN_SECRET=$(openssl rand -hex 16)
   RCLI_S3_ACCESS_KEY=$(openssl rand -hex 16)
   RCLI_S3_SECRET_KEY=$(openssl rand -hex 32)
   POSTGRES_PASSWORD=$(openssl rand -hex 16)
   RCLI_TURN_HOST=rcli.example.com
   RCLI_AGENT_HOSTNAMES=rcli.example.com:9443
   RCLI_API_ADDRESS=https://rcli.example.com
   RCLI_DASHBOARD_PORT=127.0.0.1:3080
   EOF
   chmod 600 .env
   ```

3. Khởi động, rồi đặt proxy TLS trước cổng Dashboard như ở [Sau reverse proxy của bạn](#sau-reverse-proxy-của-bạn):

   ```bash
   docker compose up -d                  # kéo image; `docker compose up -d --build` build từ mã nguồn
   docker compose ps                     # postgres, storage, server, dashboard và coturn đang chạy
   ```

4. Tạo User đầu tiên bằng bootstrap token, trên chính máy đó:

   ```bash
   TOKEN=$(sed -n 's/^RCLI_BOOTSTRAP_ADMIN_TOKEN=//p' .env)
   curl -fsS -H "Authorization: Bearer $TOKEN" -H 'Content-Type: application/json' \
     -d '{"name":"admin"}' http://127.0.0.1:3080/api/v1/users
   curl -fsS -H "Authorization: Bearer $TOKEN" -H 'Content-Type: application/json' \
     -d '{"password":"<mật khẩu>"}' http://127.0.0.1:3080/api/v1/users/admin/credentials
   ```

   Bootstrap token hết tác dụng ngay khi có User đăng nhập (xem [Quên mật khẩu](#quên-mật-khẩu)).

| Biến | Bắt buộc | Ý nghĩa |
|---|---|---|
| `RCLI_BOOTSTRAP_ADMIN_TOKEN` | có | Đóng vai một User khi không có User nào đang bật có phiên còn hạn: để tạo User đầu tiên và để cứu hộ. |
| `RCLI_MASTER_KEY` | rất nên đặt | 64 ký tự hex. Mã hoá private key của CA và TOTP secret. Không có key thì chúng nằm dạng plaintext trong Postgres (Server log cảnh báo); có key thì chúng được mã hoá ở lần khởi động sau. Đã mã hoá rồi thì thiếu key Server không khởi động, và key khác không mở được. Chưa có chức năng xoay key. |
| `RCLI_TURN_SECRET` | có | Secret chung giữa Server và coturn để cấp credential TURN. |
| `RCLI_TURN_HOST` | nên đặt | Tên hoặc IP công khai mà trình duyệt và Agent dùng để tới coturn. Mặc định `localhost`. |
| `RCLI_AGENT_HOSTNAMES` | không | Server Address (`host:port`, cách nhau bằng dấu phẩy; đưa vào chứng chỉ của cổng Agent). Áp dụng lúc khởi động nếu đã đổi so với lần trước, nên thay đổi ở trang Server Settings hoặc bằng `rcli server-address set` giữ nguyên tới lúc đó. Mặc định `localhost`. |
| `RCLI_API_ADDRESS` | không | URL `https://` của Dashboard. Áp dụng lúc khởi động nếu đã đổi so với lần trước. Rỗng: ghi lại từ lần đăng nhập `https://` đầu tiên của một User. Server không khởi động nếu nó không phải một origin `https://`. |
| `RCLI_S3_ACCESS_KEY`, `RCLI_S3_SECRET_KEY` | có | Credential của service `storage`; Compose đặt cùng giá trị cho `storage` và Server. |
| `RCLI_S3_ENDPOINT`, `RCLI_S3_BUCKET` | không | Mặc định `http://storage:8333` và `rcli`. Server tự tạo bucket nếu chưa có. |
| `POSTGRES_PASSWORD` | nên đặt | Mặc định `rcli`. |
| `RCLI_VERSION` | không | Tag image cần kéo. Mặc định `latest`. |
| `RCLI_AGENT_PORT` | không | Cổng trên host cho Agent kết nối. Mặc định `9443`. Server Address phải dùng cổng này. |
| `RCLI_DASHBOARD_PORT` | không | Cổng trên host của Dashboard, có thể kèm địa chỉ (`127.0.0.1:3080`). Mặc định `3080` trên mọi địa chỉ: chặn bằng firewall, hoặc gắn vào loopback như trên. |
| `RCLI_TRUSTED_PROXIES` | không | Mặc định `172.31.250.10`, địa chỉ của Dashboard trong mạng Compose. Chỉ với request từ các địa chỉ này, Server mới đọc IP thật của Controller từ `X-Forwarded-For` (để đếm Device Password sai theo từng Controller) và ghi lại API Address. Chỉ đổi khi đổi mạng trong `compose.yaml`. |
| `RCLI_HTTPS_CERT_FILE`, `RCLI_HTTPS_KEY_FILE` | không | Đặt cả hai cho Server chỉ có địa chỉ IP: mỗi lần khởi động, Server ghi một chứng chỉ do CA của nó cấp cho host của API Address (khi chưa có thì cho các host của Server Address) và key của nó (`0600`) vào hai đường dẫn này cho proxy TLS đọc. Restart proxy sau Server. |

## Vận hành

- **Log**: `docker compose logs -f server` trong `/opt/rcli`, bằng root.
- **Dừng và chạy**: `docker compose stop` và `docker compose up -d` ở đó. Mọi service tự chạy lại sau khi máy khởi động lại.
- **Dữ liệu**: metadata nằm trong volume Postgres `pgdata`; Agent Build và package nằm trong volume `filedata` của `storage`, không có cổng ra host: file chỉ tải được qua các bước kiểm của Server. Server không lưu lịch sử tài nguyên, inventory hay audit log của Device.
- **Wake** cần một Agent Online khác trong cùng subnet với Device cần bật (Wake Relay); nếu không, `rcli wake` báo `no_wake_relay`. Để một máy nhỏ luôn bật trong mỗi LAN.
