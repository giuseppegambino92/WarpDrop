//! `warp-signal` — run with `warp-signal [bind-addr]` (default `0.0.0.0:47900`).
//! TCP serves the WebSocket rendezvous at `/v1/ws`; the same port number on
//! UDP serves address discovery and the encrypted-packet relay.

use std::net::SocketAddr;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()))
        .init();
    let bind: SocketAddr = std::env::args()
        .nth(1)
        .or_else(|| std::env::var("WARP_SIGNAL_BIND").ok())
        .unwrap_or_else(|| "0.0.0.0:47900".into())
        .parse()?;
    let cfg = warp_signal::Config::from_env();
    let tcp = tokio::net::TcpListener::bind(bind).await?;
    let udp = tokio::net::UdpSocket::bind(bind).await?;
    tracing::info!("warp-signal listening on {bind} (tcp+udp)");
    warp_signal::serve(tcp, udp, cfg).await
}
