# Reproducible Linux build environment for the atomik-kb desktop .deb.
# Ubuntu 22.04 on purpose: the oldest glibc (2.35) we support, so the binary runs on
# 22.04, 24.04, Debian 12 and anything newer. Driven by scripts/package/linux.ps1, which
# passes the versions below from rust-toolchain.toml and package.json (the defaults
# only matter for a manual `docker build`).
FROM ubuntu:22.04

ARG RUST_VERSION=1.99.0
ARG NODE_VERSION=25.5.0
ARG PNPM_VERSION=12.8.1

ENV DEBIAN_FRONTEND=noninteractive \
    LANG=C.UTF-8 \
    RUSTUP_HOME=/opt/rustup \
    CARGO_HOME=/opt/cargo \
    PATH=/opt/cargo/bin:/opt/node/bin:$PATH \
    CARGO_NET_GIT_FETCH_WITH_CLI=true

# The Windows host has no working IPv6 (dual-stack connects fail); never let apt try it.
RUN echo 'Acquire::ForceIPv4 "true";' > /etc/apt/apt.conf.d/99force-ipv4 \
 && apt-get update \
 && apt-get install -y --no-install-recommends \
      build-essential pkg-config perl make \
      curl ca-certificates xz-utils git file patchelf \
      libwebkit2gtk-4.1-dev libgtk-3-dev librsvg2-dev libssl-dev \
      libayatana-appindicator3-dev \
 && rm -rf /var/lib/apt/lists/*
# perl + make + cc build the vendored OpenSSL inside SQLCipher; no NASM needed on Linux.
# zbus (Secret Service) is pure Rust: no libsecret / libdbus at build time.

# Node (official tarball, pinned) and pnpm (pinned, from package.json "packageManager").
RUN curl -4fsSL "https://nodejs.org/dist/v${NODE_VERSION}/node-v${NODE_VERSION}-linux-x64.tar.xz" \
      | tar -xJ -C /opt \
 && mv "/opt/node-v${NODE_VERSION}-linux-x64" /opt/node \
 && npm install -g "pnpm@${PNPM_VERSION}" \
 && node --version && pnpm --version

# Rust, with the components and target rust-toolchain.toml asks for, so cargo never
# tries to download anything on first use.
RUN curl -4fsSL https://sh.rustup.rs | sh -s -- -y --no-modify-path --profile minimal \
      --default-toolchain "${RUST_VERSION}" -c clippy -c rustfmt -t wasm32-unknown-unknown \
 && rustc --version && cargo --version

WORKDIR /build
