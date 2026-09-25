FROM rust:1-bookworm AS build
WORKDIR /app
COPY . .
RUN cargo build --locked --release -p commission-rs --bin commissiond

FROM debian:bookworm-slim
RUN apt-get update && apt-get install -y --no-install-recommends ca-certificates \
    && rm -rf /var/lib/apt/lists/* \
    && useradd --system --uid 10001 --create-home commission
COPY --from=build /app/target/release/commissiond /usr/local/bin/commissiond
USER 10001
ENV BIND_ADDR=0.0.0.0:8080
EXPOSE 8080
ENTRYPOINT ["commissiond"]
CMD ["serve"]
