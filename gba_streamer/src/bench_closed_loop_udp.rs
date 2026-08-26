use anyhow::Result;
use std::collections::HashMap;
use std::net::UdpSocket;
use std::time::{Duration, Instant};

const GBA_WIDTH: usize = 240;
const GBA_HEIGHT: usize = 160;

fn main() -> Result<()> {
    let host = "100.73.151.90:48500";
    let bind = "0.0.0.0:48502";
    println!("Connecting to live Native UDP Streamer at {}", host);

    let socket = UdpSocket::bind(bind)?;
    socket.set_read_timeout(Some(Duration::from_millis(500)))?;

    let total_frames: usize = 2000;
    let warmup_frames: usize = 120;

    let mut m2p_latencies = Vec::with_capacity(total_frames);
    let mut inter_frame_intervals = Vec::with_capacity(total_frames);
    let mut sim_times = Vec::with_capacity(total_frames);
    let mut copy_times = Vec::with_capacity(total_frames);
    let mut enc_times = Vec::with_capacity(total_frames);
    let mut dec_times = Vec::with_capacity(total_frames);
    let mut render_times = Vec::with_capacity(total_frames);
    let mut frame_bytes = Vec::with_capacity(total_frames);

    let mut input_history: HashMap<u32, Instant> = HashMap::new();
    let mut last_frame_recv = Instant::now();
    let mut screen_buffer = vec![0u32; GBA_WIDTH * GBA_HEIGHT];

    let mut recv_buf = vec![0u8; 65536];

    println!("===================================================================");
    println!(" STARTING 2,000-FRAME NATIVE UDP CLOSED-LOOP BENCHMARK ");
    println!("===================================================================");
    println!("  Transport: Direct Non-Blocking UDP (Zero TCP Head-of-Line Blocking)");
    println!("  Target Host: {}", host);
    println!("  Total Frames: {} (~33.3 seconds continuous play)", total_frames);
    println!("  Warm-up: {} frames (bypasses title screen into Level 1)", warmup_frames);
    println!("-------------------------------------------------------------------");

    for frame_idx in 0..total_frames as u32 {
        let t_input_sent = Instant::now();
        input_history.insert(frame_idx, t_input_sent);

        // Active gameplay controller bot
        let mut buttons: u16 = 0;
        if frame_idx < warmup_frames as u32 {
            if frame_idx % 20 < 10 {
                buttons |= 1 << 3; // START
                buttons |= 1 << 0; // A
            }
        } else {
            let active_f = frame_idx - warmup_frames as u32;
            if active_f % 80 < 60 { buttons |= 1 << 4; } // Right
            else { buttons |= 1 << 5; } // Left
            if active_f % 25 < 8 { buttons |= 1 << 0; } // Jump (A)
            if active_f % 45 < 12 { buttons |= 1 << 1; } // Attack (B)
        }

        // Send 14-byte input packet over UDP
        let mut input_packet = Vec::with_capacity(14);
        input_packet.extend_from_slice(&frame_idx.to_le_bytes());
        let now_us = t_input_sent.elapsed().as_micros() as u64;
        input_packet.extend_from_slice(&now_us.to_le_bytes());
        input_packet.extend_from_slice(&buttons.to_le_bytes());

        let _ = socket.send_to(&input_packet, host);

        // Drain any socket backlog to ensure we process only the freshest frame
        let mut latest_recv_len = 0;
        let mut received_any = false;

        // Non-blocking drain loop
        socket.set_nonblocking(true)?;
        while let Ok((len, _)) = socket.recv_from(&mut recv_buf) {
            latest_recv_len = len;
            received_any = true;
        }

        // If no frame arrived yet in non-blocking check, wait blocking for next frame
        if !received_any {
            socket.set_nonblocking(false)?;
            if let Ok((len, _)) = socket.recv_from(&mut recv_buf) {
                latest_recv_len = len;
                received_any = true;
            }
        }

        if received_any && latest_recv_len >= 32 {
            let t_recv = Instant::now();

            if frame_idx > warmup_frames as u32 {
                let dt = t_recv.duration_since(last_frame_recv).as_micros() as f64 / 1000.0;
                inter_frame_intervals.push(dt);
            }
            last_frame_recv = t_recv;

            let matched_seq = u32::from_le_bytes(recv_buf[0..4].try_into().unwrap());
            let sim_us = u32::from_le_bytes(recv_buf[12..16].try_into().unwrap());
            let copy_us = u32::from_le_bytes(recv_buf[16..20].try_into().unwrap());
            let enc_us = u32::from_le_bytes(recv_buf[20..24].try_into().unwrap());
            let compressed = &recv_buf[32..latest_recv_len];

            // Client LZ4 decode
            let t_dec_start = Instant::now();
            let decomp = lz4_flex::decompress_size_prepended(compressed).unwrap();
            let dec_dur = t_dec_start.elapsed().as_micros() as f64 / 1000.0;

            // Client Render Blit
            let t_render_start = Instant::now();
            let src32 = unsafe {
                std::slice::from_raw_parts(
                    decomp.as_ptr() as *const u32,
                    decomp.len() / 4,
                )
            };
            for i in 0..GBA_WIDTH * GBA_HEIGHT {
                let p = src32[i];
                let r = (p >> 16) & 0xFF;
                let g = (p >> 8) & 0xFF;
                let b = p & 0xFF;
                screen_buffer[i] = (255 << 24) | (b << 16) | (g << 8) | r;
            }
            let render_dur = t_render_start.elapsed().as_micros() as f64 / 1000.0;

            if let Some(t_sent) = input_history.get(&matched_seq) {
                let total_m2p_ms = t_sent.elapsed().as_micros() as f64 / 1000.0;
                if frame_idx > warmup_frames as u32 {
                    frame_bytes.push(compressed.len());
                    m2p_latencies.push(total_m2p_ms);
                    sim_times.push(sim_us as f64 / 1000.0);
                    copy_times.push(copy_us as f64 / 1000.0);
                    enc_times.push(enc_us as f64 / 1000.0);
                    dec_times.push(dec_dur);
                    render_times.push(render_dur);
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
    let interval_count = inter_frame_intervals.len();

    let avg_m2p = mean(&m2p_latencies);
    let p50_m2p = percentile(&m2p_latencies, 0.50);
    let p90_m2p = percentile(&m2p_latencies, 0.90);
    let p95_m2p = percentile(&m2p_latencies, 0.95);
    let p99_m2p = percentile(&m2p_latencies, 0.99);
    let max_m2p = percentile(&m2p_latencies, 1.00);

    let mean_interval = mean(&inter_frame_intervals);
    let avg_fps = 1000.0 / mean_interval;
    let jitter_std = std_dev(&inter_frame_intervals);
    let p99_interval = percentile(&inter_frame_intervals, 0.99);

    let on_time = inter_frame_intervals.iter().filter(|&&dt| dt < 18.0).count();
    let one_frame_late = inter_frame_intervals.iter().filter(|&&dt| dt >= 18.0 && dt <= 33.33).count();
    let one_pct_lows = inter_frame_intervals.iter().filter(|&&dt| dt > 33.33).count();
    let severe_freezes = inter_frame_intervals.iter().filter(|&&dt| dt > 50.0).count();

    let avg_raw_bytes = frame_bytes.iter().sum::<usize>() as f64 / frame_bytes.len() as f64;
    let avg_kb = avg_raw_bytes / 1024.0;
    let peak_kb = frame_bytes.iter().cloned().max().unwrap_or(0) as f64 / 1024.0;
    let mbps_60 = (avg_raw_bytes * 8.0 * 60.0) / 1_000_000.0;
    let compression_ratio = 153600.0 / avg_raw_bytes;

    let avg_sim = mean(&sim_times);
    let avg_copy = mean(&copy_times);
    let avg_enc = mean(&enc_times);
    let avg_dec = mean(&dec_times);
    let avg_rend = mean(&render_times);
    let avg_compute_total = avg_sim + avg_copy + avg_enc + avg_dec + avg_rend;
    let avg_net_transit = avg_m2p - avg_compute_total;

    println!("\n===================================================================");
    println!("  UDP ZERO-QUEUE 2,000-FRAME ACTIVE GAMEPLAY RESULTS ");
    println!("===================================================================");
    println!("1. EXECUTIVE OVERVIEW (RESPONSIVENESS & SMOOTHNESS):");
    println!("   - Total Measured Gameplay Frames: {}", valid_count);
    println!("   - Delivered Framerate:           {:.1} FPS (Mean Arrival: {:.2} ms)", avg_fps, mean_interval);
    println!("   - Frame Pacing Jitter (Std Dev): {:.2} ms", jitter_std);
    println!("   - P99 Frame Arrival Interval:    {:.2} ms", p99_interval);
    println!("   - Average Stream Bitrate:        {:.2} Mbps @ 60 FPS (Avg Frame: {:.2} KB, Peak: {:.2} KB)", mbps_60, avg_kb, peak_kb);
    println!("   - Framebuffer Compression Ratio: {:.1}x (153.6 KB raw -> {:.1} KB LZ4)", compression_ratio, avg_kb);
    println!("-------------------------------------------------------------------");
    println!("2. FRAME SMOOTHNESS & STUTTER BREAKDOWN:");
    println!("   - On-Time Frames (< 18 ms):       {} / {} ({:.2}%)", on_time, interval_count, (on_time as f64 / interval_count as f64) * 100.0);
    println!("   - 1-Frame Delayed (18 - 33.3 ms): {} / {} ({:.2}%)", one_frame_late, interval_count, (one_frame_late as f64 / interval_count as f64) * 100.0);
    println!("   - 1% Low Stutters (> 33.3 ms):    {} / {} ({:.2}%)", one_pct_lows, interval_count, (one_pct_lows as f64 / interval_count as f64) * 100.0);
    println!("   - Severe Freezes (> 50 ms):       {} / {} ({:.2}%)", severe_freezes, interval_count, (severe_freezes as f64 / interval_count as f64) * 100.0);
    println!("-------------------------------------------------------------------");
    println!("3. TRUE CLOSED-LOOP MOTION-TO-PHOTON (M2P) LATENCY:");
    println!("   - Mean M2P Latency:   {:.2} ms", avg_m2p);
    println!("   - P50 (Median) M2P:   {:.2} ms", p50_m2p);
    println!("   - P90 M2P:            {:.2} ms", p90_m2p);
    println!("   - P95 M2P:            {:.2} ms", p95_m2p);
    println!("   - P99 M2P:            {:.2} ms", p99_m2p);
    println!("   - Max M2P:            {:.2} ms", max_m2p);
    println!("-------------------------------------------------------------------");
    println!("4. COMPLETE 5-STAGE PIPELINE BREAKDOWN (MEASURED):");
    println!("   - Stage 1 + Stage 4: Total WAN Network Transit:   {:.2} ms ({:.1}%)", avg_net_transit, (avg_net_transit / avg_m2p) * 100.0);
    println!("   - Stage 2: Host Core Simulation (mGBA):           {:.3} ms ({:.1}%)", avg_sim, (avg_sim / avg_m2p) * 100.0);
    println!("   - Stage 2b: Host Frame Extraction & Buffer Copy:  {:.3} ms ({:.1}%)", avg_copy, (avg_copy / avg_m2p) * 100.0);
    println!("   - Stage 3: Host LZ4 Block Compression:            {:.3} ms ({:.1}%)", avg_enc, (avg_enc / avg_m2p) * 100.0);
    println!("   - Stage 5a: Client LZ4 Block Decompression:       {:.3} ms ({:.1}%)", avg_dec, (avg_dec / avg_m2p) * 100.0);
    println!("   - Stage 5b: Client Format Conversion & Screen Blit:{:.3} ms ({:.1}%)", avg_rend, (avg_rend / avg_m2p) * 100.0);
    println!("   - TOTAL PIPELINE COMPUTE OVERHEAD:                {:.3} ms ({:.1}%)", avg_compute_total, (avg_compute_total / avg_m2p) * 100.0);
    println!("===================================================================\n");

    Ok(())
}
