# Build panel-metrics from the workspace root:
#   docker build -f linux/phase3/Dockerfile -t panel-metrics .

FROM rust:1-slim AS build
WORKDIR /src
RUN apt-get update && apt-get install -y --no-install-recommends pkg-config libudev-dev \
    && rm -rf /var/lib/apt/lists/*
COPY . .
# Only this crate: the others need a serial port and have no business in a container.
RUN cargo build --release -p panel-metrics

FROM debian:stable-slim
RUN apt-get update && apt-get install -y --no-install-recommends ca-certificates \
    && rm -rf /var/lib/apt/lists/* \
    && useradd --system --no-create-home --uid 10001 panel
COPY --from=build /src/target/release/panel-metrics /usr/local/bin/panel-metrics
USER panel
EXPOSE 9101
ENTRYPOINT ["/usr/local/bin/panel-metrics", "--config", "/etc/panel-metrics/config.toml"]
