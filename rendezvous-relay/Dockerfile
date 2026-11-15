# Build from the repository root:
#   docker build -f rendezvous-relay/Dockerfile -t warp-signal .
#   docker run -p 47900:47900/tcp -p 47900:47900/udp warp-signal
FROM rust:1-slim AS build
WORKDIR /src
COPY . .
RUN cargo build --release -p warp-signal

FROM debian:stable-slim
COPY --from=build /src/target/release/warp-signal /usr/local/bin/warp-signal
EXPOSE 47900/tcp 47900/udp
USER nobody
ENTRYPOINT ["/usr/local/bin/warp-signal", "0.0.0.0:47900"]
