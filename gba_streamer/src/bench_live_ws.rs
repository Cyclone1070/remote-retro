use anyhow::Result;
use futures_util::{SinkExt, StreamExt};
use std::time::Instant;
use tokio_tungstenite::connect_async;
use tokio_tungstenite::tungstenite::Message;

#[tokio::main]
async fn main() -> Result<()> {
    let url = "ws://100.73.151.90:48500/ws";
    println!("Connecting to live WebHost at {}", url);
    let (ws_stream, _) = connect_async(url).await?;
    let (mut write, mut read) = ws_stream.split();

    let total_frames = 500;
    let mut decode_times = Vec::with_capacity(total_frames);
    let mut host_times = Vec::with_capacity(total_frames);
    let mut frame_intervals = Vec::with_capacity(total_frames);
    let mut frame_sizes = Vec::with_capacity(total_frames);

    let mut last_t = Instant::now();

    for i in 0..total_frames {
        // Send input
        let input_mask: u16 = if i % 30 < 15 { 1 << 4 } else { 1 << 7 };
        write.send(Message::Binary(input_mask.to_le_bytes().to_vec().into())).await?;

        // Receive frame
        if let Some(Ok(Message::Binary(bytes))) = read.next().await {
            let now = Instant::now();
            if i > 10 {
                frame_intervals.push(now.duration_since(last_t).as_micros() as f64 / 1000.0);
            }
            last_t = now;

            if bytes.len() > 4 {
                let host_us = u32::from_le_bytes(bytes[0..4].try_into().unwrap());
                let compressed = &bytes[4..];
                frame_sizes.push(compressed.len());

                let t_dec = Instant::now();
                let decomp = lz4_flex::decompress_size_prepended(compressed).unwrap();
                let dec_dur = t_dec.elapsed().as_micros() as f64 / 1000.0;

                if i > 10 {
                    host_times.push(host_us as f64 / 1000.0);
                    decode_times.push(dec_dur);
                }
                assert_eq!(decomp.len(), 240 * 160 * 4);
            }
        }
    }

    let mean = |v: &[f64]| v.iter().sum::<f64>() / v.len() as f64;
    let avg_kb = frame_sizes.iter().sum::<usize>() as f64 / frame_sizes.len() as f64 / 1024.0;

    println!("LIVE_INTERACTIVE_STREAM_BENCHMARK_RESULTS:");
    println!("  Frames Streamed: {}", host_times.len());
    println!("  Mean Host Processing: {:.3} ms", mean(&host_times));
    println!("  Mean Client LZ4 Decode: {:.3} ms", mean(&decode_times));
    println!("  Mean Inter-Frame Interval: {:.2} ms ({:.1} FPS)", mean(&frame_intervals), 1000.0 / mean(&frame_intervals));
    println!("  Average Frame Size: {:.2} KB", avg_kb);
    Ok(())
}
