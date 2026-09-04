use anyhow::Result;
use futures_util::StreamExt;
use std::time::Instant;
use tokio_tungstenite::connect_async;

#[tokio::main]
async fn main() -> Result<()> {
    let ws_url = "ws://100.73.151.90:48500/ws";
    println!("Connecting to {} to benchmark server frame pacing...", ws_url);
    let (ws_stream, _) = connect_async(ws_url).await?;
    let (_, mut read) = ws_stream.split();

    let total_samples = 1000;
    let warmup_samples = 60;

    let mut intervals_ms = Vec::with_capacity(total_samples);
    let mut last_t = Instant::now();
    let mut count = 0;

    while count < total_samples {
        match read.next().await {
            Some(Ok(msg)) => {
                if msg.is_binary() {
                    let now = Instant::now();
                    let dt = now.duration_since(last_t).as_secs_f64() * 1000.0;
                    last_t = now;

                    count += 1;
                    if count > warmup_samples {
                        intervals_ms.push(dt);
                    }
                }
            }
            Some(Err(e)) => {
                anyhow::bail!("WebSocket read error: {}", e);
            }
            None => {
                anyhow::bail!("WebSocket connection closed prematurely after {} samples", count);
            }
        }
    }

    let n = intervals_ms.len() as f64;
    let mean_dt: f64 = intervals_ms.iter().sum::<f64>() / n;
    let variance: f64 = intervals_ms.iter().map(|&x| (x - mean_dt).powi(2)).sum::<f64>() / (n - 1.0);
    let jitter_sigma = variance.sqrt();

    let mut sorted = intervals_ms.clone();
    sorted.sort_by(|a, b| a.partial_cmp(b).unwrap());

    let p50 = sorted[(n * 0.50) as usize];
    let p95 = sorted[(n * 0.95) as usize];
    let p99 = sorted[(n * 0.99) as usize];
    let min_dt = sorted[0];
    let max_dt = sorted[sorted.len() - 1];

    let outlier_count = intervals_ms.iter().filter(|&&dt| (dt - 16.667).abs() > 3.0).count();
    let outlier_pct = (outlier_count as f64 / n) * 100.0;

    println!("\n===================================================================");
    println!(" ⏱️ SERVER EMISSION PACING BENCHMARK (1,000 SAMPLES)");
    println!("===================================================================");
    println!("  Target VSYNC Interval:    16.667 ms (60.0000 FPS)");
    println!("  Mean Emission Interval:   {:.3} ms ({:.2} FPS)", mean_dt, 1000.0 / mean_dt);
    println!("  Pacing Jitter (σ):        {:.3} ms", jitter_sigma);
    println!("  P50 (Median) Interval:    {:.3} ms", p50);
    println!("  P95 Interval:             {:.3} ms", p95);
    println!("  P99 Interval:             {:.3} ms", p99);
    println!("  Interval Range [Min/Max]: [{:.2} ms / {:.2} ms]", min_dt, max_dt);
    println!("  Pacing Outliers (>3ms):   {} / {} ({:.2}%)", outlier_count, n as usize, outlier_pct);
    println!("===================================================================\n");

    Ok(())
}
