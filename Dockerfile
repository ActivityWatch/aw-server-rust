# Build stage: aw-webui (node) + aw-server (rust, with the webui embedded via rust-embed)
FROM rust:1-bookworm AS builder

RUN apt-get update -qq -y && \
    apt-get install -qq -y --no-install-recommends \
        build-essential pkg-config libssl-dev ca-certificates curl git make gnupg && \
    curl -fsSL https://deb.nodesource.com/setup_22.x | bash - && \
    apt-get install -qq -y --no-install-recommends nodejs && \
    rm -rf /var/lib/apt/lists/*

WORKDIR /app/aw-server-rust
COPY . .

# `make aw-server` builds the webui (aw-webui/dist) before cargo, so that
# aw-server's build.rs can embed the assets into the binary.
# We avoid the `build` target, which also builds aw-sync: that binary pulls in
# openssl with the "vendored" feature (builds OpenSSL from source) and is not
# used by this image.
RUN make aw-server

FROM debian:bookworm-slim

RUN apt-get update -qq -y && \
    apt-get install -qq -y --no-install-recommends \
        libssl3 && \
    rm -rf /var/lib/apt/lists/*

COPY --from=builder /app/aw-server-rust/target/release/aw-server /usr/local/bin/aw-server-rust
COPY --from=builder /app/aw-server-rust/aw-webui/dist /usr/local/share/aw-webui

EXPOSE 5600

CMD ["aw-server-rust"]
