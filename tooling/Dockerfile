FROM python:3.11-slim@sha256:e031123e3d85762b141ad1cbc56452ba69c6e722ebf2f042cc0dc86c47c0d8b3
RUN apt-get update && apt-get install -y --no-install-recommends \
 clang-19 lld-19 llvm-19 build-essential git curl ca-certificates perl pkg-config \
 bison flex bc libelf-dev libssl-dev libusb-1.0-0-dev rsync xz-utils unzip 7zip \
 erofs-utils device-tree-compiler u-boot-tools qemu-user openssl zlib1g-dev \
 liblz4-dev libzstd-dev && rm -rf /var/lib/apt/lists/*
RUN pip install --no-cache-dir paramiko==5.1.0 cryptography==50.0.2 pyusb==1.3.1
ENV RUSTUP_HOME=/opt/rustup CARGO_HOME=/opt/cargo
ENV PATH=/opt/cargo/bin:$PATH
RUN curl --proto '=https' --tlsv1.2 -fsSL https://sh.rustup.rs -o /tmp/rustup.sh \
 && sh /tmp/rustup.sh -y --profile minimal --default-toolchain 1.100.0 --component rust-src \
 && rm /tmp/rustup.sh
WORKDIR /input
