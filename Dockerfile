# --- Stage 1: build the Vue frontend ---
FROM node:22-alpine AS frontend
WORKDIR /build
COPY frontend/package*.json ./
RUN npm install
COPY frontend/ ./
RUN npm run build

# --- Stage 2: build the Rust backend ---
FROM rust:1-bookworm AS backend
WORKDIR /build
COPY backend/Cargo.toml backend/Cargo.lock* ./
# Pre-build dependencies for layer caching.
RUN mkdir src && echo "fn main() {}" > src/main.rs && echo "" > src/lib.rs \
    && cargo build --release && rm -rf src
COPY backend/src ./src
RUN touch src/main.rs src/lib.rs && cargo build --release

# --- Stage 3: runtime ---
FROM debian:bookworm-slim
# ca-certificates for outbound TLS; the Infisical CLI powers optional runtime
# secret injection (see backend/entrypoint.sh — plain env still works without it).
#
# Infisical CLI install notes (their Cloudsmith repo dies 2026-09-16):
#  - the setup script MUST configure artifacts-cli.infisical.com — the grep
#    guard fails the build loudly if it ever regresses to cloudsmith.io;
#  - the version is PINNED (all historical releases resolve on the new host);
#  - the apt source + keyring are removed after install so a later
#    `apt-get update` in this or a derived image can never break on it.
ARG INFISICAL_CLI_VERSION=0.43.113
RUN apt-get update \
    && apt-get install -y --no-install-recommends ca-certificates curl bash \
    && curl -1sLf 'https://artifacts-cli.infisical.com/setup.deb.sh' | bash \
    && ! grep -rqi 'cloudsmith' /etc/apt/sources.list.d/ \
    && grep -rq 'artifacts-cli.infisical.com' /etc/apt/sources.list.d/infisical.list \
    && apt-get install -y --no-install-recommends "infisical=${INFISICAL_CLI_VERSION}" \
    && rm -f /etc/apt/sources.list.d/infisical.list /usr/share/keyrings/infisical-archive-keyring.gpg \
    && apt-get purge -y curl && apt-get autoremove -y \
    && rm -rf /var/lib/apt/lists/*
WORKDIR /app
COPY --from=backend /build/target/release/memories ./memories
COPY --from=frontend /build/dist ./frontend/dist
COPY backend/entrypoint.sh /entrypoint.sh
RUN chmod +x /entrypoint.sh
# No CLI self-update check at container start: quieter logs, no network call,
# and no surprise repo-migration banners in production output.
ENV INFISICAL_DISABLE_UPDATE_CHECK=true \
    MEMORIES_DATA_DIR=/data \
    WEB_HOST=0.0.0.0 \
    WEB_PORT=8090
VOLUME /data
EXPOSE 8090
ENTRYPOINT ["/entrypoint.sh"]
CMD ["./memories"]
