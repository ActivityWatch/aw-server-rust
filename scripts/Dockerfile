# Build stage: aw-webui (node) + aw-server (rust, with the webui embedded via rust-embed)
# Node is taken from the official image instead of the NodeSource install script,
# so no remotely fetched script is executed as root in the stage that produces
# the artifacts shipped in the final image.
FROM node:22-bookworm-slim AS node

FROM rust:1-bookworm AS builder

RUN apt-get update -qq -y && \
    apt-get install -qq -y --no-install-recommends \
        build-essential pkg-config libssl-dev ca-certificates git make && \
    rm -rf /var/lib/apt/lists/*

COPY --from=node /usr/local/bin/node /usr/local/bin/node
COPY --from=node /usr/local/lib/node_modules /usr/local/lib/node_modules
RUN ln -s ../lib/node_modules/npm/bin/npm-cli.js /usr/local/bin/npm && \
    ln -s ../lib/node_modules/npm/bin/npx-cli.js /usr/local/bin/npx

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

RUN groupadd --gid 10001 aw && \
    useradd --uid 10001 --gid 10001 --home-dir /home/aw --create-home \
        --shell /usr/sbin/nologin aw && \
    mkdir -p /home/aw/.local/share/activitywatch /home/aw/.config/activitywatch && \
    chown -R aw:aw /home/aw

COPY --from=builder /app/aw-server-rust/target/release/aw-server /usr/local/bin/aw-server-rust
COPY --from=builder /app/aw-server-rust/aw-webui/dist /usr/local/share/aw-webui

USER aw
# Set explicitly so the datastore path does not depend on the runtime resolving
# the home directory from /etc/passwd.
ENV HOME=/home/aw
WORKDIR /home/aw

EXPOSE 5600

CMD ["aw-server-rust"]
