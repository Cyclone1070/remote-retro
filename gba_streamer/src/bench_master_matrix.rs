use anyhow::Result;
use futures_util::{SinkExt, StreamExt};
use std::collections::HashMap;
use std::time::Instant;
use tokio_tungstenite::connect_async;
use tokio_tungstenite::tungstenite::Message;

const GBA_WIDTH: usize = 240;
const GBA_HEIGHT: usize = 160;

struct BenchmarkResults {
    suite_name: String,
    frames_evaluated: usize,
    avg_fps: f64,
    pacing_jitter_ms: f64,
    stutter_1pct_rate: f64,
    mean_m2p_ms: f64,
    p50_m2p_ms: f64,
    p95_m2p_ms: f64,
    p99_m2p_ms: f64,
    avg_bitrate_mbps: f64,
    ghosting_error_rate: f64,
}

async fn run_benchmark_suite(
    suite_name: &str,
    is_heavy_input: bool,
    total_frames: usize,
    warmup_frames: usize,
) -> Result<BenchmarkResults> {
    let url = "ws://100.73.151.90:48500/ws";
    let (ws_stream, _) = connect_async(url).await?;
    let (mut write, mut read) = ws_stream.split();

    let mut m2p_latencies = Vec::with_capacity(total_frames);
    let mut inter_frame_intervals = Vec::with_capacity(total_frames);
    let mut frame_bytes = Vec::with_capacity(total_frames);
    let mut input_history: HashMap<u32, Instant> = HashMap::new();
    let mut last_frame_recv = Instant::now();
    let mut screen_buffer = vec![0u32; GBA_WIDTH * GBA_HEIGHT];

    for frame_idx in 0..total_frames as u32 {
        let t_input_sent = Instant::now();
        input_history.insert(frame_idx, t_input_sent);

        let mut buttons: u16 = 0;
        if frame_idx < warmup_frames as u32 {
            if frame_idx % 20 < 10 { buttons |= (1 << 3) | (1 << 0); }
        } else if is_heavy_input {
            // 120 Hz rapid multi-button spam
            let f = frame_idx - warmup_frames as u32;
            if f % 4 < 2 { buttons |= 1 << 4; } else { buttons |= 1 << 5; }
            if f % 3 == 0 { buttons |= 1 << 6; }
            if f % 5 == 0 { buttons |= 1 << 7; }
            if f % 2 == 0 { buttons |= 1 << 0; }
            if f % 3 == 1 { buttons |= 1 << 1; }
        } else {
            // Standard continuous active gameplay
            let f = frame_idx - warmup_frames as u32;
            if f % 80 < 60 { buttons |= 1 << 4; } else { buttons |= 1 << 5; }
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

                if flag == 1 {
                    let decomp = lz4_flex::decompress_size_prepended(payload)?;
                    let src16 = unsafe {
                        std::slice::from_raw_parts(decomp.as_ptr() as *const u16, decomp.len() / 2)
                    };
                    for i in 0..GBA_WIDTH * GBA_HEIGHT {
                        let p = src16[i];
                        let r = ((p & 0x7C00) >> 10) as u32 * 255 / 31;
                        let g = ((p & 0x03E0) >> 5) as u32 * 255 / 31;
                        let b = (p & 0x001F) as u32 * 255 / 31;
                        screen_buffer[i] = (255 << 24) | (b << 16) | (g << 8) | r;
                    }
                } else if flag == 0 {
                    let bitmask_len = 75;
                    if payload.len() > bitmask_len {
                        let bitmask = &payload[..bitmask_len];
                        let decomp = lz4_flex::decompress_size_prepended(&payload[bitmask_len..])?;
                        let src16 = unsafe {
                            std::slice::from_raw_parts(decomp.as_ptr() as *const u16, decomp.len() / 2)
                        };
                        let mut read_idx = 0;
                        for ty in 0..20 {
                            for tx in 0..30 {
                                let tile_idx = ty * 30 + tx;
                                if (bitmask[tile_idx / 8] & (1 << (tile_idx % 8))) != 0 {
                                    for y in 0..8 {
                                        let py = ty * 8 + y;
                                        let start_idx = py * GBA_WIDTH + tx * 8;
                                        for x in 0..8 {
                                            if read_idx < src16.len() {
                                                let p = src16[read_idx];
                                                read_idx += 1;
                                                let r = ((p & 0x7C00) >> 10) as u32 * 255 / 31;
                                                let g = ((p & 0x03E0) >> 5) as u32 * 255 / 31;
                                                let b = (p & 0x001F) as u32 * 255 / 31;
                                                screen_buffer[start_idx + x] = (255 << 24) | (b << 16) | (g << 8) | r;
                                            }
                                        }
                                    }
                                }
                            }
                        }
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
    let avg_fps = 1000.0 / mean_interval;
    let jitter_std = std_dev(&inter_frame_intervals);
    let stutters = inter_frame_intervals.iter().filter(|&&dt| dt > 33.33).count();
    let stutter_rate = (stutters as f64 / inter_frame_intervals.len() as f64) * 100.0;

    let avg_bytes = frame_bytes.iter().sum::<usize>() as f64 / frame_bytes.len() as f64;
    let mbps_60 = (avg_bytes * 8.0 * 60.0) / 1_000_000.0;

    Ok(BenchmarkResults {
        suite_name: suite_name.to_string(),
        frames_evaluated: valid_count,
        avg_fps,
        pacing_jitter_ms: jitter_std,
        stutter_1pct_rate: stutter_rate,
        mean_m2p_ms: avg_m2p,
        p50_m2p_ms: p50_m2p,
        p95_m2p_ms: p95_m2p,
        p99_m2p_ms: p99_m2p,
        avg_bitrate_mbps: mbps_60,
        ghosting_error_rate: 0.0,
    })
}

#[tokio::main]
async fn main() -> Result<()> {
    println!("===================================================================");
    println!("  MASTER PROOF-BASED BENCHMARK SUITE (EVALUATING 4,000 FRAMES)");
    println!("===================================================================");

    println!("\n>>> Running Suite 1: Standard Active Gameplay (2,000 frames)...");
    let res1 = run_benchmark_suite("Standard Active Gameplay", false, 2000, 120).await?;

    println!("\n>>> Running Suite 2: Heavy Input Stress (120 Hz Spam, 2,000 frames)...");
    let res2 = run_benchmark_suite("Heavy Input Stress (120 Hz)", true, 2000, 120).await?;

    println!("\n===================================================================");
    println!("  FINAL MULTI-SUITE BENCHMARK REPORT ");
    println!("===================================================================");
    println!("{:<32} | {:<8} | {:<10} | {:<8} | {:<8} | {:<8} | {:<8}",
        "Benchmark Scenario", "FPS", "Jitter", "1% Low", "P50 M2P", "P95 M2P", "Bitrate");
    println!("--------------------------------------------------------------------------------------------------");
    for r in [&res1, &res2] {
        println!("{:<32} | {:<8.1} | {:<8.2} ms | {:<7.2}% | {:<6.2} ms | {:<6.2} ms | {:.2} Mbps",
            r.suite_name, r.avg_fps, r.pacing_jitter_ms, r.stutter_1pct_rate, r.p50_m2p_ms, r.p95_m2p_ms, r.avg_bitrate_mbps);
    }
    println!("===================================================================\n");

    Ok(())
}
