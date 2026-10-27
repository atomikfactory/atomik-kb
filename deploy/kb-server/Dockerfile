# syntax=docker/dockerfile:1
#
# kb-server, the zero-knowledge sync server of atomik-kb.
# Build from the repository root (the workspace is the build context):
#
#   docker build -f deploy/kb-server/Dockerfile -t atomik-kb-server .
#
# The runtime image is distroless (no shell): a glibc base, the binary, and /data.

# ----- build -----------------------------------------------------------------------------------
FROM rust:1-bookworm AS builder
WORKDIR /src
# The whole workspace: Cargo needs every member's manifest to resolve. The toolchain is
# the one rust-toolchain.toml pins (rustup installs it on first use). Perl, make and a C
# compiler (all in this image) build the SQLite and OpenSSL that rusqlite vendors.
COPY . .
RUN --mount=type=cache,target=/usr/local/cargo/registry \
    --mount=type=cache,target=/usr/local/rustup \
    --mount=type=cache,target=/src/target \
    cargo build --release --locked -p kb-server \
 && install -D -m 0755 target/release/kb-server /out/kb-server \
 && install -d -o 65532 -g 65532 -m 0700 /out/data

# ----- runtime ---------------------------------------------------------------------------------
FROM gcr.io/distroless/cc-debian12:nonroot
COPY --from=builder /out/kb-server /usr/local/bin/kb-server
# Owned by the image's non-root user, so a fresh named volume mounted here is writable.
COPY --from=builder --chown=65532:65532 /out/data /data
ENV KB_SERVER_LISTEN=0.0.0.0:8787 \
    KB_SERVER_DATA=/data \
    KB_SERVER_ACCOUNT_QUOTA_BYTES=10737418240 \
    KB_SERVER_PRUNE_GRACE_DAYS=7 \
    RUST_LOG=info
# KB_SERVER_INVITE_TOKENS has no default: set it to create the first account.
VOLUME ["/data"]
EXPOSE 8787
USER nonroot
ENTRYPOINT ["/usr/local/bin/kb-server"]
