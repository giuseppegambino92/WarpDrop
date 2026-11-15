//! Raw QUIC throughput with WarpDrop's TLS/transport config (no engine logic).
use std::time::Instant;
use warp::identity::Identity;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let _ = rustls::crypto::ring::default_provider().install_default();
    tracing_subscriber::fmt().with_env_filter(std::env::var("RUST_LOG").unwrap_or_else(|_| "warn".into())).init();
    let mb: usize = std::env::args().nth(1).and_then(|s| s.parse().ok()).unwrap_or(256);
    let tcp = std::env::args().nth(2).as_deref() == Some("tcp");
    let (a, b) = (Identity::generate()?, Identity::generate()?);
    let (server, port) = if tcp {
        let l = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
        let port = l.local_addr()?.port();
        let sc = warp::tls::server_config(&b)?;
        let (tx, rx) = tokio::sync::oneshot::channel();
        tokio::spawn(async move {
            let (s, _) = l.accept().await.unwrap();
            let _ = tx.send(warp::transport::sockets::tunnel_server_endpoint(s, sc).unwrap());
        });
        (Err(rx), port)
    } else {
        let ep = quinn::Endpoint::new(quinn::EndpointConfig::default(), Some(warp::tls::server_config(&b)?), warp::transport::bind_udp(0)?, warp::transport::runtime())?;
        let port = ep.local_addr()?.port();
        (Ok(ep), port)
    };
    let srv = tokio::spawn(async move {
        let server = match server { Ok(ep) => ep, Err(rx) => rx.await.unwrap() };
        let conn = server.accept().await.unwrap().await.unwrap();
        let mut r = conn.accept_uni().await.unwrap();
        let mut buf = vec![0u8; 1 << 20];
        let mut n = 0usize;
        while let Some(k) = r.read(&mut buf).await.unwrap() { n += k; }
        n
    });
    let conn = if tcp {
        warp::transport::sockets::connect_tunnel(([127, 0, 0, 1], port).into(), warp::tls::client_config(&a, None)?).await?
    } else {
        let client = quinn::Endpoint::new(quinn::EndpointConfig::default(), None, warp::transport::bind_udp(0)?, warp::transport::runtime())?;
        client.connect_with(warp::tls::client_config(&a, None)?, ([127, 0, 0, 1], port).into(), "warpdrop.local")?.await?
    };
    let t0 = Instant::now();
    let mut s = conn.open_uni().await?;
    let chunk = vec![7u8; 1 << 20];
    for _ in 0..mb { s.write_all(&chunk).await?; }
    s.finish()?;
    let n = srv.await?;
    let secs = t0.elapsed().as_secs_f64();
    println!("raw quic: {} MiB in {:.2}s = {:.0} MB/s", n >> 20, secs, n as f64 / secs / 1e6);
    Ok(())
}
