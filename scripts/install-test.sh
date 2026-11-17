#!/bin/sh
# Runs install.sh in a temporary root against stub docker, curl, ip and uname: the files it writes, the printed name and
# password, and a second run that keeps the secrets and the Users and moves to the script's version.
set -eu
cd "$(dirname "$0")/.."
t=$(mktemp -d)
trap 'rm -rf "$t"' EXIT
fail() { echo "FAIL: $*"; exit 1; }
mkdir "$t/bin" "$t/root"
echo '[]' > "$t/users"

stub() { printf '#!/bin/sh\n%s\n' "$2" > "$t/bin/$1" && chmod +x "$t/bin/$1"; }
stub docker "echo \"docker \$*\" >> '$t/log'"
stub uname 'case $1 in -s) echo Linux ;; -m) echo x86_64 ;; esac'
stub ip "echo '1.1.1.1 via 10.0.0.1 dev eth0 src 203.0.113.7 uid 0'"
# The Server through the Dashboard's loopback port: a CA of DER bytes 00 01 02, and the Users created so far.
stub curl "echo \"curl \$*\" >> '$t/log'
case \"\$*\" in
  *server-ca*) printf -- '-----BEGIN CERTIFICATE-----\nAAEC\n-----END CERTIFICATE-----\n' ;;
  *'-X POST'*/users) cat > /dev/null; echo '[{\"name\":\"admin\"}]' > '$t/users' ;;
  */users) cat '$t/users' ;;
  *) echo \"\$* <<< \$(cat)\" >> '$t/log' ;;
esac"
run() { PATH="$t/bin:$PATH" RCLI_ROOT="$t/root" sh install.sh "$@" < /dev/null > "$t/out" 2>&1 || { cat "$t/out"; fail "install.sh $* failed"; }; }
dir=$t/root/opt/rcli

run
version=$(sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml | head -n 1)
cmp -s "$dir/compose.yaml" compose.yaml || fail "compose.yaml differs from the repo's"
[ -s "$dir/compose.override.yaml" ] && [ -s "$dir/Caddyfile" ] || fail "no proxy configuration"
for k in RCLI_BOOTSTRAP_ADMIN_TOKEN RCLI_MASTER_KEY RCLI_TURN_SECRET RCLI_S3_ACCESS_KEY RCLI_S3_SECRET_KEY POSTGRES_PASSWORD; do
  grep -Eq "^$k=[0-9a-f]{32,}$" "$dir/.env" || fail "no generated $k"
done
grep -q "^RCLI_VERSION=$version$" "$dir/.env" || fail "RCLI_VERSION is not $version"
grep -q '^RCLI_AGENT_HOSTNAMES=203.0.113.7:9443$' "$dir/.env" || fail "Server Address is not the detected IP"
grep -q '203.0.113.7' "$t/out" || fail "detected IP not printed"
grep -q 'https://203.0.113.7:3443' "$t/out" || fail "Dashboard URL not printed"
grep -q '^RCLI_API_ADDRESS=https://203.0.113.7:3443$' "$dir/.env" || fail "API Address not set"
grep -q "$(printf '\000\001\002' | sha256sum | cut -d' ' -f1)" "$t/out" || fail "CA fingerprint not printed"
grep -q 'Name: *admin' "$t/out" || fail "name not printed"
password=$(sed -n 's/.*Password: *\([A-Za-z0-9]\{16,\}\).*/\1/p' "$t/out")
[ -n "$password" ] || fail "password not printed"
! grep -rq "$password" "$t/root" || fail "password written to disk"

cp "$dir/.env" "$t/env1"
sed -i.bak "s/^RCLI_VERSION=.*/RCLI_VERSION=0.9.0/" "$dir/.env" && rm "$dir/.env.bak"
: > "$t/log"
run --ip 198.51.100.4 --port 4443
grep -q "^RCLI_VERSION=$version$" "$dir/.env" || fail "second run did not move to $version"
grep -q '^RCLI_HTTPS_PORT=4443$' "$dir/.env" || fail "--port not applied"
grep -q '198.51.100.4' "$t/out" || fail "--ip not used"
grep '^RCLI_\(BOOTSTRAP\|MASTER\|TURN_SECRET\|S3\)\|^POSTGRES' "$t/env1" | while read -r line; do
  grep -qx "$line" "$dir/.env" || fail "second run changed ${line%%=*}"
done
! grep -q -- '-X POST.*/users' "$t/log" || fail "second run created a User"
! grep -q 'Password:' "$t/out" || fail "second run printed a password"
grep -q 'docker compose up' "$t/log" || fail "second run did not restart the stack"
grep -q '^RCLI_API_ADDRESS=https://198.51.100.4:4443$' "$dir/.env" || fail "--ip did not move the API Address"
grep -q '^RCLI_AGENT_HOSTNAMES=198.51.100.4:9443$' "$dir/.env" || fail "--ip did not move the Server Address"

# The other modes, each a switch on the installed Server: only how the Dashboard is served changes.
mode() { grep -q "^RCLI_MODE=$1$" "$dir/.env" || fail "not in $1 mode"; grep -q '^RCLI_DASHBOARD_PORT=127.0.0.1:3080$' "$dir/.env" || fail "Dashboard published beyond loopback"; }
run --domain rcli.example.com
mode domain
grep -q '^RCLI_API_ADDRESS=https://rcli.example.com$' "$dir/.env" || fail "domain: API Address"
grep -q '^rcli.example.com {' "$dir/Caddyfile" && grep -q '"443:443"' "$dir/compose.override.yaml" || fail "domain: Caddy on 443"
! grep -q 'RCLI_HTTPS_CERT_FILE' "$dir/compose.override.yaml" || fail "domain: still the CA certificate"
echo cert > "$t/c.pem" && echo key > "$t/k.pem"
run --domain rcli.example.com --cert "$t/c.pem" --key "$t/k.pem"
mode cert
cmp -s "$t/c.pem" "$dir/tls/https.crt" && cmp -s "$t/k.pem" "$dir/tls/https.key" || fail "cert: files not copied"
grep -q '^RCLI_HTTPS_PORT=8443$' "$dir/.env" && grep -q '^RCLI_API_ADDRESS=https://rcli.example.com:8443$' "$dir/.env" || fail "cert: port 8443"
grep -q 'tls /certs/https.crt' "$dir/Caddyfile" || fail "cert: Caddy does not serve the certificate"
echo renewed > "$t/c.pem"
run
mode cert
cmp -s "$t/c.pem" "$dir/tls/https.crt" || fail "a run with no argument did not keep the mode"
run --port 9444
mode cert
grep -q '^RCLI_API_ADDRESS=https://rcli.example.com:9444$' "$dir/.env" || fail "cert: --port alone did not move its port"
PATH="$t/bin:$PATH" RCLI_ROOT="$t/root" sh install.sh --ip 1.2.3.4 < /dev/null > "$t/out" 2>&1 && fail "--ip alone switched cert mode to IP mode"
grep -q -- '--behind-proxy' "$t/out" || fail "--ip alone: the five modes not listed"
mode cert
run --tunnel eyJhIjoidG9rZW4ifQ==
mode tunnel
grep -q '^RCLI_TUNNEL_TOKEN=eyJhIjoidG9rZW4ifQ==$' "$dir/.env" && grep -q 'cloudflared' "$dir/compose.override.yaml" || fail "tunnel: no cloudflared"
! grep -q 'ports' "$dir/compose.override.yaml" && [ ! -e "$dir/Caddyfile" ] || fail "tunnel: a Dashboard port is published"
grep -q '^RCLI_API_ADDRESS=$' "$dir/.env" || fail "tunnel: API Address not left to the first visit"
run --behind-proxy
mode proxy
[ ! -e "$dir/compose.override.yaml" ] || fail "behind-proxy: something bundled"
grep '^RCLI_\(BOOTSTRAP\|MASTER\|TURN_SECRET\|S3\)\|^POSTGRES' "$t/env1" | while read -r line; do
  grep -qx "$line" "$dir/.env" || fail "a switch changed ${line%%=*}"
done
! grep -q -- '-X POST.*/users' "$t/log" || fail "a switch created a User"

for bad in '--tunnel x --domain a.example' '--cert c.pem --key k.pem' '--domain a.example --cert c.pem' \
  '--domain a.example --port 9000' '--behind-proxy --port 3' '--ip 1.2.3.4 --domain a.example' '--bogus'; do
  # shellcheck disable=SC2086
  PATH="$t/bin:$PATH" RCLI_ROOT="$t/root" sh install.sh $bad < /dev/null > "$t/out" 2>&1 && fail "accepted $bad"
  grep -q -- '--behind-proxy' "$t/out" && grep -q -- '--tunnel' "$t/out" || fail "$bad: the five modes not listed"
done
mode proxy

# Ports set in .env stay, and what the script writes and prints follows them.
printf 'RCLI_AGENT_PORT=9555\nRCLI_DASHBOARD_PORT=127.0.0.1:4080\n' >> "$dir/.env"
: > "$t/log"
run
grep -q '^RCLI_DASHBOARD_PORT=127.0.0.1:4080$' "$dir/.env" && grep -q '^RCLI_AGENT_PORT=9555$' "$dir/.env" || fail "ports in .env not kept"
grep -q '^RCLI_AGENT_HOSTNAMES=198.51.100.4:9555$' "$dir/.env" || fail "Server Address not on RCLI_AGENT_PORT"
grep -q 'http://127.0.0.1:4080/api/v1/server-ca' "$t/log" || fail "API not called on RCLI_DASHBOARD_PORT"
grep -q '9555/tcp (Agents)' "$t/out" && grep -q '127.0.0.1:4080' "$t/out" && ! grep -q '9443\|3080' "$t/out" || fail "printed ports not the .env ones"

stub uname 'echo Darwin'
PATH="$t/bin:$PATH" RCLI_ROOT="$t/root2" sh install.sh < /dev/null > "$t/out" 2>&1 && fail "installed on macOS"
grep -q 'manual' "$t/out" || fail "no pointer to the manual steps"
[ ! -e "$t/root2" ] || fail "half-installed on macOS"
echo ok
