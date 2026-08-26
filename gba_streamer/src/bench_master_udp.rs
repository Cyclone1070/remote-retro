use anyhow::Result;
use std::collections::HashMap;
use std::net::UdpSocket;
use std::time::{Duration, Instant};

const GBA_WIDTH: usize = 240;
const GBA_HEIGHT: usize = 160;

struct UdpBenchmarkResults {
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
}

struct FrameReassembler {
    current_frame_seq: u32,
    chunk_count: usize,
    chunks: Vec<Option<Vec<u8>>>,
    matched_seq: u32,
    sim_us: u32,
    enc_us: u32,
    flag: u8,
}

impl FrameReassembler {
    fn new() -> Self {
        Self {
            current_frame_seq: 0,
            chunk_count: 0,
            chunks: Vec::new(),
            matched_seq: 0,
            sim_us: 0,
            enc_us: 0,
            flag: 0,
        }
    }

    fn push_chunk(&mut self, packet: &[u8]) -> Option<(u32, u32, u32, u8, Vec<u8>)> {
        if packet.len() < 19 { return None; }
        let frame_seq = u32::from_le_bytes(packet[0..4].try_into().unwrap());
        let chunk_idx = packet[4] as usize;
        let chunk_count = packet[5] as usize;
        let matched_seq = u32::from_le_bytes(packet[6..10].try_into().unwrap());
        let sim_us = u32::from_le_bytes(packet[10..14].try_into().unwrap());
        let enc_us = u32::from_le_bytes(packet[14..18].try_into().unwrap());
        let flag = packet[18];
        let chunk_data = &packet[19..];

        if frame_seq > self.current_frame_seq {
            // New freshest frame arrived: discard older incomplete chunks
            self.current_frame_seq = frame_seq;
            self.chunk_count = chunk_count;
            self.chunks = vec![None; chunk_count];
            self.matched_seq = matched_seq;
            self.sim_us = sim_us;
            self.enc_us = enc_us;
            self.flag = flag;
        }

        if frame_seq == self.current_frame_seq && chunk_idx < self.chunk_count {
            self.chunks[chunk_idx] = Some(chunk_data.to_vec());
            if self.chunks.iter().all(|c| c.is_some()) {
                let mut full_payload = Vec::new();
                for c in &self.chunks {
                    full_payload.extend_from_slice(c.as_ref().unwrap());
                }
                return Some((self.current_frame_seq, self.matched_seq, self.sim_us, self.flag, full_payload));
            }
        }
        None
    }
}

fn run_udp_benchmark(
    suite_name: &str,
    is_heavy_input: bool,
    total_frames: usize,
    warmup_frames: usize,
) -> Result<UdpBenchmarkResults> {
    let host = "100.73.151.90:48500";
    let socket = UdpSocket::bind("0.0.0.0:0")?;
    socket.set_nonblocking(true)?;

    let mut m2p_latencies = Vec::with_capacity(total_frames);
    let mut inter_frame_intervals = Vec::with_capacity(total_frames);
    let mut frame_bytes = Vec::with_capacity(total_frames);
    let mut input_history: HashMap<u32, Instant> = HashMap::new();
    let mut last_frame_recv = Instant::now();
    let mut screen_buffer = vec![0u32; GBA_WIDTH * GBA_HEIGHT];

    let mut reassembler = FrameReassembler::new();
    let mut recv_buf = [0u8; 2048];

    for frame_idx in 0..total_frames as u32 {
        let t_input_sent = Instant::now();
        input_history.insert(frame_idx, t_input_sent);

        let mut buttons: u16 = 0;
        if frame_idx < warmup_frames as u32 {
            if frame_idx % 20 < 10 { buttons |= (1 << 3) | (1 << 0); }
        } else if is_heavy_input {
            let f = frame_idx - warmup_frames as u32;
            if f % 4 < 2 { buttons |= 1 << 4; } else { buttons |= 1 << 5; }
            if f % 3 == 0 { buttons |= 1 << 6; }
            if f % 5 == 0 { buttons |= 1 << 7; }
            if f % 2 == 0 { buttons |= 1 << 0; }
            if f % 3 == 1 { buttons |= 1 << 1; }
        } else {
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

        let _ = socket.send_to(&input_packet, host);

        let t_start_poll = Instant::now();
        let mut completed_frame = None;

        while t_start_poll.elapsed() < Duration::from_millis(17) {
            match socket.recv_from(&mut recv_buf) {
                Ok((len, _)) => {
                    if let Some(res) = reassembler.push_chunk(&recv_buf[..len]) {
                        completed_frame = Some(res);
                    }
                }
                Err(ref e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                    if completed_frame.is_some() {
                        break;
                    }
                    std::thread::sleep(Duration::from_micros(300));
                }
                Err(_) => break,
            }
        }

        if let Some((_frame_seq, matched_seq, _sim_us, flag, payload)) = completed_frame {
            let t_recv = Instant::now();
            if frame_idx > warmup_frames as u32 {
                let dt = t_recv.duration_since(last_frame_recv).as_micros() as f64 / 1000.0;
                inter_frame_intervals.push(dt);
            }
            last_frame_recv = t_recv;

            if flag == 1 {
                if let Ok(decomp) = lz4_flex::decompress_size_prepended(&payload) {
                    let src16 = unsafe {
                        std::slice::from_raw_parts(decomp.as_ptr() as *const u16, decomp.len() / 2)
                    };
                    for i in 0..(GBA_WIDTH * GBA_HEIGHT).min(src16.len()) {
                        let p = src16[i];
                        let r = ((p & 0x7C00) >> 10) as u32 * 255 / 31;
                        let g = ((p & 0x03E0) >> 5) as u32 * 255 / 31;
                        let b = (p & 0x001F) as u32 * 255 / 31;
                        screen_buffer[i] = (255 << 24) | (b << 16) | (g << 8) | r;
                    }
                }
            } else if flag == 0 {
                let bitmask_len = 75;
                if payload.len() > bitmask_len {
                    let bitmask = &payload[..bitmask_len];
                    if let Ok(decomp) = lz4_flex::decompress_size_prepended(&payload[bitmask_len..]) {
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
    let stutters = inter_frame_intervals.iter().filter(|&&dt| dt > 33.33).count();
    let stutter_rate = if !inter_frame_intervals.is_empty() { (stutters as f64 / inter_frame_intervals.len() as f64) * 100.0 } else { 0.0 };

    let avg_bytes = if !frame_bytes.is_empty() { frame_bytes.iter().sum::<usize>() as f64 / frame_bytes.len() as f64 } else { 0.0 };
    let mbps_60 = (avg_bytes * 8.0 * 60.0) / 1_000_000.0;

    Ok(UdpBenchmarkResults {
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
    })
}

fn main() -> Result<()> {
    println!("===================================================================");
    println!("  MASTER UDP ZERO-QUEUE BENCHMARK MATRIX (4,000 FRAMES) ");
    println!("===================================================================");

    println!("\n>>> Running UDP Suite 1: Standard Active Gameplay (2,000 frames)...");
    let res1 = run_udp_benchmark("UDP Standard Active Gameplay", false, 2000, 120)?;

    println!("\n>>> Running UDP Suite 2: Heavy Input Stress (120 Hz Spam, 2,000 frames)...");
    let res2 = run_udp_benchmark("UDP Heavy Input Stress (120 Hz)", true, 2000, 120)?;

    println!("\n===================================================================");
    println!("  FINAL UDP PROOF-BASED BENCHMARK REPORT ");
    println!("===================================================================");
    println!("{:<34} | {:<8} | {:<10} | {:<8} | {:<8} | {:<8} | {:<8}",
        "Benchmark Scenario", "FPS", "Jitter", "1% Low", "P50 M2P", "P95 M2P", "Bitrate");
    println!("----------------------------------------------------------------------------------------------------");
    for r in [&res1, &res2] {
        println!("{:<34} | {:<8.1} | {:<8.2} ms | {:<7.2}% | {:<6.2} ms | {:<6.2} ms | {:.2} Mbps",
            r.suite_name, r.avg_fps, r.pacing_jitter_ms, r.stutter_1pct_rate, r.p50_m2p_ms, r.p95_m2p_ms, r.avg_bitrate_mbps);
    }
    println!("===================================================================\n");

    Ok(())
}
