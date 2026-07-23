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
RUN apt-get update \
    && apt-get install -y --no-install-recommends ca-certificates \
    && rm -rf /var/lib/apt/lists/*
WORKDIR /app
COPY --from=backend /build/target/release/memories ./memories
COPY --from=frontend /build/dist ./frontend/dist
ENV MEMORIES_DATA_DIR=/data \
    WEB_HOST=0.0.0.0 \
    WEB_PORT=8090
VOLUME /data
EXPOSE 8090
CMD ["./memories"]
