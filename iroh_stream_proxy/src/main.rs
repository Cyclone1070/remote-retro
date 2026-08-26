use anyhow::Result;
use clap::{Parser, Subcommand};
use iroh::{endpoint::presets, Endpoint, EndpointAddr, EndpointId};
use std::time::Instant;

const ALPN: &[u8] = b"iroh-stream-benchmark/0";

#[derive(Parser)]
#[command(name = "iroh_stream_proxy")]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Run as Iroh server
    Server,
    /// Connect to Iroh server and measure round-trip latency & throughput
    Benchmark {
        node_id: EndpointId,
    },
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();

    match cli.command {
        Commands::Server => {
            let endpoint = Endpoint::builder(presets::N0)
                .alpns(vec![ALPN.to_vec()])
                .bind()
                .await?;
            let node_addr = endpoint.addr();
            println!("=== IROH SERVER READY ===");
            println!("NodeId: {}", endpoint.id());
            println!("NodeAddr: {:?}", node_addr);
            println!("=========================");

            while let Some(incoming) = endpoint.accept().await {
                tokio::spawn(async move {
                    if let Ok(connecting) = incoming.accept() {
                        if let Ok(connection) = connecting.await {
                            println!("Client connected: {}", connection.remote_id());
                            while let Ok((mut send, mut recv)) = connection.accept_bi().await {
                                tokio::spawn(async move {
                                    let mut buf = vec![0u8; 65536];
                                    loop {
                                        match recv.read(&mut buf).await {
                                            Ok(Some(n)) if n > 0 => {
                                                if send.write_all(&buf[..n]).await.is_err() {
                                                    break;
                                                }
                                            }
                                            _ => break,
                                        }
                                    }
                                });
                            }
                        }
                    }
                    Ok::<(), anyhow::Error>(())
                });
            }
        }
        Commands::Benchmark { node_id } => {
            let endpoint = Endpoint::builder(presets::N0)
                .alpns(vec![ALPN.to_vec()])
                .bind()
                .await?;
            println!("Connecting to Iroh Server Node: {}", node_id);
            let addr = EndpointAddr::new(node_id);
            let conn = endpoint.connect(addr, ALPN).await?;
            println!("Connected via Iroh P2P! Measuring QUIC stream round-trip latency...");

            let mut latencies = Vec::new();
            for i in 1..=10 {
                let (mut send, mut recv) = conn.open_bi().await?;
                let start = Instant::now();
                let msg = format!("ping-packet-{}", i);
                send.write_all(msg.as_bytes()).await?;
                send.finish()?;

                let resp = recv.read_to_end(65536).await?;
                let elapsed = start.elapsed();
                let rtt_ms = elapsed.as_secs_f64() * 1000.0;
                latencies.push(rtt_ms);
                println!(
                    "  [Packet {:2}/10] Iroh QUIC RTT: {:.2} ms | Payload echo: {}",
                    i,
                    rtt_ms,
                    String::from_utf8_lossy(&resp)
                );
                tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;
            }

            let avg: f64 = latencies.iter().sum::<f64>() / latencies.len() as f64;
            let min: f64 = latencies.iter().cloned().fold(f64::INFINITY, f64::min);
            let max: f64 = latencies.iter().cloned().fold(f64::NEG_INFINITY, f64::max);
            println!("\n=== IROH DIRECT P2P LATENCY BENCHMARK RESULT ===");
            println!("  Min RTT: {:.2} ms", min);
            println!("  Avg RTT: {:.2} ms", avg);
            println!("  Max RTT: {:.2} ms", max);
            println!("================================================");
        }
    }

    Ok(())
}
