use anyhow::Result;
use futures_util::{SinkExt, StreamExt};
use std::sync::{atomic::{AtomicI16, AtomicU32, AtomicU64, Ordering}, Arc};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use warp::ws::Ws;
use warp::Filter;

use crate::codec::{AudioEncoder, PaletteEncoder};
use crate::core::RetroCore;

const BROWSER_HTML: &str = include_str!("../static/index.html");

pub async fn run_web_host(core_path: String, rom_path: String, bind_addr: String) -> Result<()> {
    println!("=== Starting GBA WebHost (A/V Synchronized Bit-Exact Stream) ===");
    let mut core = RetroCore::load(&core_path, &rom_path)?;

    let (tx, _rx) = tokio::sync::broadcast::channel::<Arc<Vec<u8>>>(64);
    let tx_arc = Arc::new(tx);
    let tx_producer = tx_arc.clone();

    let last_input_seq = Arc::new(AtomicU32::new(0));
    let last_input_ts_us = Arc::new(AtomicU64::new(0));
    let input_mask = Arc::new(AtomicI16::new(0));
    let latched_mask = Arc::new(AtomicI16::new(0));
    let runahead_frames = Arc::new(std::sync::atomic::AtomicU8::new(core.runahead_frames));

    let seq_producer = last_input_seq.clone();
    let ts_producer = last_input_ts_us.clone();
    let mask_producer = input_mask.clone();
    let latched_producer = latched_mask.clone();
    let runahead_producer = runahead_frames.clone();

    std::thread::spawn(move || {
        let mut encoder = PaletteEncoder::new();
        let mut audio_enc = AudioEncoder::new(44100);

        let mut next_frame_time = Instant::now();
        let frame_budget = Duration::from_nanos(16_666_667); // 60.0000 FPS VSYNC-matched clock

        loop {
            next_frame_time += frame_budget;

            let matched_seq = seq_producer.load(Ordering::Relaxed);
            let matched_t_us = ts_producer.load(Ordering::Relaxed);
            let current_mask = mask_producer.load(Ordering::Relaxed);
            let latched = latched_producer.swap(0, Ordering::Relaxed);
            let effective_mask = current_mask | latched;
            let desired_runahead = runahead_producer.load(Ordering::Relaxed);

            if core.runahead_frames != desired_runahead {
                core.set_runahead_frames(desired_runahead);
                println!("⚡ Live Run-Ahead switched to: {}F", desired_runahead);
            }

            core.set_input(effective_mask);
            let (sim_us, raw_frame, audio_samples) = core.step();

            if !raw_frame.is_empty() {
                let t_enc = Instant::now();
                let (flag, video_payload) = encoder.encode(&raw_frame);
                let enc_us = t_enc.elapsed().as_micros() as u32;

                let t_audio = Instant::now();
                if !audio_samples.is_empty() {
                    audio_enc.push_samples(&audio_samples);
                }
                let audio_payload = audio_enc.flush_frame_lz4().unwrap_or_default();
                let audio_enc_us = t_audio.elapsed().as_micros() as u32;

                let now_us = SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_micros() as u64;

                let audio_len = audio_payload.len() as u16;
                let mut packet = Vec::with_capacity(35 + audio_payload.len() + video_payload.len());
                packet.extend_from_slice(&matched_seq.to_le_bytes());
                packet.extend_from_slice(&matched_t_us.to_le_bytes());
                packet.extend_from_slice(&sim_us.to_le_bytes());
                packet.extend_from_slice(&audio_enc_us.to_le_bytes());
                packet.extend_from_slice(&enc_us.to_le_bytes());
                // Embed 7 bytes timestamp + 1 byte active runahead frames at index 31
                let now_ts_7bytes = (now_us & 0x00FF_FFFF_FFFF_FFFF) as u64;
                let ts_and_runahead = now_ts_7bytes | ((desired_runahead as u64) << 56);
                packet.extend_from_slice(&ts_and_runahead.to_le_bytes());
                packet.push(flag);
                packet.extend_from_slice(&audio_len.to_le_bytes());
                packet.extend_from_slice(&audio_payload);
                packet.extend_from_slice(&video_payload);

                let _ = tx_producer.send(Arc::new(packet));
            }

            let now = Instant::now();
            if now < next_frame_time {
                let remaining = next_frame_time - now;
                if remaining > Duration::from_millis(2) {
                    std::thread::sleep(remaining - Duration::from_millis(2));
                }
                while Instant::now() < next_frame_time {
                    std::hint::spin_loop();
                }
            } else if now - next_frame_time > frame_budget * 2 {
                next_frame_time = now;
            }
        }
    });

    let addr: std::net::SocketAddr = bind_addr.parse()?;
    println!("WebHost running on http://{}", addr);

    let html_route = warp::path::end().map(|| warp::reply::html(BROWSER_HTML));
    let ping_route = warp::path("ping").map(|| warp::reply::html("pong"));

    let tx_for_ws = tx_arc.clone();
    let seq_consumer = last_input_seq.clone();
    let ts_consumer = last_input_ts_us.clone();
    let mask_consumer = input_mask.clone();
    let latched_consumer = latched_mask.clone();
    let runahead_consumer = runahead_frames.clone();

    let ws_route = warp::path("ws")
        .and(warp::ws())
        .map(move |ws: Ws| {
            let mut client_rx = tx_for_ws.subscribe();
            let seq = seq_consumer.clone();
            let ts = ts_consumer.clone();
            let mask = mask_consumer.clone();
            let latched = latched_consumer.clone();
            let runahead = runahead_consumer.clone();

            ws.on_upgrade(move |websocket| async move {
                    let (mut ws_sender, mut ws_receiver) = websocket.split();
                    println!("Browser client connected via WebSocket (TCP_NODELAY active)!");

                    tokio::spawn(async move {
                        while let Some(Ok(msg)) = ws_receiver.next().await {
                            if msg.is_binary() {
                                let bytes = msg.as_bytes();
                                if bytes.len() == 3 && bytes[0] == 0xAA && bytes[1] == 0x52 {
                                    let target_f = bytes[2];
                                    runahead.store(target_f, Ordering::Relaxed);
                                    println!("Client set Run-Ahead to: {}F", target_f);
                                } else if bytes.len() >= 14 {
                                    let s = u32::from_le_bytes(bytes[0..4].try_into().unwrap_or_default());
                                    let t = u64::from_le_bytes(bytes[4..12].try_into().unwrap_or_default());
                                    let m = u16::from_le_bytes(bytes[12..14].try_into().unwrap_or_default()) as i16;
                                    seq.store(s, Ordering::Relaxed);
                                    ts.store(t, Ordering::Relaxed);
                                    mask.store(m, Ordering::Relaxed);
                                    if m != 0 {
                                        latched.fetch_or(m, Ordering::Relaxed);
                                    }
                                } else if bytes.len() >= 2 {
                                    let m = u16::from_le_bytes(bytes[0..2].try_into().unwrap_or_default()) as i16;
                                    mask.store(m, Ordering::Relaxed);
                                    if m != 0 {
                                        latched.fetch_or(m, Ordering::Relaxed);
                                    }
                                }
                            }
                        }
                    });

                    loop {
                        match client_rx.recv().await {
                            Ok(mut packet) => {
                                // Drain stale frames to guarantee zero queuing delay
                                while let Ok(newer_packet) = client_rx.try_recv() {
                                    packet = newer_packet;
                                }
                                if ws_sender
                                    .send(warp::ws::Message::binary((*packet).clone()))
                                    .await
                                    .is_err()
                                {
                                    break;
                                }
                            }
                            Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => {
                                // Dropped frames to catch up with slow connection, continue
                                continue;
                            }
                            Err(tokio::sync::broadcast::error::RecvError::Closed) => {
                                break;
                            }
                        }
                    }
                    println!("Client disconnected.");
                })
        });

    let routes = html_route.or(ping_route).or(ws_route);
    warp::serve(routes).run(addr).await;

    Ok(())
}
