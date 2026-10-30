# Production image for self-hosted MyMCPs.
#
# Holds the `mymcps` server binary, and Node.js with its npm, so npm-transport
# MCPs work out of the box (see crates/node), and `tailscaled`, which the
# server starts for the Tailscale tailnets a team connects. The server runs
# npm MCPs in a sandbox of the kernel (crates/sandbox) that needs no
# privilege: the container can drop every capability, as docker-compose.yml
# does.
#
# Build:  docker build -t mymcps .
# Run:    docker run --rm -p 3333:3333 \
#           -e APP_URL=http://localhost:3333 \
#           -v mymcps-data:/app/tmp mymcps

# Base images are pinned by tag and multi-arch index digest, both written out
# in each FROM line (not behind an ARG) so Dependabot can update them.

# Official Node.js image (multi-arch), for its `node` binary and its npm. It is
# built on the Debian release of the runtime stage, whose C++ library the
# binary loads.
FROM node:26.12.0-trixie-slim@sha256:e556a144463cc8dc53afe8de6d2fa0db5c132fd39ffa232c17641bd40df2bf11 AS node

# Official Tailscale image (multi-arch), for its `tailscaled` daemon. The
# binary is a static Go program: it runs on the Debian runtime although the
# image is built on Alpine. Only `tailscaled` is copied: not the `tailscale`
# command, not `containerboot`.
FROM tailscale/tailscale:v1.104.2@sha256:b11583ad2e93e4671128538c88cf11f460444a5d7e449311dd1933a400ab849d AS tailscale

# ---------------------------------------------------------------------------
# Compile the server
# ---------------------------------------------------------------------------
# Runs on the architecture of the builder and cross-compiles when the image is
# for another one, so a multi-arch build never compiles under emulation.
#
# The Rust version in this tag is the toolchain of the project: the workflows
# install the same one (crates/xtask/tests/container_image.rs checks it), and
# its Debian release is the one the runtime stage runs on.
FROM --platform=$BUILDPLATFORM rust:1.99.1-slim-trixie@sha256:e6cd1d294a300601eaaa4a294b337e5f0826f427fb2d481e9f76e69e7981e244 AS build

ARG TARGETARCH

# The Rust target of the image. When the builder has another architecture, the
# C code of the workspace (SQLite, the TLS library) and the final link need a
# cross compiler and the C library of the target.
RUN set -eu; \
  case "$TARGETARCH" in \
    amd64) target=x86_64-unknown-linux-gnu; gnu=x86_64-linux-gnu; cross_gcc=gcc-x86-64-linux-gnu ;; \
    arm64) target=aarch64-unknown-linux-gnu; gnu=aarch64-linux-gnu; cross_gcc=gcc-aarch64-linux-gnu ;; \
    *) echo "MyMCPs images are built for amd64 and arm64, not $TARGETARCH." >&2; exit 1 ;; \
  esac; \
  echo "$target" > /rust-target; \
  printf '[build]\ntarget = "%s"\n' "$target" > "$CARGO_HOME/config.toml"; \
  if [ "$(dpkg --print-architecture)" != "$TARGETARCH" ]; then \
    apt-get update; \
    apt-get install -y --no-install-recommends "$cross_gcc" "libc6-dev-$TARGETARCH-cross"; \
    rm -rf /var/lib/apt/lists/*; \
    rustup target add "$target"; \
    target_env="$(echo "$target" | tr - _)"; \
    printf '\n[target.%s]\nlinker = "%s-gcc"\n\n[env]\nCC_%s = "%s-gcc"\nAR_%s = "%s-ar"\n' \
      "$target" "$gnu" "$target_env" "$gnu" "$target_env" "$gnu" >> "$CARGO_HOME/config.toml"; \
  fi

WORKDIR /src

# Only what Cargo reads (see .dockerignore).
COPY Cargo.toml Cargo.lock ./
COPY .cargo ./.cargo
COPY crates ./crates

# The downloaded crates and the compiled dependencies are kept in cache mounts
# of the builder, so a rebuild does not compile them again. --locked refuses a
# Cargo.lock that does not match the manifests.
#
# The crates of the workspace are always compiled again: Cargo trusts a cached
# artifact that is newer than its sources, and the files of a build context
# can be older than the previous build and still differ from what it compiled
# (an archive, a restored checkout). The touch rules that out.
RUN --mount=type=cache,id=mymcps-cargo-registry,target=/usr/local/cargo/registry,sharing=locked \
  --mount=type=cache,id=mymcps-cargo-git,target=/usr/local/cargo/git,sharing=locked \
  --mount=type=cache,id=mymcps-cargo-target,target=/src/target,sharing=locked \
  find crates -type f -exec touch {} + \
  && cargo build --release --locked --bin mymcps \
  && mkdir /out \
  && cp "target/$(cat /rust-target)/release/mymcps" /out/mymcps

# ---------------------------------------------------------------------------
# Runtime
# ---------------------------------------------------------------------------
FROM debian:trixie-slim@sha256:62bd423aeee912639ca8ee681c2b762cb3e25137e0a92b20be455898a9f034ad AS runtime

# NODE_VERSION and TAILSCALE_VERSION must name the versions in the node and
# tailscale tags above; Dependabot only edits the FROM lines
# (crates/xtask/tests/container_image.rs checks them).
ENV APP_ENV=production \
  HOST=0.0.0.0 \
  PORT=3333 \
  DATA_DIR=/app/tmp \
  NODE_BINARY=/usr/local/bin/node \
  NODE_VERSION=24.22.0 \
  TAILSCALED_BINARY=/usr/local/bin/tailscaled \
  TAILSCALE_VERSION=1.104.2

# ca-certificates: the server verifies upstream MCPs and OAuth providers
# against the system trust store. dumb-init: see ENTRYPOINT.
#
# The server runs as uid and gid 1000, the ids of the `node` user of the
# images built before the server was rewritten in Rust: a volume created by
# one of those keeps working. The data directory is created private to that
# user, which is what a new volume starts from.
RUN apt-get update \
  && apt-get install -y --no-install-recommends ca-certificates dumb-init \
  && rm -rf /var/lib/apt/lists/* \
  && groupadd --gid 1000 mymcps \
  && useradd --uid 1000 --gid 1000 --create-home --shell /usr/sbin/nologin mymcps \
  && install -d -o 1000 -g 1000 -m 0700 /app/tmp /app/tmp/mcp-sandboxes /app/tmp/mcp-packages

# The binary, and the npm it runs to install a package: nothing else of the
# image, which also holds yarn and corepack.
COPY --from=node /usr/local/bin/node /usr/local/bin/node
COPY --from=node /usr/local/lib/node_modules/npm /usr/local/lib/node_modules/npm
COPY --from=tailscale /usr/local/bin/tailscaled /usr/local/bin/tailscaled
COPY --from=build /out/mymcps /usr/local/bin/mymcps

WORKDIR /app

USER 1000:1000

EXPOSE 3333

# The binary checks its own /health, so the image needs no curl or wget.
HEALTHCHECK --interval=30s --timeout=5s --start-period=30s --retries=3 \
  CMD ["mymcps", "healthcheck"]

# Persist SQLite (tmp/db.sqlite3), the generated key, and the sandboxes and
# installed packages of npm MCPs across restarts.
VOLUME ["/app/tmp"]

# dumb-init reaps the processes of npm MCPs. There is no entrypoint script: the
# server sets its own umask, creates or reads its key, and migrates the
# database when it starts.
ENTRYPOINT ["dumb-init", "--"]
CMD ["mymcps", "serve"]
