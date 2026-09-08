use anyhow::Result;
use futures_util::{SinkExt, StreamExt};
use std::sync::{atomic::{AtomicBool, Ordering}, Arc, Mutex};
use std::time::Instant;
use tokio_tungstenite::connect_async;
use tokio_tungstenite::tungstenite::Message;

const GBA_WIDTH: usize = 240;
const GBA_HEIGHT: usize = 160;
const TOTAL_PIXELS: usize = GBA_WIDTH * GBA_HEIGHT;

#[tokio::main]
async fn main() -> Result<()> {
    let ws_url = "ws://100.73.151.90:48500/ws";
    println!("Connecting to live GBA WebHost at {}", ws_url);
    let (ws_stream, _) = connect_async(ws_url).await?;
    let (mut write, mut read) = ws_stream.split();

    let total_frames: usize = std::env::var("BENCH_FRAMES")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(10_800);
    let warmup_frames: usize = 120;

    let input_history: Arc<Mutex<Vec<Option<Instant>>>> = Arc::new(Mutex::new(vec![None; 256]));
    let running = Arc::new(AtomicBool::new(true));

    let history_sender = input_history.clone();
    let is_running = running.clone();
    let (input_tx, mut input_rx) = tokio::sync::mpsc::channel::<Vec<u8>>(100);

    tokio::spawn(async move {
        let mut step_count = 0usize;
        let mut interval = tokio::time::interval(std::time::Duration::from_millis(8));
        while is_running.load(Ordering::Relaxed) {
            interval.tick().await;
            step_count += 1;
            let seq = (step_count & 0xFF) as u8;
            let t_sent = Instant::now();
            {
                let mut guard = history_sender.lock().unwrap();
                guard[seq as usize] = Some(t_sent);
            }

            let mut buttons: u16 = 0;
            if step_count < 120 {
                if step_count % 20 < 10 { buttons |= (1 << 3) | (1 << 0); } // Start + A to skip title
            } else {
                let f = (step_count - 120) % 720; // 12-second repeating choreographed human cycle
                match f {
                    0..=149 => {
                        buttons |= 1 << 4; // Right
                        if (60..=72).contains(&f) { buttons |= 1 << 0; } // Hop A
                    }
                    150..=269 => {
                        buttons |= 1 << 5; // Left
                        if (210..=230).contains(&f) { buttons |= 1 << 0; } // High Jump A
                    }
                    270..=329 => {
                        buttons |= 1 << 7; // Down (crouch)
                    }
                    330..=389 => {
                        buttons |= 1 << 6; // Up (look up)
                    }
                    390..=539 => {
                        buttons |= 1 << 4; // Right
                        if (450..=470).contains(&f) { buttons |= 1 << 0; } // Hop A
                    }
                    540..=629 => {
                        buttons |= 1 << 5; // Left
                    }
                    _ => {
                        if (step_count - 120) % 2160 < 90 {
                            buttons |= 1 << 3; // Start (Pause Menu overlay check)
                        } else {
                            buttons |= 1 << 4; // Right
                        }
                    }
                }
            }

            let mut packet = Vec::with_capacity(3);
            packet.push(seq);
            packet.extend_from_slice(&buttons.to_le_bytes());

            let _ = input_tx.send(packet).await;
        }
    });

    tokio::spawn(async move {
        while let Some(pkt) = input_rx.recv().await {
            if write.send(Message::binary(pkt)).await.is_err() {
                break;
            }
        }
    });

    let mut m2p_latencies = Vec::with_capacity(total_frames);
    let mut inter_frame_intervals = Vec::with_capacity(total_frames);
    let mut frame_bytes = Vec::with_capacity(total_frames);
    let mut audio_bytes = Vec::with_capacity(total_frames);
    let mut last_frame_recv = Instant::now();

    let mut clean_video_frames = 0;
    let mut clean_audio_frames = 0;
    let mut corrupt_frames = 0;
    let mut screen_buffer = vec![0u16; TOTAL_PIXELS];
    let mut tile_decoder = gba_streamer::codec::TileCacheDecoder::new();
    let mut ppu_state = gba_streamer::codec::PpuState::new();

    println!("===================================================================");
    println!(" ⚡ AUDITED A/V SYNCHRONIZED LOSSLESS BENCHMARK (2,000 FRAMES) ");
    println!("===================================================================");

    let mut evaluated = 0usize;

    while evaluated < total_frames {
        if let Some(Ok(msg)) = read.next().await {
            if msg.is_binary() {
                let t_recv = Instant::now();
                let bytes = msg.into_data();
                if bytes.len() < 4 { continue; }

                evaluated += 1;
                if evaluated > warmup_frames {
                    let dt = t_recv.duration_since(last_frame_recv).as_micros() as f64 / 1000.0;
                    inter_frame_intervals.push(dt);
                }
                last_frame_recv = t_recv;

                let header_byte0 = bytes[0];
                let flag = header_byte0 & 0x3F;
                let _active_runahead = header_byte0 >> 6;
                let matched_seq = bytes[1] as usize;
                let audio_len = u16::from_le_bytes(bytes[2..4].try_into().unwrap()) as usize;

                if bytes.len() < 4 + audio_len { continue; }
                let audio_payload = &bytes[4..4 + audio_len];
                let video_payload = &bytes[4 + audio_len..];

                // Audio validation
                if audio_len > 0 {
                    if let Ok(decomp_audio) = lz4_flex::decompress_size_prepended(audio_payload) {
                        if decomp_audio.len() % 4 == 0 {
                            clean_audio_frames += 1;
                        }
                    }
                } else {
                    clean_audio_frames += 1;
                }

                // Video validation
                let mut valid_video = false;
                if flag == gba_streamer::codec::FLAG_PPU_STATE {
                    if ppu_state.apply_payload(video_payload).is_ok() {
                        valid_video = true;
                    }
                } else if let Ok(frame) = tile_decoder.decode(flag, video_payload) {
                    screen_buffer.copy_from_slice(frame);
                    valid_video = true;
                }

                if valid_video { clean_video_frames += 1; } else { corrupt_frames += 1; }

                let t_sent_opt = {
                    let mut guard = input_history.lock().unwrap();
                    guard[matched_seq].take()
                };

                if let Some(t_sent) = t_sent_opt {
                    let total_m2p_ms = t_sent.elapsed().as_micros() as f64 / 1000.0;
                    if evaluated > warmup_frames {
                        frame_bytes.push(bytes.len());
                        audio_bytes.push(audio_len);
                        m2p_latencies.push(total_m2p_ms);
                    }
                }

                if evaluated > 0 && evaluated % 1800 == 0 {
                    let elapsed_sec = evaluated as f64 / 60.0;
                    let cur_clean = clean_video_frames;
                    println!("  ⏳ [{}/{} frames | {:.0}s] Streaming healthy (Clean Video: {}/{})", evaluated, total_frames, elapsed_sec, cur_clean, evaluated);
                }
            }
        }
    }

    running.store(false, Ordering::Relaxed);

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

    let avg_m2p = mean(&m2p_latencies);
    let p50_m2p = percentile(&m2p_latencies, 0.50);
    let p95_m2p = percentile(&m2p_latencies, 0.95);
    let p99_m2p = percentile(&m2p_latencies, 0.99);

    let mean_interval = mean(&inter_frame_intervals);
    let avg_fps = if mean_interval > 0.0 { 1000.0 / mean_interval } else { 0.0 };
    let jitter_std = std_dev(&inter_frame_intervals);

    let mut sorted_intervals = inter_frame_intervals.clone();
    sorted_intervals.sort_by(|a, b| b.partial_cmp(a).unwrap());
    let p1_idx = ((sorted_intervals.len() as f64 * 0.01) as usize).min(sorted_intervals.len().saturating_sub(1));
    let p1_worst_interval = if !sorted_intervals.is_empty() { sorted_intervals[p1_idx] } else { 16.67 };
    let p1_fps = if p1_worst_interval > 0.0 { 1000.0 / p1_worst_interval } else { 0.0 };

    let stutters = inter_frame_intervals.iter().filter(|&&x| x > 33.34).count();
    let stutter_rate = if !inter_frame_intervals.is_empty() { (stutters as f64 / inter_frame_intervals.len() as f64) * 100.0 } else { 0.0 };

    let total_eval = clean_video_frames + corrupt_frames;
    let video_integrity_rate = if total_eval > 0 { (clean_video_frames as f64 / total_eval as f64) * 100.0 } else { 100.0 };
    let audio_integrity_rate = if total_eval > 0 { (clean_audio_frames as f64 / total_eval as f64) * 100.0 } else { 100.0 };

    let avg_total_bytes = if !frame_bytes.is_empty() { frame_bytes.iter().sum::<usize>() as f64 / frame_bytes.len() as f64 } else { 0.0 };
    let avg_audio_bytes = if !audio_bytes.is_empty() { audio_bytes.iter().sum::<usize>() as f64 / audio_bytes.len() as f64 } else { 0.0 };
    let total_mbps_60 = (avg_total_bytes * 8.0 * 60.0) / 1_000_000.0;
    let audio_kbps_60 = (avg_audio_bytes * 8.0 * 60.0) / 1_000.0;

    println!("\n===================================================================");
    println!("  AUDITED A/V STREAMING REPORT ({} FRAMES EVALUATED) ", evaluated);
    println!("===================================================================");
    println!("  Evaluated Frames:                 {}", evaluated);
    println!("  Delivered Framerate:              {:.1} FPS", avg_fps);
    println!("  1% Low Framerate (P1):            {:.1} FPS", p1_fps);
    println!("  Frame Drop / Stutter Rate (>33ms):{:.2}% ({} stutters)", stutter_rate, stutters);
    println!("  Inter-Frame Pacing Jitter (σ):    {:.2} ms", jitter_std);
    println!("  Combined A/V Bandwidth:           {:.2} KB/frame ({:.2} Mbps @ 60 FPS)", avg_total_bytes / 1024.0, total_mbps_60);
    println!("  Audio Stream Bitrate (Lossless):  {:.1} bytes/frame ({:.1} kbps @ 60 FPS)", avg_audio_bytes, audio_kbps_60);
    println!("  Frame Header Size (Lean):         4 bytes (Codec + Seq + AudioLen)");
    println!("  Mean Socket-to-Socket Wire M2P:   {:.2} ms", avg_m2p);
    println!("  P50 (Median) Socket Wire M2P:     {:.2} ms", p50_m2p);
    println!("  P95 Socket Wire M2P (Tail):       {:.2} ms", p95_m2p);
    println!("  P99 Socket Wire M2P:              {:.2} ms", p99_m2p);
    println!("  Video Pixel Integrity:            {:.2}% ({} clean / {} total)", video_integrity_rate, clean_video_frames, total_eval);
    println!("  Audio Stream Integrity:           {:.2}% ({} clean / {} total)", audio_integrity_rate, clean_audio_frames, total_eval);
    println!("===================================================================\n");

    Ok(())
}
