# Production image for self-hosted MyMCPs.
#
# Holds the `mymcps` server binary and Deno, so npm-transport MCP sandboxes
# work out of the box (see crates/deno).
#
# Build:  docker build -t mymcps .
# Run:    docker run --rm -p 3333:3333 \
#           -e APP_URL=http://localhost:3333 \
#           -v mymcps-data:/app/tmp mymcps

# Base images are pinned by tag and multi-arch index digest, both written out
# in each FROM line (not behind an ARG) so Dependabot can update them.

# Official Deno binary only (multi-arch).
FROM denoland/deno:bin-2.9.7@sha256:bc5aa4466e21b6d3021226a85ba2e1911f7c386254d97b9d797903ab74edace2 AS deno

# ---------------------------------------------------------------------------
# Compile the server
# ---------------------------------------------------------------------------
# Runs on the architecture of the builder and cross-compiles when the image is
# for another one, so a multi-arch build never compiles under emulation.
#
# The Rust version in this tag is the toolchain of the project: the workflows
# install the same one (crates/xtask/tests/container_image.rs checks it), and
# its Debian release is the one the runtime stage runs on.
FROM --platform=$BUILDPLATFORM rust:1.98.1-slim-trixie@sha256:4cd829461bd5c4d511c32e269da9cb8929223b666519d8004e35fc8d1d771ab7 AS build

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
FROM debian:trixie-slim@sha256:a29215f6a35e51e22adffa17f89e9d2ef06214e64a2bad10d765c46aea49f11f AS runtime

# DENO_VERSION must name the version in the denoland/deno tag above; Dependabot
# only edits the FROM line (crates/xtask/tests/container_image.rs checks it).
ENV APP_ENV=production \
  HOST=0.0.0.0 \
  PORT=3333 \
  DATA_DIR=/app/tmp \
  DENO_PATH=/usr/local/bin/deno \
  DENO_DIR=/app/tmp/deno-cache \
  DENO_VERSION=2.9.7

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
  && install -d -o 1000 -g 1000 -m 0700 /app/tmp /app/tmp/mcp-sandboxes /app/tmp/deno-cache

COPY --from=deno /deno /usr/local/bin/deno
COPY --from=build /out/mymcps /usr/local/bin/mymcps

WORKDIR /app

USER 1000:1000

EXPOSE 3333

# The binary checks its own /health, so the image needs no curl or wget.
HEALTHCHECK --interval=30s --timeout=5s --start-period=30s --retries=3 \
  CMD ["mymcps", "healthcheck"]

# Persist SQLite (tmp/db.sqlite3), the generated key and the Deno MCP
# sandboxes across restarts.
VOLUME ["/app/tmp"]

# dumb-init reaps Deno MCP child processes. There is no entrypoint script: the
# server sets its own umask, creates or reads its key, and migrates the
# database when it starts.
ENTRYPOINT ["dumb-init", "--"]
CMD ["mymcps", "serve"]
