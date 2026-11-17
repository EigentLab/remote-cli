#!/bin/sh
# Installs an rcli Server on a Linux x86_64 machine into /opt/rcli, or moves an installed one to this version.
#
#   curl -fsSL https://rcli.eigentlab.com/install.sh | sudo sh
#   curl -fsSL https://rcli.eigentlab.com/install.sh | sudo sh -s -- --domain rcli.example.com
#
# Five ways to serve the Dashboard, all HTTPS towards the network (see USAGE). IP address mode is the default: the
# Dashboard on https://<IP>:<port> with a certificate from the Server's CA (ADR 0015). The secrets in /opt/rcli/.env
# are generated on the first run and kept after it; a later run with no argument keeps the mode, one with a mode
# switches to it and keeps the data and the Users.
set -eu

VERSION=1.0.1
DIR=${RCLI_ROOT:-}/opt/rcli # RCLI_ROOT: a temporary root for scripts/install-test.sh
DOCS=https://rcli.eigentlab.com
USAGE="usage: install.sh [mode]. The five modes:
  (no argument) [--ip <IPv4 address>] [--port <port>]
      IP address mode: https://<IP>:3443 (or --port) with a certificate from the Server's CA
  --domain <name>
      Caddy holds ports 80 and 443 and gets a Let's Encrypt certificate for <name>
  --domain <name> --cert <file> --key <file> [--port <port>]
      Caddy serves your certificate on port 8443 (or --port; --port alone moves it later)
  --tunnel <token>
      cloudflared connects a Cloudflare Tunnel; no Dashboard port is opened
  --behind-proxy
      nothing bundled; your proxy serves http://127.0.0.1:3080
--ip also names the address Agents and TURN use with --tunnel and --behind-proxy.
With no argument, an installed Server keeps its mode; to switch it to IP address mode, give --ip and --port."

die() { echo "rcli install: $*" >&2; exit 1; }
env_get() { sed -n "s/^$1=//p" .env 2>/dev/null | tail -n 1; }
set_env() { { grep -v "^$1=" .env || true; echo "$1=$2"; } > .env.new && mv .env.new .env; }
# A value from .env, when this run keeps the mode of the last one.
kept() { [ "$mode" != "$prev" ] || env_get "$1"; }
hex() { od -An -tx1 -N"$1" /dev/urandom | tr -d ' \n'; }
api() { curl -fsS -H "Authorization: Bearer $(env_get RCLI_BOOTSTRAP_ADMIN_TOKEN)" -H 'Content-Type: application/json' "$@"; }
wait_for() {
  i=0
  until "$@" > /dev/null 2>&1; do
    i=$((i + 1))
    [ "$i" -lt 90 ] || die "rcli did not start; see: cd $DIR && docker compose logs"
    sleep 2
  done
}

main() {
  addr= port= domain= cert= key= token= proxy=
  while [ $# -gt 0 ]; do
    case $1 in
      --behind-proxy) proxy=1; shift; continue ;;
      --ip | --port | --domain | --cert | --key | --tunnel) [ $# -ge 2 ] || die "$USAGE" ;;
      *) die "$USAGE" ;;
    esac
    case $1 in
      --ip) addr=$2 ;; --port) port=$2 ;; --domain) domain=$2 ;; --cert) cert=$2 ;; --key) key=$2 ;; --tunnel) token=$2 ;;
    esac
    shift 2
  done
  # The arguments given, in a fixed order: d(omain) c(ert) k(ey) t(unnel) p(roxy) i(p) n (port).
  args=${domain:+d}${cert:+c}${key:+k}${token:+t}${proxy:+p}${addr:+i}${port:+n}
  case $args in
    '') mode= ;;
    i | n | in) mode=ip ;;
    d) mode=domain ;;
    dck | dckn) mode=cert ;;
    t | ti) mode=tunnel ;;
    p | pi) mode=proxy ;;
    *) die "$USAGE" ;;
  esac
  case $cert in '' | /*) ;; *) cert=$PWD/$cert ;; esac
  case $key in '' | /*) ;; *) key=$PWD/$key ;; esac

  [ "$(uname -s)/$(uname -m)" = Linux/x86_64 ] ||
    die "this script installs on Linux x86_64 only; elsewhere follow the manual steps at $DOCS/en/server/#manual-install"
  if ! command -v docker > /dev/null; then
    echo "Installing Docker with Docker's script (https://get.docker.com)"
    curl -fsSL https://get.docker.com | sh
  fi
  docker compose version > /dev/null 2>&1 || die "Docker Compose v2 is missing: install Docker's docker-compose-plugin"

  umask 077
  mkdir -p "$DIR"
  cd "$DIR"
  if [ ! -f .env ]; then
    cat > .env << EOF
# Written by install.sh. Back it up: RCLI_MASTER_KEY seals the Server's CA; losing it loses the CA.
RCLI_BOOTSTRAP_ADMIN_TOKEN=$(hex 16)
RCLI_MASTER_KEY=$(hex 32)
RCLI_TURN_SECRET=$(hex 16)
RCLI_S3_ACCESS_KEY=$(hex 16)
RCLI_S3_SECRET_KEY=$(hex 32)
POSTGRES_PASSWORD=$(hex 16)
EOF
  fi
  prev=$(env_get RCLI_MODE)
  # --ip or --port alone never switches an installed Server to IP address mode: --port moves the certificate mode's.
  case $args/$prev in
    n/cert) mode=cert ;;
    [in]/ | [in]/ip) ;;
    [in]/*) die "this Server is in $prev mode. $USAGE" ;;
  esac
  mode=${mode:-${prev:-ip}}
  # The last mode's files are set aside: this mode writes what it needs. A mode that refuses its arguments puts them
  # and .env back, so the installed Server stays as it was.
  rm -rf .last && mkdir .last && cp .env .last/
  for f in compose.override.yaml Caddyfile tls; do [ ! -e "$f" ] || mv "$f" .last/; done
  trap 'rm -rf compose.override.yaml Caddyfile tls; mv .last/.env .; mv .last/* . 2> /dev/null || true; rm -rf .last' EXIT
  set_env RCLI_VERSION "$VERSION"
  # Every mode keeps the Dashboard on loopback unless .env says otherwise: this script calls the API there, and a
  # proxy is always in front. The ports set in .env stay.
  dashboard=$(env_get RCLI_DASHBOARD_PORT)
  dashboard=${dashboard:-127.0.0.1:3080}
  set_env RCLI_DASHBOARD_PORT "$dashboard"
  API=http://127.0.0.1:${dashboard##*:}/api/v1 # the Dashboard forwards the Controller API
  agent_port=$(env_get RCLI_AGENT_PORT)
  agent_port=${agent_port:-9443}
  write_compose
  url= open= note= moved=
  "mode_$mode"
  trap - EXIT
  rm -rf .last
  set_env RCLI_MODE "$mode"
  # The Server applies it at its start when it changed, so a switch works after the bootstrap token has stopped
  # working; empty: learned from the first https sign-in through the tunnel or proxy.
  set_env RCLI_API_ADDRESS "$url"

  docker compose up -d --remove-orphans
  echo "Waiting for the Server to start"
  wait_for curl -fs "$API/server-ca"
  password=
  # The bootstrap token works until a User signs in to the Dashboard; then the Users are theirs. Until then every run
  # sets admin's password again: a run that failed half way may have left admin without one.
  if api "$API/users" > /dev/null 2>&1; then
    password=$(LC_ALL=C tr -dc 'A-Za-z0-9' < /dev/urandom | head -c 20)
    echo '{"name":"admin"}' | api -X POST -d @- "$API/users" > /dev/null 2>&1 || true # admin is there after an earlier run
    printf '{"password":"%s"}' "$password" | api -X POST -d @- "$API/users/admin/credentials" > /dev/null
  fi
  # Caddy loads the Caddyfile and certificate files at its start only, and the Server writes its CA's at its own.
  case $mode in ip | domain | cert) docker compose restart caddy ;; esac
  [ -z "$port" ] || wait_for curl -fsk "https://127.0.0.1:$port/"
  fingerprint=$(curl -fsS "$API/server-ca" | sed '/-----/d' | base64 -d | sha256sum | cut -d' ' -f1)

  cat << EOF

rcli $VERSION is running.

  Dashboard:       ${url:-$note}
  CA fingerprint:  $fingerprint
EOF
  if [ -n "$password" ]; then
    cat << EOF
  Name:            admin
  Password:        $password   (shown once and not saved: write it down)
EOF
  else
    echo "  Users:           kept; no User was created"
  fi
  echo
  [ "$mode" != ip ] || cat << EOF
The browser warns about the certificate until you import the Server's CA, $url/api/v1/server-ca
(check its SHA-256 fingerprint against the one above).
EOF
  [ -n "$url" ] || echo "The API Address is learned at the first sign-in through it; check it on the Server Settings page."
  [ "$mode" != domain ] || echo "Caddy gets the certificate once $domain points at this machine and ports 80 and 443 are open."
  [ -z "$moved" ] || cat << EOF
The Server Address moved from $moved to $(env_get RCLI_AGENT_HOSTNAMES). Installed Agents keep the old one and are
offline until you run on each Device: sudo rcli agent server set $(env_get RCLI_AGENT_HOSTNAMES) --key <Server Key>
EOF
  cat << EOF
Open these ports: ${open}$agent_port/tcp (Agents), 3478/tcp and 3478/udp, 49160-49200/udp (TURN relay).
Back up $DIR/.env: its RCLI_MASTER_KEY seals the Server's CA, and the database cannot be used without it.
EOF
}

# The address of this machine for Agents and TURN: --ip, the one kept in .env, or the detected one.
ip_address() {
  addr=${addr:-$(env_get RCLI_IP)}
  if [ -z "$addr" ]; then
    addr=$(ip -4 route get 1.1.1.1 2> /dev/null | sed -n 's/.* src \([0-9.]*\).*/\1/p')
    [ -n "$addr" ] || die "could not detect the IP address; give it with --ip"
    echo "Detected IP address $addr (give --ip for another, such as the public address in front of a NAT)"
  fi
  case $addr in *[!0-9.]*) die "--ip takes an IPv4 address" ;; esac
  set_env RCLI_IP "$addr"
  agent_host "$addr"
}

# Agents and TURN reach the machine at $1. The Server applies a changed Server Address at its start, so a User's
# change on the Server Settings page stands until a run gives another address.
agent_host() {
  set_env RCLI_TURN_HOST "$1"
  moved=$(env_get RCLI_AGENT_HOSTNAMES)
  [ "$moved" != "$1:$agent_port" ] || moved=
  set_env RCLI_AGENT_HOSTNAMES "$1:$agent_port"
}

https_port() {
  port=${port:-$(kept RCLI_HTTPS_PORT)}
  port=${port:-$1}
  case $port in '' | *[!0-9]*) die "--port takes a port number" ;; esac
  set_env RCLI_HTTPS_PORT "$port"
  open="$port/tcp (Dashboard), "
}

domain_name() {
  domain=${domain:-$(kept RCLI_DOMAIN)}
  case $domain in '' | .* | *[!A-Za-z0-9.-]*) die "--domain takes a host name such as rcli.example.com" ;; esac
  set_env RCLI_DOMAIN "$domain"
  agent_host "$domain"
}

# The bundled Caddy in front of the Dashboard: it publishes ports $1 and reads volume $2.
caddy_service() {
  cat << EOF
  caddy:
    image: caddy:2
    restart: unless-stopped # retries until the Server has written the certificate
    depends_on: [dashboard]
    ports: [$1]
    volumes:
      - ./Caddyfile:/etc/caddy/Caddyfile:ro
      - $2
EOF
}

# The Caddyfile: the site at address $1, and the line $2 naming its certificate files (none: Let's Encrypt).
caddyfile() {
  cat > Caddyfile << EOF
# Written by install.sh, $mode mode.
$1 {
	$2
	reverse_proxy dashboard:3000 {
		# Stream the shell and the Control Lock watch as they come.
		flush_interval -1
	}
}
EOF
}

# IP address mode: Caddy serves the Dashboard over HTTPS with the certificate the Server issues from its CA for the
# API Address at each start.
mode_ip() {
  ip_address
  https_port 3443
  echo "IP address $addr, Dashboard port $port"
  url=https://$addr:$port
  {
    cat << 'EOF'
# Written by install.sh, IP address mode: Caddy serves the Dashboard over HTTPS on RCLI_HTTPS_PORT with the
# certificate the Server writes from its CA at each start (ADR 0015).
services:
  server:
    environment:
      RCLI_HTTPS_CERT_FILE: /certs/https.crt
      RCLI_HTTPS_KEY_FILE: /certs/https.key
    volumes:
      - certs:/certs
EOF
    caddy_service '"${RCLI_HTTPS_PORT}:3443"' certs:/certs:ro
    printf 'volumes:\n  certs:\n'
  } > compose.override.yaml
  caddyfile :3443 'tls /certs/https.crt /certs/https.key'
}

# Domain mode: Caddy holds ports 80 and 443 and gets a Let's Encrypt certificate (kept in the caddydata volume).
mode_domain() {
  domain_name
  url=https://$domain
  open="80/tcp and 443/tcp (Dashboard), "
  {
    echo '# Written by install.sh, domain mode: Caddy serves the Dashboard with a Let'"'"'s Encrypt certificate.'
    echo services:
    caddy_service '"80:80", "443:443"' caddydata:/data
    printf 'volumes:\n  caddydata:\n'
  } > compose.override.yaml
  caddyfile "$domain" ''
}

# Certificate mode: Caddy serves the person's certificate on RCLI_HTTPS_PORT. The files are copied into tls/; after
# renewing them, run the script again.
mode_cert() {
  domain_name
  cert=${cert:-$(kept RCLI_CERT_FILE)} key=${key:-$(kept RCLI_KEY_FILE)}
  [ -f "$cert" ] && [ -f "$key" ] || die "no certificate or key file at $cert and $key"
  https_port 8443
  url=https://$domain:$port
  set_env RCLI_CERT_FILE "$cert"
  set_env RCLI_KEY_FILE "$key"
  mkdir tls
  cp "$cert" tls/https.crt
  cp "$key" tls/https.key
  {
    echo '# Written by install.sh, certificate mode: Caddy serves the Dashboard with the certificate in tls/.'
    echo services:
    caddy_service '"${RCLI_HTTPS_PORT}:3443"' ./tls:/certs:ro
  } > compose.override.yaml
  caddyfile :3443 'tls /certs/https.crt /certs/https.key'
}

# Tunnel mode: cloudflared carries the Dashboard out through a Cloudflare Tunnel; no port is opened for it.
mode_tunnel() {
  token=${token:-$(kept RCLI_TUNNEL_TOKEN)}
  case $token in '' | *[!A-Za-z0-9+/=_-]*) die "--tunnel takes the tunnel token from the Cloudflare dashboard" ;; esac
  set_env RCLI_TUNNEL_TOKEN "$token"
  ip_address
  cat > compose.override.yaml << 'EOF'
# Written by install.sh, tunnel mode: cloudflared carries the Dashboard out through a Cloudflare Tunnel.
services:
  cloudflared:
    image: cloudflare/cloudflared:latest
    restart: unless-stopped
    depends_on: [dashboard]
    command: tunnel --no-autoupdate run
    environment:
      TUNNEL_TOKEN: ${RCLI_TUNNEL_TOKEN}
EOF
  note="the public hostname of your tunnel; in Cloudflare, route it to http://dashboard:3000"
}

# Behind-proxy mode: nothing bundled; the person's proxy serves the Dashboard from the loopback port.
mode_proxy() {
  ip_address
  note="your proxy's https:// URL; point it at http://127.0.0.1:${dashboard##*:} with X-Forwarded-For and X-Forwarded-Proto"
}

# The repo's compose.yaml, unchanged (scripts/install-test.sh compares them).
write_compose() {
  cat > compose.yaml << 'RCLI_COMPOSE_EOF'
# Server + Postgres + file storage + Dashboard + coturn. Set RCLI_BOOTSTRAP_ADMIN_TOKEN, RCLI_TURN_SECRET,
# RCLI_TURN_HOST (the VPS's public name/IP; Agents and browsers must reach it), RCLI_S3_ACCESS_KEY and
# RCLI_S3_SECRET_KEY (and POSTGRES_PASSWORD) in the environment or .env.
# The Server and Dashboard images are pulled from ghcr.io/eigentlab at RCLI_VERSION (default `latest`); to build
# them from source instead, run `docker compose build` or `docker compose up -d --build`.
# Backup = `pg_dump` of Postgres plus the `filedata` volume (install packages and Agent builds live there, not in
# the database).
services:
  postgres:
    image: postgres:16-alpine
    restart: unless-stopped
    environment:
      POSTGRES_USER: rcli
      POSTGRES_PASSWORD: ${POSTGRES_PASSWORD:-rcli}
      POSTGRES_DB: rcli
    volumes:
      - pgdata:/var/lib/postgresql/data
    healthcheck:
      test: ["CMD", "pg_isready", "-U", "rcli"]
      interval: 2s
      retries: 30

  # SeaweedFS with its S3 gateway: install packages and Agent builds. Internal only (no host port): files are
  # served through the Server's access checks.
  storage:
    image: chrislusf/seaweedfs:4.53
    restart: unless-stopped
    command: ["server", "-s3", "-dir=/data"]
    environment:
      AWS_ACCESS_KEY_ID: ${RCLI_S3_ACCESS_KEY:?set RCLI_S3_ACCESS_KEY}
      AWS_SECRET_ACCESS_KEY: ${RCLI_S3_SECRET_KEY:?set RCLI_S3_SECRET_KEY}
    volumes:
      - filedata:/data
    healthcheck:
      test: ["CMD", "wget", "-q", "-O", "/dev/null", "http://127.0.0.1:8333/healthz"]
      interval: 2s
      retries: 30

  server:
    image: ghcr.io/eigentlab/remote-cli-server:${RCLI_VERSION:-latest}
    restart: unless-stopped
    build: .
    depends_on:
      postgres:
        condition: service_healthy
      storage:
        condition: service_healthy
    environment:
      DATABASE_URL: postgres://rcli:${POSTGRES_PASSWORD:-rcli}@postgres:5432/rcli
      RCLI_BOOTSTRAP_ADMIN_TOKEN: ${RCLI_BOOTSTRAP_ADMIN_TOKEN:?set RCLI_BOOTSTRAP_ADMIN_TOKEN}
      # The Server Address (names Agents use to reach port 9443 on the host): applied at a start when it changed since
      # the last one, so a change with `rcli server-address set` or the Server Settings page stands until then.
      RCLI_AGENT_HOSTNAMES: ${RCLI_AGENT_HOSTNAMES:-localhost}
      # Seals the CA private key and TOTP secrets: 64 hex chars (`openssl rand -hex 32`). Keep a backup; losing it loses them.
      RCLI_MASTER_KEY: ${RCLI_MASTER_KEY:-}
      # The API Address an install's mode gives (install.sh sets it): applied at a start when it changed since the last
      # one, so a User's correction stands until then. Empty = learned from a User's first https visit.
      RCLI_API_ADDRESS: ${RCLI_API_ADDRESS:-}
      # The Dashboard forwards the Controller API: from its IP the Controller's IP is read from X-Forwarded-For (which
      # the TLS proxy in front of the Dashboard must append), so wrong Device Passwords are counted per real Controller.
      RCLI_TRUSTED_PROXIES: ${RCLI_TRUSTED_PROXIES:-172.31.250.10}
      # WebRTC (live view): peers try direct paths first, then relay through coturn with short-lived credentials.
      RCLI_ICE_URLS: stun:${RCLI_TURN_HOST:-localhost}:3478,turn:${RCLI_TURN_HOST:-localhost}:3478?transport=udp,turn:${RCLI_TURN_HOST:-localhost}:3478?transport=tcp
      RCLI_TURN_SECRET: ${RCLI_TURN_SECRET:?set RCLI_TURN_SECRET}
      # The file storage service (the Server creates the bucket if missing).
      RCLI_S3_ENDPOINT: ${RCLI_S3_ENDPOINT:-http://storage:8333}
      RCLI_S3_BUCKET: ${RCLI_S3_BUCKET:-rcli}
      RCLI_S3_ACCESS_KEY: ${RCLI_S3_ACCESS_KEY:?set RCLI_S3_ACCESS_KEY}
      RCLI_S3_SECRET_KEY: ${RCLI_S3_SECRET_KEY:?set RCLI_S3_SECRET_KEY}
    ports:
      # The Controller API (8080) stays internal: Controllers reach it through the Dashboard (ADR 0006).
      - "${RCLI_AGENT_PORT:-9443}:8443" # Agent connections (TLS / mTLS); inside the container the port stays 8443

  dashboard:
    image: ghcr.io/eigentlab/remote-cli-dashboard:${RCLI_VERSION:-latest}
    restart: unless-stopped
    build: ./dashboard
    depends_on: [server]
    environment:
      RCLI_API_URL: http://server:8080
    ports:
      - "${RCLI_DASHBOARD_PORT:-3080}:3000" # Dashboard and Controller API (put TLS in front of it; reach it only through that proxy)
    networks:
      default:
        ipv4_address: 172.31.250.10 # RCLI_TRUSTED_PROXIES

  coturn:
    image: coturn/coturn:4.6
    restart: unless-stopped
    # Host networking: TURN allocates relay ports from a range; open 3478 tcp/udp and 49160-49200/udp on the VPS.
    network_mode: host
    command:
      - --listening-port=3478
      - --min-port=49160
      - --max-port=49200
      - --realm=rcli
      - --use-auth-secret
      - --static-auth-secret=${RCLI_TURN_SECRET:?set RCLI_TURN_SECRET}
      - --fingerprint
      - --no-cli
      - --no-multicast-peers
      # Never relay into private networks (the VPS's Docker network, Postgres).
      - --denied-peer-ip=10.0.0.0-10.255.255.255
      - --denied-peer-ip=172.16.0.0-172.31.255.255
      - --denied-peer-ip=192.168.0.0-192.168.255.255
      - --log-file=stdout

volumes:
  pgdata:
  filedata:

networks:
  default:
    ipam:
      config:
        - subnet: 172.31.250.0/24 # fixed, so the Server can trust the Dashboard's address
RCLI_COMPOSE_EOF
}

# Called last, so a download cut short runs nothing.
main "$@"
