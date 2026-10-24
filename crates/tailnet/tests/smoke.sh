#!/bin/sh
# Runs the ignored smoke test of mymcps-tailnet against the real tailscaled, in
# a Linux container: the binary is copied out of the official tailscale image
# (a static Go executable), and the crate is tested in the Rust image of the
# Dockerfile. Needs Docker and the Internet.
#
#   crates/tailnet/tests/smoke.sh
set -eu

TAILSCALE_TAG="${TAILSCALE_TAG:-v1.104.2}"
RUST_IMAGE="${RUST_IMAGE:-rust:1.99.1-slim-trixie}"

root=$(cd "$(dirname "$0")/../../.." && pwd)
out=$(mktemp -d)
trap 'rm -rf "$out"' EXIT

docker run --rm --entrypoint cp -v "$out:/out" "tailscale/tailscale:$TAILSCALE_TAG" \
  /usr/local/bin/tailscaled /out/tailscaled

# The checkout is mounted read-only and nothing is built in it; the Cargo
# registry is kept in a volume between runs.
docker run --rm \
  -v "$root:/work:ro" \
  -v "$out:/ts:ro" \
  -v mymcps-tailnet-cargo:/usr/local/cargo/registry \
  -e CARGO_TARGET_DIR=/tmp/target \
  -e TAILSCALED_BIN=/ts/tailscaled \
  -w /work \
  "$RUST_IMAGE" \
  cargo test --locked -p mymcps-tailnet --test smoke -- --ignored --nocapture
