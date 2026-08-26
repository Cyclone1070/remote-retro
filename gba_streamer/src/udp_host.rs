use anyhow::Result;
use std::net::UdpSocket;
use std::time::{Duration, Instant};

use crate::codec::PaletteEncoder;
use crate::core::RetroCore;

pub async fn run_udp_host(
    core_path: String,
    rom_path: String,
    bind_addr: String,
    client: Option<String>,
    frames: u64,
) -> Result<()> {
    println!("=== Starting GBA UDP Host (Native Adaptive Palette) ===");
    let mut core = RetroCore::load(&core_path, &rom_path)?;

    let socket = UdpSocket::bind(&bind_addr)?;
    socket.set_nonblocking(true)?;
    println!("Host listening on UDP: {}", bind_addr);

    let mut client_target = client;
    let mut frame_idx = 0u64;
    let mut encoder = PaletteEncoder::new();

    let mut matched_seq = 0u32;

    while frames == 0 || frame_idx < frames {
        let frame_start = Instant::now();

        let mut input_buf = [0u8; 16];
        while let Ok((len, src)) = socket.recv_from(&mut input_buf) {
            if len >= 14 {
                let seq = u32::from_le_bytes(input_buf[0..4].try_into().unwrap_or_default());
                let mask = (input_buf[12] as i16) | ((input_buf[13] as i16) << 8);
                matched_seq = seq;
                core.set_input(mask);
                client_target = Some(src.to_string());
            }
        }

        let (sim_us, raw_frame) = core.step();

        if !raw_frame.is_empty() {
            let t_enc = Instant::now();
            let (flag, payload) = encoder.encode(&raw_frame);
            let enc_us = t_enc.elapsed().as_micros() as u32;

            let chunk_size = 1024usize;
            let total_chunks = (payload.len() + chunk_size - 1) / chunk_size;
            for chunk_idx in 0..total_chunks {
                let start = chunk_idx * chunk_size;
                let end = (start + chunk_size).min(payload.len());
                let chunk_data = &payload[start..end];

                let mut packet = Vec::with_capacity(19 + chunk_data.len());
                packet.extend_from_slice(&(frame_idx as u32).to_le_bytes());
                packet.push(chunk_idx as u8);
                packet.push(total_chunks as u8);
                packet.extend_from_slice(&matched_seq.to_le_bytes());
                packet.extend_from_slice(&sim_us.to_le_bytes());
                packet.extend_from_slice(&enc_us.to_le_bytes());
                packet.push(flag);
                packet.extend_from_slice(chunk_data);

                if let Some(ref target) = client_target {
                    let _ = socket.send_to(&packet, target);
                }
            }
        }

        frame_idx += 1;
        let frame_budget = Duration::from_micros(16742);
        let elapsed = frame_start.elapsed();
        if elapsed < frame_budget {
            tokio::time::sleep(frame_budget - elapsed).await;
        }
    }

    Ok(())
}
