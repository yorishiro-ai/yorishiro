# syntax=docker/dockerfile:1.29.0

# Build against AlmaLinux 10's glibc for package compatibility.
FROM almalinux:10

RUN dnf install -y gcc gcc-c++ openssl-devel pkgconf-pkg-config git binutils \
    && dnf clean all

ENV PATH=/usr/local/cargo/bin:$PATH
ENV RUSTUP_HOME=/usr/local/rustup
# The host's sccache and mold are not part of this build image.
ENV RUSTC_WRAPPER=""
ENV RUSTFLAGS="-C link-arg=-fuse-ld=bfd"
