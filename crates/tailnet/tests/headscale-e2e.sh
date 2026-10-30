#!/bin/sh
# End-to-end proof that MyMCPs reaches an HTTP MCP through a tailnet, with real
# software only, all in Docker, on a private network of its own:
#
#   - a Headscale (the open-source Tailscale control server),
#   - a "device": the real `tailscaled`, joined to it with a pre-auth key, that
#     serves a small streamable-HTTP MCP (headscale-e2e/mcp-device.js),
#   - the MyMCPs image, built from this checkout, which joins the same tailnet
#     with the real `tailscaled` of its image.
#
# The script drives MyMCPs over HTTP like a person would (onboarding, sign-in,
# the Tailnets page, the MCP dialog, an access token) and checks that the
# tailnet turns Connected, that an HTTP MCP with that network saves as ready
# and lists the tools of the device, that a call through the gateway reaches
# it, and that a host that is not a device of the tailnet is refused. It also
# follows the sign-in link of a tailnet up to the register page of Headscale,
# and finishes that registration.
#
# Nothing touches a Tailscale account. Needs Docker, curl, jq and the Internet
# (images, and the crates of the build, unless MYMCPS_IMAGE names an image).
#
#   crates/tailnet/tests/headscale-e2e.sh
#
# Environment:
#   MYMCPS_IMAGE     use this image instead of building one from the checkout
#   HEADSCALE_TAG    Headscale image tag (default below)
#   TAILSCALE_TAG    tailscale image tag of the device (default: the Dockerfile's)
#   E2E_KEEP=1       leave the containers running when it ends, to look around
set -eu

HEADSCALE_TAG="${HEADSCALE_TAG:-v0.26.1}"
TAILSCALE_TAG="${TAILSCALE_TAG:-v1.104.2}"
NODE_IMAGE="${NODE_IMAGE:-node:24.22.0-trixie-slim}"
MCP_PORT=8931
SUFFIX="tailnet.test"
DEVICE_NAME="e2e-device"

root=$(cd "$(dirname "$0")/../../.." && pwd)
here="$root/crates/tailnet/tests/headscale-e2e"
run="mymcps-e2e-$$"
network="$run"
control_url="http://$run-headscale:8080"
work=$(mktemp -d)
jar="$work/cookies"
started=$(date +%s)
checks=0

for tool in docker curl jq; do
  command -v "$tool" >/dev/null 2>&1 || { echo "$tool is required" >&2; exit 2; }
done

# ---------------------------------------------------------------------------
# Reporting and clean-up
# ---------------------------------------------------------------------------
step() { printf '\n== %s\n' "$*"; }
ok() { checks=$((checks + 1)); printf '  PASS %s\n' "$*"; }

dump_logs() {
  for name in headscale device mymcps; do
    if docker inspect "$run-$name" >/dev/null 2>&1; then
      printf '\n---- logs of %s (last 40 lines)\n' "$name" >&2
      docker logs --tail 40 "$run-$name" >&2 2>&1 || true
    fi
  done
}

fail() {
  printf '  FAIL %s\n' "$*" >&2
  dump_logs
  exit 1
}

cleanup() {
  status=$?
  if [ "${E2E_KEEP:-}" = 1 ]; then
    echo "E2E_KEEP=1: left $run-* running on network $network (web: ${base:-?})" >&2
  else
    # -v takes the anonymous volumes of the containers with them.
    docker rm -f -v "$run-mymcps" "$run-device" "$run-headscale" >/dev/null 2>&1 || true
    docker network rm "$network" >/dev/null 2>&1 || true
  fi
  [ "${E2E_KEEP:-}" = 1 ] || rm -rf "$work"
  seconds=$(($(date +%s) - started))
  if [ "$status" -eq 0 ]; then
    printf '\nALL GREEN: %s checks in %ss\n' "$checks" "$seconds"
  else
    printf '\nFAILED after %s checks, %ss\n' "$checks" "$seconds" >&2
  fi
}
trap cleanup EXIT

# Retries a command until it succeeds: `until <seconds> <what> <command...>`.
until_ok() {
  limit=$1
  what=$2
  shift 2
  deadline=$(($(date +%s) + limit))
  while ! "$@" >/dev/null 2>&1; do
    [ "$(date +%s)" -lt "$deadline" ] || fail "timed out after ${limit}s waiting for $what"
    sleep 1
  done
}

# ---------------------------------------------------------------------------
# The web client: a cookie jar and the CSRF token of the session
# ---------------------------------------------------------------------------
csrf=""
# GET, following redirects. A page that shows a flash message shows it once.
get() { curl -sS -b "$jar" -c "$jar" -L "$base$1"; }
# POST a form with the CSRF token, following the redirect like a browser.
post() {
  path=$1
  shift
  curl -sS -b "$jar" -c "$jar" -L "$base$path" --data-urlencode "_csrf=$csrf" "$@"
}
# The CSRF token of the session as of the page at $1.
refresh_csrf() {
  csrf=$(get "$1" | grep -o '<meta name="csrf-token" content="[^"]*"' | head -1 | sed 's/.*content="//; s/"$//')
  [ -n "$csrf" ] || fail "no CSRF token on $1"
}
# What a page shows, with the markup flattened into words.
text_of() { sed 's/<[^>]*>/ /g; s/&quot;/"/g; s/&#39;/'"'"'/g; s/&amp;/\&/g' | tr -s ' \n\t' ' '; }

# ---------------------------------------------------------------------------
# 1. Images
# ---------------------------------------------------------------------------
step "Images"
if [ -n "${MYMCPS_IMAGE:-}" ]; then
  image=$MYMCPS_IMAGE
  ok "using the MyMCPs image $image"
else
  image="mymcps-e2e:local"
  docker build -q -t "$image" "$root" >/dev/null || fail "docker build of the MyMCPs image"
  ok "built the MyMCPs image from this checkout ($image)"
fi

# The device: the real tailscaled and tailscale of the official image, next to
# Node.js for the MCP server.
device_image="mymcps-e2e-device:$TAILSCALE_TAG"
docker build -q -t "$device_image" - >/dev/null <<EOF || fail "docker build of the device image"
FROM tailscale/tailscale:$TAILSCALE_TAG AS tailscale
FROM $NODE_IMAGE
COPY --from=tailscale /usr/local/bin/tailscaled /usr/local/bin/tailscale /usr/local/bin/
EOF
ok "built the device image (tailscale $TAILSCALE_TAG, $NODE_IMAGE)"

# ---------------------------------------------------------------------------
# 2. Headscale
# ---------------------------------------------------------------------------
step "Headscale $HEADSCALE_TAG"
docker network create "$network" >/dev/null

cat >"$work/headscale.yaml" <<EOF
server_url: $control_url
listen_addr: 0.0.0.0:8080
metrics_listen_addr: 127.0.0.1:9090
grpc_listen_addr: 127.0.0.1:50443
grpc_allow_insecure: false
noise:
  private_key_path: /var/lib/headscale/noise_private.key
prefixes:
  v4: 100.64.0.0/10
  v6: fd7a:115c:a1e0::/48
  allocation: sequential
derp:
  server:
    enabled: true
    region_id: 999
    region_code: e2e
    region_name: E2E DERP
    stun_listen_addr: 0.0.0.0:3478
    private_key_path: /var/lib/headscale/derp_server_private.key
    automatically_add_embedded_derp_region: true
  urls: []
  paths: []
  auto_update_enabled: false
disable_check_updates: true
ephemeral_node_inactivity_timeout: 30m
database:
  type: sqlite
  sqlite:
    path: /var/lib/headscale/db.sqlite
policy:
  mode: file
  path: ""
dns:
  magic_dns: true
  base_domain: $SUFFIX
  override_local_dns: false
  nameservers:
    global: []
unix_socket: /var/run/headscale/headscale.sock
unix_socket_permission: "0770"
log:
  level: info
EOF

docker run -d --name "$run-headscale" --network "$network" \
  -p 127.0.0.1::8080 \
  -v "$work/headscale.yaml:/etc/headscale/config.yaml:ro" \
  -v /var/lib/headscale -v /var/run/headscale \
  "headscale/headscale:$HEADSCALE_TAG" serve >/dev/null
headscale_port=$(docker port "$run-headscale" 8080/tcp | head -1 | sed 's/.*://')
until_ok 60 "Headscale to answer" curl -sf "http://127.0.0.1:$headscale_port/health"
ok "Headscale answers (host port $headscale_port)"

hs() { docker exec "$run-headscale" headscale "$@"; }
hs users create e2e >/dev/null
user_id=$(hs users list -o json | jq -r '.[] | select(.name == "e2e") | .id')
[ -n "$user_id" ] || fail "no Headscale user id"
preauth() { hs preauthkeys create --user "$user_id" --expiration 1h | tail -1 | tr -d '[:space:]'; }
device_key=$(preauth)
mymcps_key=$(preauth)
[ -n "$device_key" ] && [ -n "$mymcps_key" ] || fail "no pre-auth key"
ok "created the user e2e and two one-off pre-auth keys"

# ---------------------------------------------------------------------------
# 3. The device: tailscaled joined with a pre-auth key, serving an MCP
# ---------------------------------------------------------------------------
step "Device: tailscaled and an MCP server"
docker run -d --name "$run-device" --network "$network" --hostname "$DEVICE_NAME" \
  --cap-add NET_ADMIN --device /dev/net/tun \
  -v "$here/mcp-device.js:/mcp-device.js:ro" \
  -e "PORT=$MCP_PORT" -e "AUTH_KEY=$device_key" -e "LOGIN_SERVER=$control_url" \
  --entrypoint sh "$device_image" -c '
    tailscaled --state=/tmp/tailscaled.state --socket=/tmp/tailscaled.sock &
    until tailscale --socket=/tmp/tailscaled.sock up --login-server="$LOGIN_SERVER" \
        --authkey="$AUTH_KEY" --hostname="'"$DEVICE_NAME"'" --netfilter-mode=off; do sleep 1; done
    exec node /mcp-device.js
  ' >/dev/null

device_state() {
  docker exec "$run-device" tailscale --socket=/tmp/tailscaled.sock status --json 2>/dev/null
}
device_running() { [ "$(device_state | jq -r .BackendState)" = Running ]; }
until_ok 90 "the device to join the tailnet" device_running
device_fqdn=$(device_state | jq -r '.Self.DNSName | rtrimstr(".")')
device_ip=$(device_state | jq -r '.Self.TailscaleIPs[0]')
[ "$device_fqdn" = "$DEVICE_NAME.$SUFFIX" ] || fail "unexpected device name: $device_fqdn"
ok "the device joined Headscale as $device_fqdn ($device_ip)"
until_ok 30 "the MCP server of the device" \
  docker exec "$run-device" node -e "require('node:net').connect($MCP_PORT, '127.0.0.1').on('connect', () => process.exit(0)).on('error', () => process.exit(1))"
ok "the MCP server listens on the device, port $MCP_PORT"

# ---------------------------------------------------------------------------
# 4. MyMCPs
# ---------------------------------------------------------------------------
step "MyMCPs"
# APP_URL names the origin the browser uses, so the host port is chosen first.
web_port=""
for attempt in 1 2 3 4 5 6 7 8; do
  candidate=$(awk -v seed="$$$attempt" 'BEGIN { srand(seed + systime()); print int(20000 + rand() * 30000) }' 2>/dev/null \
    || echo $((20000 + ($$ + attempt * 7919) % 30000)))
  if docker run -d --name "$run-mymcps" --network "$network" \
      -p "127.0.0.1:$candidate:3333" \
      -e "APP_URL=http://localhost:$candidate" \
      -e "TAILNET_CONTROL_URLS=$control_url" \
      "$image" >/dev/null 2>&1; then
    web_port=$candidate
    break
  fi
  docker rm -f "$run-mymcps" >/dev/null 2>&1 || true
done
[ -n "$web_port" ] || fail "no free host port for MyMCPs"
base="http://localhost:$web_port"
until_ok 60 "MyMCPs to answer" curl -sf "$base/health"
ok "MyMCPs is up at $base"

# ---------------------------------------------------------------------------
# 5. Sign in like a person
# ---------------------------------------------------------------------------
step "Onboarding and sign-in"
password="correct horse battery"
refresh_csrf /onboarding
page=$(post /onboarding --data-urlencode "fullName=E2E Admin" --data-urlencode "email=admin@example.com" \
  --data-urlencode "password=$password" --data-urlencode "passwordConfirmation=$password")
echo "$page" | grep -q 'href="/tailnets"' || fail "onboarding did not sign in an administrator with the Tailnets page: $(echo "$page" | text_of | head -c 400)"
ok "the first account is created and signed in (Tailnets is in the navigation)"

refresh_csrf /
post /logout >/dev/null
[ "$(curl -s -o /dev/null -w '%{http_code}' -b "$jar" -c "$jar" "$base/tailnets")" != 200 ] || fail "still signed in after logout"
refresh_csrf /login
page=$(post /login --data-urlencode "email=admin@example.com" --data-urlencode "password=$password")
echo "$page" | grep -q 'href="/tailnets"' || fail "sign-in failed"
refresh_csrf /tailnets
ok "signed out and in again with the password"

# ---------------------------------------------------------------------------
# 6. Connect the tailnet with an auth key
# ---------------------------------------------------------------------------
step "Tailnets: connect the Headscale with an auth key"
page=$(get /tailnets | text_of)
echo "$page" | grep -qi 'Tailscale is off' && fail "Tailscale is off in the image (no tailscaled)"
page=$(post /tailnets --data-urlencode "name=lab" --data-urlencode "method=key" \
  --data-urlencode "authKey=$mymcps_key" --data-urlencode "controlUrl=$control_url" | text_of)
echo "$page" | grep -q 'Tailnet connected' || fail "the Connect form was refused: $(echo "$page" | grep -o 'Could not connect[^.]*' | head -1)"
ok "the form was accepted (\"Tailnet connected\")"

# The row of a tailnet as words: its name, status and device.
tailnet_row() { get /tailnets | text_of | grep -o "$1 \(Headscale[^—]*\)\{0,1\}" | head -1; }
row_has() { tailnet_row "$1" | grep -q "$2"; }
until_ok 90 "the tailnet row to reach Connected" row_has lab Connected
row=$(tailnet_row lab)
ok "the tailnet row reaches Connected: $row"
echo "$row" | grep -q "mymcps-lab.$SUFFIX" || fail "the device name of MyMCPs is not shown: $row"
ok "the row names the device of MyMCPs (mymcps-lab.$SUFFIX)"

# Headscale agrees: both nodes are in its list, with the names it gave them.
nodes=$(hs nodes list -o json | jq -r '[.[] | .name] | sort | join(" ")')
echo "$nodes" | grep -q "mymcps-lab" && echo "$nodes" | grep -q "$DEVICE_NAME" || fail "Headscale lists: $nodes"
ok "Headscale lists both nodes: $nodes"

tailnet_id=$(get "/mcps/new?template=custom" | grep -o '<option value="[0-9]*">Tailnet: lab</option>' | head -1 | sed 's/<option value="\([0-9]*\)".*/\1/')
[ -n "$tailnet_id" ] || fail "the MCP dialog has no Network choice for the tailnet"
ok "the MCP dialog offers Network = Tailnet: lab (id $tailnet_id)"

# The embedded DERP relay of a Headscale that is not behind TLS cannot be used
# by Tailscale clients (they only speak TLS to it), so peers connect directly
# on the Docker network. Let the device and MyMCPs find that path before the
# first request, as a relay would have carried it; a real tailnet needs none.
mymcps_ip=$(hs nodes list -o json | jq -r '.[] | select(.name == "mymcps-lab") | .ip_addresses[0]')
docker exec "$run-device" tailscale --socket=/tmp/tailscaled.sock ping --until-direct --c 30 --timeout 3s "$mymcps_ip" >/dev/null 2>&1 \
  || fail "the device found no direct path to MyMCPs at $mymcps_ip"
ok "the device and MyMCPs reach each other directly ($mymcps_ip)"

# ---------------------------------------------------------------------------
# 7. An HTTP MCP on that tailnet
# ---------------------------------------------------------------------------
step "MCPs on the tailnet"
create_mcp() { # name url
  post /mcps --data-urlencode "name=$1" --data-urlencode "transport=http" \
    --data-urlencode "httpUrl=$2" --data-urlencode "tailnetId=$tailnet_id" \
    --data-urlencode "authType=auto" --data-urlencode "enabled=on" | text_of
}
# `status|tools|endpoint and message` of the row of the MCP called $1 on the list.
mcp_row() {
  row=$(get "/mcps?q=$1" | tr '\n' ' ' | grep -o '<tr><td><span class="status.*' | sed 's/<\/tr>.*//' | head -1)
  status=$(echo "$row" | grep -o 'class="status status--[a-z]*">[a-z]*' | head -1 | sed 's/.*>//')
  tools=$(echo "$row" | grep -o 'tabular">[0-9]*' | head -1 | sed 's/.*>//')
  printf '%s|%s|%s\n' "$status" "${tools:--}" "$(echo "$row" | text_of)"
}

expect_ready() { # name url what
  page=$(create_mcp "$1" "$2")
  echo "$page" | grep -q 'MCP created' || fail "the MCP form was refused: $(echo "$page" | grep -o 'The httpUrl[^.]*\|MCP URL[^.]*' | head -1)"
  row=$(mcp_row "$1")
  status=$(echo "$row" | cut -d'|' -f1)
  tools=$(echo "$row" | cut -d'|' -f2)
  { [ "$status" = ready ] && [ "$tools" -gt 0 ]; } 2>/dev/null || fail "$3: $2 is not ready with tools: $row"
  ok "$3: $2 saves as ready, with $tools tools"
}
expect_ready e2e-short "http://$DEVICE_NAME:$MCP_PORT/mcp" "short MagicDNS name"
expect_ready e2e-fqdn "http://$device_fqdn:$MCP_PORT/mcp" "full MagicDNS name"
expect_ready e2e-ip "http://$device_ip:$MCP_PORT/mcp" "Tailscale address"
mcp_row e2e-short | grep -q 'via tailnet lab' || fail "the list does not name the tailnet under the URL"
ok "the MCP list names the tailnet under the URL (via tailnet lab)"
mcp_row e2e-short | grep -q '|2|' || fail "expected the 2 tools of the device: $(mcp_row e2e-short)"
ok "the MCP lists the 2 tools of the device"

# ---------------------------------------------------------------------------
# 8. A call through the gateway
# ---------------------------------------------------------------------------
step "Gateway: a call that crosses the tailnet"
refresh_csrf /tokens
page=$(post /tokens --data-urlencode "name=e2e" --data-urlencode "scopeMode=all")
token=$(echo "$page" | grep -o 'id="created-token">[^<]*' | sed 's/.*>//')
[ -n "$token" ] || fail "no access token was shown"
ok "created an access token"

# One JSON-RPC request to the gateway; the answer is an SSE message or JSON.
rpc() {
  out=$(curl -sS -X POST "$base/mcp" -H "Authorization: Bearer $token" \
    -H 'Content-Type: application/json' -H 'Accept: application/json, text/event-stream' -d "$1")
  data=$(printf '%s\n' "$out" | sed -n 's/^data: //p')
  printf '%s\n' "${data:-$out}"
}
rpc '{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-03-26","capabilities":{},"clientInfo":{"name":"e2e","version":"1"}}}' \
  | jq -e '.result.serverInfo.name == "mymcps"' >/dev/null || fail "the gateway did not answer initialize"
tools=$(rpc '{"jsonrpc":"2.0","id":2,"method":"tools/list"}' | jq -r '[.result.tools[].name] | join(" ")')
echo "$tools" | grep -q 'e2e-short__whoami' && echo "$tools" | grep -q 'e2e-short__echo' || fail "the gateway lists: $tools"
ok "tools/list through the gateway names the tools of the device: $(echo "$tools" | tr ' ' '\n' | grep '^e2e-short__' | tr '\n' ' ')"

answer=$(rpc '{"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"e2e-short__echo","arguments":{"text":"across the tailnet"}}}' \
  | jq -r '.result.content[0].text // .error.message')
[ "$answer" = "device:$DEVICE_NAME echo:across the tailnet" ] || fail "the tool call answered: $answer"
ok "tools/call through the gateway reached the device and answered \"$answer\""

# The device saw MyMCPs come in over the tailnet: its peer is a tailnet address.
docker logs "$run-device" 2>&1 | grep -q '^100\.64\.\|^::ffff:100\.64\.\|^fd7a:' \
  || fail "the MCP server of the device saw no request from a tailnet address"
ok "the MCP server of the device saw the requests come from a tailnet address ($(docker logs "$run-device" 2>&1 | grep 'tools/call' | tail -1 | cut -d' ' -f1))"

# ---------------------------------------------------------------------------
# 9. Hosts that are not devices of the tailnet
# ---------------------------------------------------------------------------
step "A host that is not a device of the tailnet is refused"
docker_ip=$(docker inspect -f "{{(index .NetworkSettings.Networks \"$network\").IPAddress}}" "$run-device")
expect_refused() { # name url what
  create_mcp "$1" "$2" >/dev/null
  row=$(mcp_row "$1")
  case "$row" in
    error\|*"is not a device or a subnet route of tailnet"*) ok "$3: $2 is refused: $(echo "$row" | grep -o '"[^"]*" is not a device or a subnet route of tailnet "lab"' | head -1)" ;;
    *) fail "$3: $2 was not refused with the tailnet message: $row" ;;
  esac
}
expect_refused e2e-nodevice "http://not-a-device:$MCP_PORT/mcp" "an unknown name"
expect_refused e2e-public "http://example.com/mcp" "a public host"
expect_refused e2e-docker "http://$docker_ip:$MCP_PORT/mcp" "the Docker address of the device, outside the tailnet"
expect_refused e2e-nobody "http://100.64.0.99:$MCP_PORT/mcp" "a tailnet address that no device has"

# The controls: the same two URLs of the device without the tailnet. Its name
# means nothing on the internet, and its tailnet address is not a public one.
# It is the tailnet that made the MCPs above work.
create_on_internet() { # name url
  post /mcps --data-urlencode "name=$1" --data-urlencode "transport=http" \
    --data-urlencode "httpUrl=$2" --data-urlencode "tailnetId=" \
    --data-urlencode "authType=auto" --data-urlencode "enabled=on" | text_of
}
page=$(create_on_internet e2e-internet-name "http://$DEVICE_NAME:$MCP_PORT/mcp")
echo "$page" | grep -q 'MCP created' && fail "the short name of the device was accepted on Network = Internet"
echo "$page" | grep -q 'must be a valid URL' || fail "unexpected answer for the short name on the internet: $page"
ok "the short name is not a URL on Network = Internet (only a tailnet takes it)"
page=$(create_on_internet e2e-internet-ip "http://$device_ip:$MCP_PORT/mcp")
if echo "$page" | grep -q 'MCP created'; then
  row=$(mcp_row e2e-internet-ip)
  case "$row" in
    error\|*) ok "the tailnet address on Network = Internet is not reachable: $(echo "$row" | cut -d'|' -f3 | sed 's/.*8931\/mcp//; s/ auto.*//')" ;;
    *) fail "the device was reached without the tailnet: $row" ;;
  esac
else
  ok "the tailnet address is refused on Network = Internet: $(echo "$page" | grep -o 'MCP URL[^.]*\|The httpUrl[^.]*' | head -1)"
fi

# Nothing of the refused ones is reachable through the gateway, and the good
# one still is.
tools=$(rpc '{"jsonrpc":"2.0","id":4,"method":"tools/list"}' | jq -r '[.result.tools[].name] | join(" ")')
echo "$tools" | grep -q 'e2e-docker__' && fail "a refused MCP lists tools: $tools"
echo "$tools" | grep -q 'e2e-short__echo' || fail "the MCP of the device no longer lists its tools: $tools"
ok "refused MCPs list no tools, and the MCP of the device still does"

# ---------------------------------------------------------------------------
# 10. The sign-in link, up to the register page of Headscale
# ---------------------------------------------------------------------------
step "Tailnets: connect with a sign-in link"
refresh_csrf /tailnets
page=$(post /tailnets --data-urlencode "name=link" --data-urlencode "method=link" --data-urlencode "controlUrl=$control_url" | text_of)
echo "$page" | grep -q 'Tailnet added' || fail "the Connect form was refused: $page"
ok "the form was accepted (\"Tailnet added: use its Sign in link\")"

link_url() { get /tailnets | grep -o 'href="http[^"]*/register/[^"]*"' | head -1 | sed 's/^href="//; s/"$//; s/&amp;/\&/g'; }
until_ok 60 "the sign-in link" sh -c 'test -n "$1"' _ "$(link_url)"
until_ok 60 "the sign-in link" row_has link "Needs sign-in"
link=$(link_url)
[ -n "$link" ] || fail "the row has no Sign in link"
case "$link" in
  "$control_url"/register/*) ok "the row says Needs sign-in and links to the register page of Headscale: $link" ;;
  *) fail "the sign-in link is not on the Headscale: $link" ;;
esac

# The link names the Docker network host: open it through the published port.
register_key=${link##*/register/}
register_page=$(curl -sS "http://127.0.0.1:$headscale_port/register/$register_key")
echo "$register_page" | grep -q "headscale nodes register" && echo "$register_page" | grep -q "$register_key" \
  || fail "the register page of Headscale did not show the command: $register_page"
ok "the register page of Headscale answers with the command to run for this node"

# Finish what the page asks: register the node as the Headscale admin would.
hs nodes register --user "$user_id" --key "$register_key" >/dev/null 2>&1 \
  || hs nodes register --user e2e --key "$register_key" >/dev/null 2>&1 \
  || fail "headscale nodes register failed"
until_ok 90 "the signed-in tailnet row to reach Connected" row_has link Connected
ok "after the registration the row turns Connected by itself: $(tailnet_row link)"
