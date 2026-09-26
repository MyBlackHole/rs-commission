# Build from repository root. Frontend is a static Leptos CSR application.
FROM rust:1-bookworm AS build
RUN apt-get update && apt-get install -y --no-install-recommends pkg-config libssl-dev \
    && rm -rf /var/lib/apt/lists/*
RUN rustup target add wasm32-unknown-unknown \
    && cargo install trunk --version 0.21.14 --locked
WORKDIR /app
COPY . .
WORKDIR /app/apps/console
RUN trunk build --release --locked

FROM nginx:stable-alpine
COPY deploy/console.nginx.conf /etc/nginx/conf.d/default.conf
COPY --from=build /app/apps/console/dist /usr/share/nginx/html
EXPOSE 80
