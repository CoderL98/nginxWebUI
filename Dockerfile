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
RUN cargo build --release

FROM debian:bookworm-slim
RUN apt-get update \
    && apt-get install -y --no-install-recommends \
        nginx \
        openssl \
        curl \
        ca-certificates \
        cron \
    && rm -rf /var/lib/apt/lists/* \
    && curl -fsSL https://get.acme.sh | sh -s -- --home /root/.acme.sh -m nginxwebui@localhost \
    && ln -sf /root/.acme.sh/acme.sh /usr/local/bin/acme.sh

COPY --from=server /src/server/target/release/nginxwebui /opt/nginxwebui/nginxwebui
COPY --from=web /src/web/build /opt/nginxwebui/web
COPY docker/entrypoint.sh /entrypoint.sh
COPY docker/nginxwebui.conf /opt/nginxwebui/nginxwebui.conf
RUN chmod 755 /opt/nginxwebui/nginxwebui /entrypoint.sh \
    && mkdir -p /home/nginxWebUI /etc/nginx/ssl /var/www

ENV NGINXWEBUI_PORT=8080 \
    NGINXWEBUI_BIND=0.0.0.0 \
    NGINXWEBUI_HOME=/home/nginxWebUI \
    NGINXWEBUI_WEB_DIR=/opt/nginxwebui/web \
    NGINXWEBUI_RELOAD_CMD="nginx -s reload" \
    NGINXWEBUI_START_NGINX=1

EXPOSE 8080 80 443
VOLUME ["/home/nginxWebUI", "/etc/nginx/sites-available", "/etc/nginx/sites-enabled", "/etc/nginx/ssl", "/var/www", "/root/.acme.sh", "/var/log/nginx"]

# The image starts nginx (80/443) and acme.sh under /root/.acme.sh.
# Switching to a non-root USER would break that all-in-one model; leave the
# process as root inside the container. On the host, publish 8080 only to
# 127.0.0.1 and put HTTPS in front.
ENTRYPOINT ["/entrypoint.sh"]
