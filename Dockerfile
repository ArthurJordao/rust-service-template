# syntax=docker/dockerfile:1

# --- Stage 1: build the SPA (web/dist) ---
FROM node:20-bookworm-slim AS web
WORKDIR /web
COPY web/package.json web/package-lock.json web/.npmrc ./
RUN npm ci
COPY web/ ./
RUN npm run build

# --- Stage 2: build the Rust binaries ---
# migrations/ must be present: `sqlx::migrate!` embeds them at COMPILE time.
# (Optional rebuild-speed upgrade: cargo-chef to cache the dependency layer.)
FROM rust:1-bookworm AS build
WORKDIR /app
COPY Cargo.toml Cargo.lock ./
COPY crates/ crates/
COPY migrations/ migrations/
RUN cargo build --release --locked -p app --bin app --bin migrate

# --- Stage 3: runtime ---
FROM debian:bookworm-slim AS runtime
# ca-certificates + libssl3: reqwest's default `native-tls` backend links OpenSSL
# on Linux. (Future slimming: switch reqwest to rustls-tls to drop libssl3.)
RUN apt-get update \
 && apt-get install -y --no-install-recommends ca-certificates libssl3 \
 && rm -rf /var/lib/apt/lists/* \
 && useradd --create-home --uid 10001 app
WORKDIR /app
COPY --from=build /app/target/release/app /app/app
COPY --from=build /app/target/release/migrate /app/migrate
COPY --from=web /web/dist /app/web/dist
ARG GIT_SHA=""
ENV APP_BUILD_SHA=$GIT_SHA
ENV APP__SERVER__PORT=8080
EXPOSE 8080
USER app
CMD ["/app/app"]
