# syntax=docker/dockerfile:1

FROM node:22-bookworm-slim AS web
WORKDIR /src/web
RUN corepack enable
COPY web/package.json web/pnpm-lock.yaml ./
RUN pnpm install --frozen-lockfile
COPY web/ ./
RUN pnpm run build

FROM rust:1-bookworm AS server
WORKDIR /src/server
COPY server/ ./
RUN cargo build --release --bin nginxwebui --bin nginxwebui-agent

FROM debian:bookworm-slim
ARG TARGETARCH
# Pin file is the single source for the acme.sh tag + sha256 (keep in
# sync with server/src/install.rs). The hash is verified before extract.
COPY script/acme.sh.pin /tmp/acme.sh.pin
RUN apt-get update \
    && apt-get install -y --no-install-recommends \
        nginx \
        openssl \
        curl \
        ca-certificates \
        cron \
        sudo \
        tar \
    && rm -rf /var/lib/apt/lists/* \
    && set -eu; . /tmp/acme.sh.pin \
    && curl -fsSL -o /tmp/acme.sh.tar.gz "$ACME_SH_URL" \
    && echo "$ACME_SH_SHA256  /tmp/acme.sh.tar.gz" | sha256sum -c - \
    && tar -xzf /tmp/acme.sh.tar.gz -C /tmp \
    && sh "/tmp/acme.sh-${ACME_SH_VERSION}/acme.sh" --install --home /opt/acme.sh -m nginxwebui@localhost \
    && ln -sf /opt/acme.sh/acme.sh /usr/local/bin/acme.sh \
    && rm -rf /tmp/acme.sh.tar.gz "/tmp/acme.sh-${ACME_SH_VERSION}" /tmp/acme.sh.pin \
    && groupadd --system nginxwebui \
    && useradd --system --gid nginxwebui --home-dir /home/nginxWebUI --shell /usr/sbin/nologin --create-home nginxwebui

COPY --from=server /src/server/target/release/nginxwebui /opt/nginxwebui/nginxwebui
COPY --from=server /src/server/target/release/nginxwebui-agent /opt/nginxwebui/nginxwebui-agent
COPY --from=web /src/web/build /opt/nginxwebui/web
COPY docker/entrypoint.sh /entrypoint.sh
COPY docker/nginxwebui.conf /opt/nginxwebui/nginxwebui.conf
COPY server/assets/nginxwebui-priv.sh /usr/local/sbin/nginxwebui-priv
RUN chmod 755 /opt/nginxwebui/nginxwebui /opt/nginxwebui/nginxwebui-agent /entrypoint.sh \
    && chown root:root /usr/local/sbin/nginxwebui-priv \
    && chmod 755 /usr/local/sbin/nginxwebui-priv \
    && printf 'nginxwebui ALL=(root) NOPASSWD: /usr/local/sbin/nginxwebui-priv\n' > /etc/sudoers.d/nginxwebui \
    && chmod 440 /etc/sudoers.d/nginxwebui \
    && mkdir -p /home/nginxWebUI /etc/nginx/ssl /var/www /var/log/nginx \
    && arch="$(uname -m)" \
    && if [ "$TARGETARCH" = "amd64" ]; then arch=x86_64; fi \
    && if [ "$TARGETARCH" = "arm64" ]; then arch=aarch64; fi \
    && mkdir -p "/opt/nginxwebui/agents/linux-${arch}" \
    && cp /opt/nginxwebui/nginxwebui-agent "/opt/nginxwebui/agents/linux-${arch}/nginxwebui-agent" \
    && chown -R nginxwebui:nginxwebui /home/nginxWebUI

ENV NGINXWEBUI_PORT=8080 \
    NGINXWEBUI_BIND=0.0.0.0 \
    NGINXWEBUI_HOME=/home/nginxWebUI \
    NGINXWEBUI_WEB_DIR=/opt/nginxwebui/web \
    NGINXWEBUI_RELOAD_CMD="nginx -s reload" \
    NGINXWEBUI_START_NGINX=1

EXPOSE 8080 80 443
VOLUME ["/home/nginxWebUI", "/etc/nginx/sites-available", "/etc/nginx/sites-enabled", "/etc/nginx/ssl", "/var/www", "/home/nginxWebUI/.acme.sh", "/var/log/nginx"]

# entrypoint starts system nginx (80/443) as root, then runs the panel as
# user nginxwebui. Binding 80/443 still needs a privileged nginx master.
# acme.sh lives under /home/nginxWebUI/.acme.sh (not /root/.acme.sh).
ENTRYPOINT ["/entrypoint.sh"]
