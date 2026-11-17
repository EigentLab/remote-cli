---
title: Server guide
description: Install, run, back up, upgrade and remove an rcli Server.
sidebar:
  order: 2
---

The Server connects Controllers to Devices. It runs as one Docker Compose project: the Server, Postgres, file storage (SeaweedFS), the Dashboard and coturn (the TURN relay for Live View), plus Caddy or cloudflared depending on the mode. The Server and `rcli` share one version number: run a Server and its Devices on the same version.

## Install

On a Linux x86_64 server (Debian or Ubuntu are the first targets), run:

```bash
curl -fsSL https://rcli.eigentlab.com/install.sh | sudo sh
```

The same script is in the repo: `https://raw.githubusercontent.com/EigentLab/remote-cli/main/install.sh`.

The script asks for nothing. It installs Docker with Docker's own script when Docker is missing, writes everything to `/opt/rcli/`, generates every secret into `/opt/rcli/.env`, pulls the Server and Dashboard images of its version from `ghcr.io/eigentlab/`, starts them and creates the first User, `admin`, with a random password. It ends like this:

```text
rcli 1.0.3 is running.

  Dashboard:       https://203.0.113.10:3443
  CA fingerprint:  4f1c…
  Name:            admin
  Password:        …   (shown once and not saved: write it down)

The browser warns about the certificate until you import the Server's CA, https://203.0.113.10:3443/api/v1/server-ca
(check its SHA-256 fingerprint against the one above).
Open these ports: 3443/tcp (Dashboard), 9443/tcp (Agents), 3478/tcp and 3478/udp, 49160-49200/udp (TURN relay).
Back up /opt/rcli/.env: its RCLI_MASTER_KEY seals the Server's CA, and the database cannot be used without it.
```

The password is printed once and stored nowhere. On another OS or architecture the script stops before changing anything; follow [Manual install](#manual-install) instead.

Then:

1. Open the [ports](#ports) it names.
2. Sign in to the Dashboard as `admin` and change the password on the Users page (or create your own User there and disable `admin`).
3. Upload the `rcli` files from the [GitHub release](https://github.com/EigentLab/remote-cli/releases) on the Agent updates page (**Cập nhật Agent**). A file named like `rcli-1.0.3-linux-x86_64` fills in the version and target by itself. New Devices download their `rcli` from these Agent Builds, so the install commands on the Connect Guide work only after this step. The `.dmg` and `.deb` packages go up with `rcli package publish` (see [Server administration](/en/usage/#server-administration)).
4. Add Devices: see [Install on a Device](/en/devices/).

## The five modes

Every mode serves the Dashboard over HTTPS; none serves plain HTTP to the network. In every mode the Dashboard itself listens on `127.0.0.1:3080` only, behind the proxy.

| Command | Dashboard | Certificate |
|---|---|---|
| no argument, or `--ip <IPv4>` and/or `--port <port>` | `https://<IP>:3443` | issued by the Server's own CA |
| `--domain <name>` | `https://<name>` (ports 80 and 443) | Let's Encrypt |
| `--domain <name> --cert <file> --key <file> [--port <port>]` | `https://<name>:8443` | yours |
| `--tunnel <token>` | your tunnel's public hostname | Cloudflare's |
| `--behind-proxy` | your proxy's URL | your proxy's |

Arguments go after `sh -s --`, for example:

```bash
curl -fsSL https://rcli.eigentlab.com/install.sh | sudo sh -s -- --domain rcli.example.com
```

### IP address (default)

For a server with no domain name. The script detects the machine's IPv4 address and prints it; behind a NAT, give the public one with `--ip`. The port is 3443, or `--port`. Ports 80 and 443 stay free for other software.

```bash
curl -fsSL https://rcli.eigentlab.com/install.sh | sudo sh -s -- --ip 203.0.113.10 --port 3443
```

At every start the Server issues a certificate for that address from its CA (valid 800 days) and the bundled Caddy serves it. Every rcli install already trusts this CA: the Server Key carries its fingerprint, and the install keeps the CA certificate after checking it. So `rcli` and `rcli mcp` connect over verified HTTPS, and the Linux and macOS install command on the Connect Guide carries the CA certificate for `curl`. Browsers do not know this CA: they show a certificate warning until you [import the CA certificate](#the-ca-certificate). On Windows, the Connect Guide tells you to download `rcli.exe` in the browser (accept the warning, or import the CA first) and start it.

If you change the API Address on the Server Settings page, restart both the Server and Caddy so the certificate follows it: `cd /opt/rcli && docker compose restart server caddy` (as root; Caddy reads the certificate only when it starts).

### Domain name

```bash
curl -fsSL https://rcli.eigentlab.com/install.sh | sudo sh -s -- --domain rcli.example.com
```

The bundled Caddy holds ports 80 and 443 and gets a Let's Encrypt certificate once `rcli.example.com` points at this machine and both ports are open. Agents and TURN use the same name.

### Your own certificate

```bash
curl -fsSL https://rcli.eigentlab.com/install.sh | sudo sh -s -- \
  --domain rcli.example.com --cert /etc/ssl/rcli/fullchain.pem --key /etc/ssl/rcli/privkey.pem --port 8443
```

Caddy serves your certificate on port 8443 (or `--port`). The script copies both files into `/opt/rcli/tls/`, so after renewing them, run the command again with no argument: it copies them again from the same paths and restarts Caddy.

### Cloudflare Tunnel

```bash
curl -fsSL https://rcli.eigentlab.com/install.sh | sudo sh -s -- --tunnel <tunnel token>
```

A bundled `cloudflared` connects the tunnel, so no Dashboard port is opened on the machine. In the Cloudflare dashboard, create the tunnel, copy its token, and route its public hostname to `http://dashboard:3000`. Agents and TURN still reach the machine directly on its IP address (detected, or `--ip`), so their ports must be open. The API Address is learned the first time a User signs in through the hostname over `https://`; check it on the Server Settings page.

### Behind your reverse proxy

```bash
curl -fsSL https://rcli.eigentlab.com/install.sh | sudo sh -s -- --behind-proxy
```

Nothing is bundled. Point your proxy at `http://127.0.0.1:3080`. It must:

- append the client's address to `X-Forwarded-For` and set `X-Forwarded-Proto: https`, and pass the `Host` the browser used;
- not buffer or compress responses: the shell and the Control Lock are streamed;
- allow request bodies up to 200 MB (Agent Builds) and responses that take up to an hour (`rcli exec --timeout 3600`).

Caddy does the first two by default:

```text
rcli.example.com {
	reverse_proxy 127.0.0.1:3080 {
		flush_interval -1
	}
}
```

With nginx:

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

`--ip` names the address Agents and TURN use, as with `--tunnel`. The API Address is learned at the first `https://` sign-in through the proxy.

### Switching modes and running it again

Run the command again with another mode to switch: the secrets, the data, the Users and the Devices stay, but a switch that moves the Server Address (see below) leaves the installed Agents offline until they are pointed at the new one, and the script says so. A run with no argument keeps the installed mode. A run that refuses its arguments changes nothing. A second run never regenerates the secrets and never creates a User once one has signed in (the output then says `Users: kept`); until then, each run sets `admin`'s password again and prints it.

`--ip` or `--port` alone never switches an installed Server to IP address mode: give both to switch to it. In certificate mode, `--port` alone moves its port. A run that gives another address (another `--ip`, or a switch to or from a domain name) moves the Server Address with it; installed Agents keep the old one until `sudo rcli agent server set <host>:<port> --key <Server Key>` points them at the new one.

## Ports

| Port | Protocol | For |
|---|---|---|
| 3443 (IP mode), 80 and 443 (`--domain`), 8443 (`--cert`) | TCP | the Dashboard and the Controller API |
| 9443 | TCP | Agents (TLS with client certificates); never put a proxy in front of it |
| 3478 | TCP and UDP | TURN/STUN (coturn) |
| 49160–49200 | UDP | coturn's relay ports |

The script does not change the firewall. With `ufw`, in IP address mode:

```bash
sudo ufw allow 3443/tcp
sudo ufw allow 9443/tcp
sudo ufw allow 3478
sudo ufw allow 49160:49200/udp
```

Docker opens the ports it publishes (the Agent port, and the Dashboard port of the bundled Caddy) in iptables itself, past `ufw`; coturn runs on the host network, so `ufw` does apply to it. A cloud provider's firewall or security group must allow them all.

The Controller API (port 8080 inside the Docker network) is never published: `rcli` and `rcli mcp` reach it through the Dashboard's URL.

**Changing the Agent port.** If 9443 is taken, add `RCLI_AGENT_PORT=<port>` to `/opt/rcli/.env`, run `docker compose up -d` in `/opt/rcli` (as root), and set the Server Address to `<host>:<port>` on the Server Settings page or with `rcli server-address set`. Do it before adding Devices: an installed Agent keeps the address it was installed with, until `sudo rcli agent server set <host>:<port> --key <Server Key>` points it at the new one. `RCLI_DASHBOARD_PORT` sets the Dashboard's host port. The install script writes `127.0.0.1:3080` when it is not set, and keeps both ports when you set them in `/opt/rcli/.env`.

## The CA certificate

Each Server has its own CA. It signs the Agents' certificates, the Server Key carries its fingerprint, and in IP address mode it signs the Dashboard's certificate. The install prints the fingerprint (`CA fingerprint`); the certificate is at `<Dashboard URL>/api/v1/server-ca`.

Download it and check the fingerprint before you trust it (`-k` because the browser-facing certificate is not trusted yet; the fingerprint check is what proves it is yours):

```bash
curl -fsSk https://203.0.113.10:3443/api/v1/server-ca -o rcli-ca.pem
openssl x509 -in rcli-ca.pem -outform der | sha256sum      # macOS: shasum -a 256
```

The result must equal the `CA fingerprint` the install printed. Then import it to remove the browser's warning:

- **macOS**: `sudo security add-trusted-cert -d -r trustRoot -k /Library/Keychains/System.keychain rcli-ca.pem` (Safari and Chrome use it).
- **Windows**: in an administrator prompt, `certutil -addstore -f Root rcli-ca.pem` (Edge and Chrome use it).
- **Debian and Ubuntu**: `sudo cp rcli-ca.pem /usr/local/share/ca-certificates/rcli-ca.crt && sudo update-ca-certificates`. Chrome on Linux has its own store: Settings → Privacy and security → Security → Manage certificates → Authorities → Import.
- **Firefox**, on every OS: Settings → Privacy & Security → Certificates → View Certificates → Authorities → Import, then tick "Trust this CA to identify websites".

An imported CA can vouch for any website, so import only the CA of a Server you run.

## Backups

`/opt/rcli` and its `.env` belong to root, so the commands below run as root (`sudo -i`). Back up three things:

- **`/opt/rcli/.env`**, once. Its `RCLI_MASTER_KEY` seals the CA's private key and the Users' TOTP secrets in the database. Without it the Server does not start on that database, and the CA is lost: every Device would have to be installed again with a new Server Key. Keep this file somewhere safe: with a database dump, it is the whole Server.
- **The database**, regularly.
- **The files** (Agent Builds and packages) in the `rcli_filedata` volume. They can also be uploaded again from the release.

```bash
cd /opt/rcli
cp .env /root/rcli.env
docker compose exec -T postgres pg_dump -U rcli -Fc rcli > /root/rcli-db.dump
docker run --rm -v rcli_filedata:/data -v /root:/backup alpine tar czf /backup/rcli-files.tgz -C /data .
```

Then copy the three files off the machine.

To restore on a new machine, put the three files in `/root`, then, as root:

```bash
mkdir -p /opt/rcli && install -m 600 /root/rcli.env /opt/rcli/.env
curl -fsSL https://rcli.eigentlab.com/install.sh | sh     # the same version; it keeps the .env (ignore the User it creates)
cd /opt/rcli
docker compose stop server dashboard storage
docker compose exec -T postgres pg_restore -U rcli -d rcli --clean --if-exists < /root/rcli-db.dump
docker run --rm -v rcli_filedata:/data -v /root:/backup alpine tar xzf /backup/rcli-files.tgz -C /data
curl -fsSL https://rcli.eigentlab.com/install.sh | sh     # starts everything again
```

Devices reach the Server at its Server Address, so give the new machine the same IP address or domain name (on the first run, pass the mode's arguments when they changed, such as `--ip`), or point each Device at the new address with `sudo rcli agent server set`.

## Lost password

If another User can still sign in, they set a new password on the Users page or with `rcli user set-password <name> --password '<new password>'`.

If nobody can sign in, use the bootstrap token in `/opt/rcli/.env`. It works again once no enabled User has a live session; a session lasts 12 hours after sign-in (`rcli logout` ends one sooner). On the Server, as root:

```bash
cd /opt/rcli
TOKEN=$(sed -n 's/^RCLI_BOOTSTRAP_ADMIN_TOKEN=//p' .env)
curl -fsS -H "Authorization: Bearer $TOKEN" -H 'Content-Type: application/json' \
  -d '{"password":"<new password>"}' http://127.0.0.1:3080/api/v1/users/admin/credentials
```

Replace `admin` with the User's name. The User's TOTP setting stays. To not wait for the sessions to expire, end them all first:

```bash
docker compose exec postgres psql -U rcli rcli -c "UPDATE api_tokens SET revoked_at = now() WHERE revoked_at IS NULL"
```

A User who lost their TOTP authenticator cannot turn TOTP off. Create a new User the same way (`-d '{"name":"<name>"}'` to `http://127.0.0.1:3080/api/v1/users`, then set its password as above), sign in with it, and disable the old one. Five wrong sign-ins in a row lock a User out for 15 minutes.

## Upgrading

Upgrade the Server first, then its Devices:

1. **The Server.** Back it up, then run the new version's install command again (`curl -fsSL https://rcli.eigentlab.com/install.sh | sudo sh`). It pulls that version's images and restarts, keeping the mode, the `.env`, the data and the Users.
2. **The Agent Builds.** Upload the new version's `rcli` files on the Agent updates page (or with `rcli build upload`), and the new packages with `rcli package publish`. The highest version per target is the Latest Build.
3. **The Devices.** Either run `sudo rcli update` on each (`rcli update` in User Mode), or set the **Minimum Version** on the Server Settings page to the new version. Every connected Agent below the Minimum Version then downloads the Latest Build, checks its sha256 and restarts; one that fails tries again every hour, and an Offline Device updates when it next connects. A Device Owner can turn this off with `rcli agent general auto-update off`.

The order matters because Devices update only to a Latest Build the Server holds: the Server offers an Agent an update only when the Agent is below the Minimum Version and the Latest Build is above the Agent's version. A Minimum Version set before the new builds are uploaded updates no one. While a Minimum Version is set, Devices below it show "update needed" and Controllers below it print `Server recommends rcli ≥ <version>, run: rcli update`.

## Uninstall

As root:

```bash
cd /opt/rcli
docker compose down -v        # stops rcli and deletes its volumes: the database, the files, the certificates
cd / && rm -rf /opt/rcli      # the compose files, the proxy files and .env (with the master key)
docker image rm ghcr.io/eigentlab/remote-cli-server:1.0.3 ghcr.io/eigentlab/remote-cli-dashboard:1.0.3
```

Docker stays installed. Close the ports you opened (`sudo ufw delete allow 9443/tcp`, and so on). Devices still installed keep trying to connect: run `rcli uninstall` on each (it warns when the Server is gone and removes the local install anyway).

## Manual install

Use this when the install script does not fit: another OS or architecture, your own Compose setup, or images built from source. You need Docker with Compose v2 and a TLS reverse proxy.

1. Get the compose file of the version you run:

   ```bash
   mkdir rcli && cd rcli
   curl -fsSLO https://raw.githubusercontent.com/EigentLab/remote-cli/v1.0.3/compose.yaml
   ```

2. Write `.env` next to it:

   ```bash
   cat > .env <<EOF
   RCLI_VERSION=1.0.3
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

3. Start it, then put your TLS proxy in front of the Dashboard port as in [Behind your reverse proxy](#behind-your-reverse-proxy):

   ```bash
   docker compose up -d                  # pulls the images; `docker compose up -d --build` builds them from source
   docker compose ps                     # postgres, storage, server, dashboard and coturn are running
   ```

4. Create the first User with the bootstrap token, on the same machine:

   ```bash
   TOKEN=$(sed -n 's/^RCLI_BOOTSTRAP_ADMIN_TOKEN=//p' .env)
   curl -fsS -H "Authorization: Bearer $TOKEN" -H 'Content-Type: application/json' \
     -d '{"name":"admin"}' http://127.0.0.1:3080/api/v1/users
   curl -fsS -H "Authorization: Bearer $TOKEN" -H 'Content-Type: application/json' \
     -d '{"password":"<password>"}' http://127.0.0.1:3080/api/v1/users/admin/credentials
   ```

   The bootstrap token stops working as soon as a User has signed in (see [Lost password](#lost-password)).

| Variable | Required | Meaning |
|---|---|---|
| `RCLI_BOOTSTRAP_ADMIN_TOKEN` | yes | Acts as a User while no enabled User has a live session: for the first User and for rescue. |
| `RCLI_MASTER_KEY` | strongly advised | 64 hex characters. Seals the CA private key and the TOTP secrets. Without it they sit in Postgres in the clear (the Server logs a warning); with it, they are sealed at the next start. Once sealed, the Server does not start without this key, and another key cannot open them. There is no key rotation. |
| `RCLI_TURN_SECRET` | yes | Shared by the Server and coturn to issue TURN credentials. |
| `RCLI_TURN_HOST` | advised | The public name or IP browsers and Agents use to reach coturn. Default `localhost`. |
| `RCLI_AGENT_HOSTNAMES` | no | The Server Address (`host:port`, comma-separated; it goes into the Agent port's certificate). Applied at a start when it changed since the last one, so a change on the Server Settings page or with `rcli server-address set` stands until then. Default `localhost`. |
| `RCLI_API_ADDRESS` | no | The Dashboard's `https://` URL. Applied at a start when it changed since the last one. Empty: learned from a User's first `https://` sign-in. The Server does not start when it is not an `https://` origin. |
| `RCLI_S3_ACCESS_KEY`, `RCLI_S3_SECRET_KEY` | yes | Credentials of the `storage` service; Compose gives the same values to `storage` and the Server. |
| `RCLI_S3_ENDPOINT`, `RCLI_S3_BUCKET` | no | Default `http://storage:8333` and `rcli`. The Server creates the bucket when missing. |
| `POSTGRES_PASSWORD` | advised | Default `rcli`. |
| `RCLI_VERSION` | no | The image tag to pull. Default `latest`. |
| `RCLI_AGENT_PORT` | no | The host port Agents connect to. Default `9443`. The Server Address must use it. |
| `RCLI_DASHBOARD_PORT` | no | The Dashboard's host port, optionally with an address (`127.0.0.1:3080`). Default `3080` on every address: firewall it, or bind it to the loopback address as above. |
| `RCLI_TRUSTED_PROXIES` | no | Default `172.31.250.10`, the Dashboard's address in the Compose network. Only from these addresses does the Server read the Controller's real IP from `X-Forwarded-For` (so wrong Device Passwords are counted per Controller) and learn the API Address. Change it only if you change the network in `compose.yaml`. |
| `RCLI_HTTPS_CERT_FILE`, `RCLI_HTTPS_KEY_FILE` | no | Set both for a Server on an IP address: at every start the Server writes a certificate from its CA for the API Address host (before there is one, for the Server Address hosts) and its key (`0600`) to these paths, for the TLS proxy to read. Restart the proxy after the Server. |

## Running it

- **Logs**: `docker compose logs -f server` in `/opt/rcli`, as root.
- **Stop and start**: `docker compose stop` and `docker compose up -d` there. Every service restarts by itself after a reboot.
- **Data**: metadata is in the Postgres volume `pgdata`; Agent Builds and packages are in the `filedata` volume of `storage`, which has no host port: files are downloaded only through the Server's checks. The Server keeps no resource history, inventory or audit log of Devices.
- **Wake** needs another Online Agent on the same subnet as the Device to wake (a Wake Relay); `rcli wake` answers `no_wake_relay` otherwise. Keep a small machine always on in each LAN.
