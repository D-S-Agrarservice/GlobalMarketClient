# Build environment for the self-hosted Linux CI and release jobs (job container).
# Built and pushed by .github/workflows/ci-image.yml; the tag is derived from this
# file's hash, so any change here produces a new pinned image.
# ubuntu:22.04 keeps the old glibc baseline of the ubuntu-22.04 hosted runner,
# so the AppImage/.deb still run on older distros.
FROM ubuntu:22.04

ARG DEBIAN_FRONTEND=noninteractive
RUN apt-get update \
 && apt-get install -y --no-install-recommends \
      ca-certificates curl wget file git xz-utils unzip build-essential pkg-config \
      libwebkit2gtk-4.1-dev \
      libappindicator3-dev \
      librsvg2-dev \
      patchelf \
      libxdo-dev \
      libssl-dev \
 && rm -rf /var/lib/apt/lists/*

# Rust lives in the image, not in $HOME: the runner mounts /github/home from the
# host and keeps it between jobs, which left half-updated toolchains behind.
# dtolnay/rust-toolchain still updates stable on top of this when needed.
ENV RUSTUP_HOME=/usr/local/rustup \
    CARGO_HOME=/usr/local/cargo \
    PATH=/usr/local/cargo/bin:$PATH
RUN curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs \
      | sh -s -- -y --no-modify-path --profile minimal --default-toolchain stable \
 && rustc --version
