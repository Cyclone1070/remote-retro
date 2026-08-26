use anyhow::Result;
use futures_util::{SinkExt, StreamExt};
use std::collections::HashMap;
use std::time::Instant;
use tokio_tungstenite::connect_async;
use tokio_tungstenite::tungstenite::Message;

const GBA_WIDTH: usize = 240;
const GBA_HEIGHT: usize = 160;

#[tokio::main]
async fn main() -> Result<()> {
    let url = "ws://100.73.151.90:48500/ws";
    println!("Connecting to live GBA WebHost at {}", url);
    let (ws_stream, _) = connect_async(url).await?;
    let (mut write, mut read) = ws_stream.split();

    let total_frames: usize = 2000;
    let warmup_frames: usize = 120;

    let mut m2p_latencies = Vec::with_capacity(total_frames);
    let mut inter_frame_intervals = Vec::with_capacity(total_frames);
    let mut frame_bytes = Vec::with_capacity(total_frames);
    let mut input_history: HashMap<u32, Instant> = HashMap::new();
    let mut last_frame_recv = Instant::now();
    let mut screen_buffer = vec![0u16; GBA_WIDTH * GBA_HEIGHT];

    let mut corrupt_frames = 0;
    let mut clean_frames = 0;

    println!("===================================================================");
    println!(" ⚡ VISUAL GHOSTING & M2P BENCHMARK (DYNAMIC PALETTE 2,000 FRAMES) ");
    println!("===================================================================");

    for frame_idx in 0..total_frames as u32 {
        let t_input_sent = Instant::now();
        input_history.insert(frame_idx, t_input_sent);

        let mut buttons: u16 = 0;
        if frame_idx < warmup_frames as u32 {
            if frame_idx % 20 < 10 { buttons |= (1 << 3) | (1 << 0); }
        } else {
            let f = frame_idx - warmup_frames as u32;
            if f % 4 < 2 { buttons |= 1 << 4; } else { buttons |= 1 << 5; }
            if f % 25 < 8 { buttons |= 1 << 0; }
            if f % 45 < 12 { buttons |= 1 << 1; }
        }

        let mut input_packet = Vec::with_capacity(14);
        input_packet.extend_from_slice(&frame_idx.to_le_bytes());
        let now_us = t_input_sent.elapsed().as_micros() as u64;
        input_packet.extend_from_slice(&now_us.to_le_bytes());
        input_packet.extend_from_slice(&buttons.to_le_bytes());

        write.send(Message::Binary(input_packet.into())).await?;

        if let Some(Ok(Message::Binary(bytes))) = read.next().await {
            let t_recv = Instant::now();

            if frame_idx > warmup_frames as u32 {
                let dt = t_recv.duration_since(last_frame_recv).as_micros() as f64 / 1000.0;
                inter_frame_intervals.push(dt);
            }
            last_frame_recv = t_recv;

            if bytes.len() >= 33 {
                let matched_seq = u32::from_le_bytes(bytes[0..4].try_into().unwrap());
                let flag = bytes[32];
                let payload = &bytes[33..];

                if flag == 2 {
                    if let Ok(decomp) = lz4_flex::decompress_size_prepended(payload) {
                        let pal_len = u16::from_le_bytes(decomp[0..2].try_into().unwrap()) as usize;
                        let pal_src = unsafe {
                            std::slice::from_raw_parts(decomp[2..2 + pal_len * 2].as_ptr() as *const u16, pal_len)
                        };
                        let indices = &decomp[2 + pal_len * 2..];
                        if indices.len() >= GBA_WIDTH * GBA_HEIGHT {
                            for i in 0..GBA_WIDTH * GBA_HEIGHT {
                                screen_buffer[i] = pal_src[indices[i] as usize];
                            }
                            clean_frames += 1;
                        } else {
                            corrupt_frames += 1;
                        }
                    } else {
                        corrupt_frames += 1;
                    }
                } else if flag == 1 {
                    if let Ok(decomp) = lz4_flex::decompress_size_prepended(payload) {
                        if decomp.len() == GBA_WIDTH * GBA_HEIGHT * 2 {
                            let src16 = unsafe {
                                std::slice::from_raw_parts(decomp.as_ptr() as *const u16, decomp.len() / 2)
                            };
                            screen_buffer.copy_from_slice(src16);
                            clean_frames += 1;
                        } else {
                            corrupt_frames += 1;
                        }
                    } else {
                        corrupt_frames += 1;
                    }
                }

                if let Some(t_sent) = input_history.get(&matched_seq) {
                    let total_m2p_ms = t_sent.elapsed().as_micros() as f64 / 1000.0;
                    if frame_idx > warmup_frames as u32 {
                        frame_bytes.push(payload.len());
                        m2p_latencies.push(total_m2p_ms);
                    }
                }
            }
        }
    }

    let mean = |v: &[f64]| if v.is_empty() { 0.0 } else { v.iter().sum::<f64>() / v.len() as f64 };
    let std_dev = |v: &[f64]| {
        if v.len() < 2 { return 0.0; }
        let m = mean(v);
        let variance = v.iter().map(|x| (x - m).powi(2)).sum::<f64>() / (v.len() - 1) as f64;
        variance.sqrt()
    };
    let percentile = |v: &[f64], p: f64| {
        let mut sorted = v.to_vec();
        sorted.sort_by(|a, b| a.partial_cmp(b).unwrap());
        let idx = ((sorted.len() as f64 * p) as usize).min(sorted.len() - 1);
        sorted[idx]
    };

    let valid_count = m2p_latencies.len();
    let avg_m2p = mean(&m2p_latencies);
    let p50_m2p = percentile(&m2p_latencies, 0.50);
    let p95_m2p = percentile(&m2p_latencies, 0.95);
    let p99_m2p = percentile(&m2p_latencies, 0.99);

    let mean_interval = mean(&inter_frame_intervals);
    let avg_fps = if mean_interval > 0.0 { 1000.0 / mean_interval } else { 0.0 };
    let jitter_std = std_dev(&inter_frame_intervals);
    let total_eval = clean_frames + corrupt_frames;
    let ghosting_rate = if total_eval > 0 { (corrupt_frames as f64 / total_eval as f64) * 100.0 } else { 0.0 };

    let avg_bytes = if !frame_bytes.is_empty() { frame_bytes.iter().sum::<usize>() as f64 / frame_bytes.len() as f64 } else { 0.0 };
    let mbps_60 = (avg_bytes * 8.0 * 60.0) / 1_000_000.0;

    println!("\n===================================================================");
    println!("  LIVE DYNAMIC PALETTE BENCHMARK RESULTS (2,000 FRAMES) ");
    println!("===================================================================");
    println!("  Evaluated Frames:                 {}", valid_count);
    println!("  Delivered Framerate:              {:.1} FPS", avg_fps);
    println!("  Frame Pacing Jitter:              {:.2} ms", jitter_std);
    println!("  Average Frame Size:               {:.2} KB ({:.2} Mbps @ 60 FPS)", avg_bytes / 1024.0, mbps_60);
    println!("  Mean M2P Latency:                 {:.2} ms", avg_m2p);
    println!("  P50 (Median) M2P:                 {:.2} ms", p50_m2p);
    println!("  P95 M2P (Tail):                   {:.2} ms", p95_m2p);
    println!("  P99 M2P:                          {:.2} ms", p99_m2p);
    println!("  Visual Corruption / Ghosting Rate:{:.2}% ({} corrupt / {} total)", ghosting_rate, corrupt_frames, total_eval);
    println!("===================================================================\n");

    Ok(())
}
