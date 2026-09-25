# Build from the repository root: docker build -f apps/console/Dockerfile .
FROM rust:1-bookworm AS build
RUN apt-get update && apt-get install -y --no-install-recommends pkg-config libssl-dev \
    && rm -rf /var/lib/apt/lists/*
RUN rustup target add wasm32-unknown-unknown \
    && cargo install dioxus-cli --version 0.7.10 --locked
WORKDIR /app
COPY . .
WORKDIR /app/apps/console
RUN dx build --platform web --release

FROM nginx:stable-alpine
COPY deploy/console.nginx.conf /etc/nginx/conf.d/default.conf
COPY --from=build /app/target/dx/commission-console/release/web/public /usr/share/nginx/html
EXPOSE 80
