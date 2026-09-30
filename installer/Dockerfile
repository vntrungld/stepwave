# makensis for hosts without NSIS (e.g. Arch/CachyOS, where it is AUR-only).
FROM ubuntu:24.04
RUN apt-get update \
    && apt-get install -y --no-install-recommends nsis \
    && rm -rf /var/lib/apt/lists/*
