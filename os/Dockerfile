FROM debian:bookworm-slim

ENV DEBIAN_FRONTEND=noninteractive \
    FORCE_UNSAFE_CONFIGURE=1

RUN apt-get update && apt-get install -y --no-install-recommends \
    bc \
    bison \
    build-essential \
    bzip2 \
    ca-certificates \
    cpio \
    curl \
    file \
    flex \
    gawk \
    git \
    gzip \
    libelf-dev \
    libncurses-dev \
    libssl-dev \
    locales \
    make \
    patch \
    perl \
    python3 \
    rsync \
    unzip \
    wget \
    xz-utils \
    zstd \
 && sed -i 's/# en_US.UTF-8 UTF-8/en_US.UTF-8 UTF-8/' /etc/locale.gen \
 && locale-gen \
 && rm -rf /var/lib/apt/lists/*

ENV LANG=en_US.UTF-8 \
    LC_ALL=en_US.UTF-8

WORKDIR /input
