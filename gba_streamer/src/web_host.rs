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

    let (tx, _rx) = tokio::sync::broadcast::channel::<Arc<Vec<u8>>>(4);
    let tx_arc = Arc::new(tx);
    let tx_producer = tx_arc.clone();

    let last_input_seq = Arc::new(AtomicU32::new(0));
    let last_input_ts_us = Arc::new(AtomicU64::new(0));
    let input_mask = Arc::new(AtomicI16::new(0));

    let seq_producer = last_input_seq.clone();
    let ts_producer = last_input_ts_us.clone();
    let mask_producer = input_mask.clone();

    std::thread::spawn(move || {
        let mut encoder = PaletteEncoder::new();
        let mut audio_enc = AudioEncoder::new(44100);

        loop {
            let frame_start = Instant::now();

            let matched_seq = seq_producer.load(Ordering::Relaxed);
            let matched_t_us = ts_producer.load(Ordering::Relaxed);
            let current_mask = mask_producer.load(Ordering::Relaxed);

            core.set_input(current_mask);
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
                packet.extend_from_slice(&now_us.to_le_bytes());
                packet.push(flag);
                packet.extend_from_slice(&audio_len.to_le_bytes());
                packet.extend_from_slice(&audio_payload);
                packet.extend_from_slice(&video_payload);

                let _ = tx_producer.send(Arc::new(packet));
            }

            let frame_budget = Duration::from_micros(16742);
            let elapsed = frame_start.elapsed();
            if elapsed < frame_budget {
                std::thread::sleep(frame_budget - elapsed);
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

    let ws_route = warp::path("ws")
        .and(warp::ws())
        .map(move |ws: Ws| {
            let mut client_rx = tx_for_ws.subscribe();
            let seq = seq_consumer.clone();
            let ts = ts_consumer.clone();
            let mask = mask_consumer.clone();

            ws.on_upgrade(move |websocket| async move {
                let (mut ws_sender, mut ws_receiver) = websocket.split();
                println!("Browser client connected via WebSocket!");

                tokio::spawn(async move {
                    while let Some(Ok(msg)) = ws_receiver.next().await {
                        if msg.is_binary() {
                            let bytes = msg.as_bytes();
                            if bytes.len() >= 14 {
                                let s = u32::from_le_bytes(bytes[0..4].try_into().unwrap_or_default());
                                let t = u64::from_le_bytes(bytes[4..12].try_into().unwrap_or_default());
                                let m = (bytes[12] as i16) | ((bytes[13] as i16) << 8);
                                seq.store(s, Ordering::Relaxed);
                                ts.store(t, Ordering::Relaxed);
                                mask.store(m, Ordering::Relaxed);
                            } else if bytes.len() >= 2 {
                                let m = (bytes[0] as i16) | ((bytes[1] as i16) << 8);
                                mask.store(m, Ordering::Relaxed);
                            }
                        }
                    }
                });

                while let Ok(packet) = client_rx.recv().await {
                    if ws_sender
                        .send(warp::ws::Message::binary((*packet).clone()))
                        .await
                        .is_err()
                    {
                        break;
                    }
                }
                println!("Client disconnected.");
            })
        });

    let routes = html_route.or(ping_route).or(ws_route);
    warp::serve(routes).run(addr).await;

    Ok(())
}
