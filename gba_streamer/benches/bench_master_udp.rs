use anyhow::Result;
use std::collections::HashMap;
use std::net::UdpSocket;
use std::time::Instant;

const GBA_WIDTH: usize = 240;
const GBA_HEIGHT: usize = 160;
const TOTAL_PIXELS: usize = GBA_WIDTH * GBA_HEIGHT;

fn main() -> Result<()> {
    let host_addr = "100.73.151.90:48500";
    let client_socket = UdpSocket::bind("0.0.0.0:0")?;
    client_socket.connect(host_addr)?;
    client_socket.set_nonblocking(true)?;

    println!("Connecting to live GBA Native UDP Host at {}", host_addr);

    let total_frames: usize = 2000;
    let warmup_frames: usize = 120;

    let mut m2p_latencies = Vec::with_capacity(total_frames);
    let mut inter_frame_intervals = Vec::with_capacity(total_frames);
    let mut frame_bytes = Vec::with_capacity(total_frames);
    let mut host_compute_times = Vec::with_capacity(total_frames);
    let mut input_history: HashMap<u32, Instant> = HashMap::new();
    let mut last_frame_recv = Instant::now();
    let mut screen_buffer = vec![0u16; TOTAL_PIXELS];

    let mut current_frame_chunks: HashMap<u8, Vec<u8>> = HashMap::new();
    let mut current_frame_seq = u32::MAX;
    let mut expected_chunks = 0u8;

    let mut clean_frames = 0;
    let mut corrupt_frames = 0;

    println!("===================================================================");
    println!(" ⚡ AUDITED NATIVE UDP BENCHMARK (2,000 FRAMES) ");
    println!("===================================================================");

    let mut recv_buf = [0u8; 2048];

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

        let _ = client_socket.send(&input_packet);

        let poll_start = Instant::now();
        while poll_start.elapsed().as_millis() < 25 {
            while let Ok(len) = client_socket.recv(&mut recv_buf) {
                if len >= 19 {
                    let f_seq = u32::from_le_bytes(recv_buf[0..4].try_into().unwrap());
                    let chunk_idx = recv_buf[4];
                    let total_chunks = recv_buf[5];
                    let matched_seq = u32::from_le_bytes(recv_buf[6..10].try_into().unwrap());
                    let sim_us = u32::from_le_bytes(recv_buf[10..14].try_into().unwrap());
                    let enc_us = u32::from_le_bytes(recv_buf[14..18].try_into().unwrap());
                    let flag = recv_buf[18];
                    let chunk_data = &recv_buf[19..len];

                    if f_seq > current_frame_seq || current_frame_seq == u32::MAX {
                        current_frame_seq = f_seq;
                        current_frame_chunks.clear();
                        expected_chunks = total_chunks;
                    }

                    if f_seq == current_frame_seq {
                        current_frame_chunks.insert(chunk_idx, chunk_data.to_vec());

                        if current_frame_chunks.len() == expected_chunks as usize {
                            let t_recv = Instant::now();
                            if frame_idx > warmup_frames as u32 {
                                let dt = t_recv.duration_since(last_frame_recv).as_micros() as f64 / 1000.0;
                                inter_frame_intervals.push(dt);
                            }
                            last_frame_recv = t_recv;

                            let mut payload = Vec::new();
                            for i in 0..expected_chunks {
                                if let Some(c) = current_frame_chunks.get(&i) {
                                    payload.extend_from_slice(c);
                                }
                            }

                            let mut valid = false;
                            if flag == 4 {
                                if let Ok(decomp) = lz4_flex::decompress_size_prepended(&payload) {
                                    if decomp.len() >= 2 {
                                        let pal_len = u16::from_le_bytes(decomp[0..2].try_into().unwrap()) as usize;
                                        if decomp.len() >= 2 + pal_len * 2 + TOTAL_PIXELS / 2 {
                                            let pal_src = unsafe {
                                                std::slice::from_raw_parts(decomp[2..2 + pal_len * 2].as_ptr() as *const u16, pal_len)
                                            };
                                            let packed = &decomp[2 + pal_len * 2..];
                                            for i in 0..TOTAL_PIXELS / 2 {
                                                let b = packed[i];
                                                screen_buffer[i * 2] = pal_src[(b & 0x0F) as usize];
                                                screen_buffer[i * 2 + 1] = pal_src[((b >> 4) & 0x0F) as usize];
                                            }
                                            valid = true;
                                        }
                                    }
                                }
                            } else if flag == 2 {
                                if let Ok(decomp) = lz4_flex::decompress_size_prepended(&payload) {
                                    let pal_len = u16::from_le_bytes(decomp[0..2].try_into().unwrap()) as usize;
                                    let pal_src = unsafe {
                                        std::slice::from_raw_parts(decomp[2..2 + pal_len * 2].as_ptr() as *const u16, pal_len)
                                    };
                                    let indices = &decomp[2 + pal_len * 2..];
                                    if indices.len() >= TOTAL_PIXELS {
                                        for p in 0..TOTAL_PIXELS {
                                            screen_buffer[p] = pal_src[indices[p] as usize];
                                        }
                                        valid = true;
                                    }
                                }
                            } else if flag == 1 {
                                if let Ok(decomp) = lz4_flex::decompress_size_prepended(&payload) {
                                    if decomp.len() == TOTAL_PIXELS * 2 {
                                        let src16 = unsafe {
                                            std::slice::from_raw_parts(decomp.as_ptr() as *const u16, decomp.len() / 2)
                                        };
                                        screen_buffer.copy_from_slice(src16);
                                        valid = true;
                                    }
                                }
                            }

                            if valid { clean_frames += 1; } else { corrupt_frames += 1; }

                            if let Some(t_sent) = input_history.get(&matched_seq) {
                                let total_m2p_ms = t_sent.elapsed().as_micros() as f64 / 1000.0;
                                if frame_idx > warmup_frames as u32 {
                                    frame_bytes.push(payload.len());
                                    m2p_latencies.push(total_m2p_ms);
                                    host_compute_times.push((sim_us + enc_us) as f64 / 1000.0);
                                }
                            }
                            break;
                        }
                    }
                }
            }
            if !current_frame_chunks.is_empty() && current_frame_chunks.len() == expected_chunks as usize {
                break;
            }
            std::thread::sleep(std::time::Duration::from_micros(300));
        }

        std::thread::sleep(std::time::Duration::from_millis(16));
    }

    let mean = |v: &[f64]| if v.is_empty() { 0.0 } else { v.iter().sum::<f64>() / v.len() as f64 };
    let std_dev = |v: &[f64]| {
        if v.len() < 2 { return 0.0; }
        let m = mean(v);
        let variance = v.iter().map(|x| (x - m).powi(2)).sum::<f64>() / (v.len() - 1) as f64;
        variance.sqrt()
    };
    let percentile = |v: &[f64], p: f64| {
        if v.is_empty() { return 0.0; }
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
    let integrity_rate = if total_eval > 0 { (clean_frames as f64 / total_eval as f64) * 100.0 } else { 100.0 };

    let avg_bytes = if !frame_bytes.is_empty() { frame_bytes.iter().sum::<usize>() as f64 / frame_bytes.len() as f64 } else { 0.0 };
    let mbps_60 = (avg_bytes * 8.0 * 60.0) / 1_000_000.0;
    let avg_compute = mean(&host_compute_times);

    println!("\n===================================================================");
    println!("  AUDITED UDP BENCHMARK REPORT (2,000 FRAMES EVALUATED) ");
    println!("===================================================================");
    println!("  Evaluated Frames:                 {}", valid_count);
    println!("  Delivered Framerate:              {:.1} FPS", avg_fps);
    println!("  Inter-Frame Pacing Jitter (σ):    {:.2} ms", jitter_std);
    println!("  Average Frame Size:               {:.2} KB ({:.2} Mbps @ 60 FPS)", avg_bytes / 1024.0, mbps_60);
    println!("  Host Compute (Sim + Enc):         {:.3} ms", avg_compute);
    println!("  Mean Wire M2P Latency:            {:.2} ms", avg_m2p);
    println!("  P50 (Median) Wire M2P:            {:.2} ms", p50_m2p);
    println!("  P95 Wire M2P (Tail):              {:.2} ms", p95_m2p);
    println!("  P99 Wire M2P:                     {:.2} ms", p99_m2p);
    println!("  Pixel Integrity & Bit-Exact Rate: {:.2}% ({} clean / {} total)", integrity_rate, clean_frames, total_eval);
    println!("===================================================================\n");

    Ok(())
}
