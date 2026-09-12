use anyhow::Result;
use futures_util::{SinkExt, StreamExt};
use std::path::PathBuf;
use std::sync::{atomic::{AtomicI16, AtomicU32, Ordering}, Arc, Mutex};
use std::time::{Duration, Instant};
use warp::ws::Ws;
use warp::Filter;

use crate::codec::{AudioEncoder, PaletteEncoder, PpuStateEncoder, StreamEngineMode, StreamProfiler, FLAG_PPU_STATE};
use crate::core::RetroCore;

const BROWSER_HTML: &str = include_str!("../static/index.html");
const BROWSER_WASM: &[u8] = include_bytes!("../static/gba_ppu.wasm");

pub struct RomInfoEntry {
    pub id: String,
    pub title: String,
    pub code: String,
    pub size_bytes: u64,
    pub path: PathBuf,
}

pub fn scan_available_roms() -> Vec<RomInfoEntry> {
    let mut entries = Vec::new();
    let mut seen_ids = std::collections::HashSet::new();

    let search_dirs = [
        PathBuf::from("/tmp"),
        PathBuf::from("./roms"),
        PathBuf::from("."),
    ];

    for dir in &search_dirs {
        if let Ok(read_dir) = std::fs::read_dir(dir) {
            for entry in read_dir.flatten() {
                let path = entry.path();
                if path.is_file() {
                    if let Some(ext) = path.extension() {
                        if ext.eq_ignore_ascii_case("gba") {
                            let filename = path.file_name().unwrap_or_default().to_string_lossy().to_string();
                            if !seen_ids.contains(&filename) {
                                seen_ids.insert(filename.clone());
                                let size = entry.metadata().map(|m| m.len()).unwrap_or(0);
                                let info = crate::runahead_db::inspect_gba_rom(path.to_str().unwrap_or_default());
                                entries.push(RomInfoEntry {
                                    id: filename,
                                    title: info.title,
                                    code: info.game_code,
                                    size_bytes: size,
                                    path,
                                });
                            }
                        }
                    }
                }
            }
        }
    }
    entries.sort_by(|a, b| a.title.cmp(&b.title));
    entries
}

pub async fn run_web_host(core_path: String, rom_path: String, bind_addr: String) -> Result<()> {
    println!("=== Starting GBA WebHost (A/V Synchronized Bit-Exact Stream) ===");
    let mut core = RetroCore::load(&core_path, &rom_path)?;

    let (tx, _rx) = tokio::sync::broadcast::channel::<Arc<Vec<u8>>>(256);
    let tx_arc = Arc::new(tx);
    let tx_producer = tx_arc.clone();

    let last_input_seq = Arc::new(AtomicU32::new(0));
    let input_mask = Arc::new(AtomicI16::new(0));
    let latched_mask = Arc::new(AtomicI16::new(0));
    let runahead_frames = Arc::new(std::sync::atomic::AtomicU8::new(core.runahead_frames));
    let force_keyframe = Arc::new(std::sync::atomic::AtomicBool::new(true));
    let probe_trigger = Arc::new(std::sync::atomic::AtomicI32::new(-1));
    let is_paused = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let step_trigger = Arc::new(std::sync::atomic::AtomicI32::new(-1));
    let rom_game_code = Arc::new(Mutex::new(core.rom_game_code.clone()));
    let switch_rom_req = Arc::new(Mutex::new(None::<String>));

    // Stream mode: 0 = Pixel XOR, 1 = PPU State, 2 = Auto-Profile (Default)
    let initial_mode = match std::env::var("STREAM_MODE").as_deref() {
        Ok("pixel") => 0u8,
        Ok("ppu") => 1u8,
        _ => 2u8,
    };
    let stream_mode = Arc::new(std::sync::atomic::AtomicU8::new(initial_mode));

    let seq_producer = last_input_seq.clone();
    let mask_producer = input_mask.clone();
    let latched_producer = latched_mask.clone();
    let runahead_producer = runahead_frames.clone();
    let force_keyframe_producer = force_keyframe.clone();
    let probe_producer = probe_trigger.clone();
    let is_paused_producer = is_paused.clone();
    let step_producer = step_trigger.clone();
    let stream_mode_producer = stream_mode.clone();
    let switch_rom_producer = switch_rom_req.clone();
    let rom_game_code_producer = rom_game_code.clone();

    std::thread::spawn(move || {
        let mut ppu_enc = PpuStateEncoder::new();
        let mut fallback_enc = PaletteEncoder::new();
        let mut audio_enc = AudioEncoder::new(44100);
        let mut profiler = StreamProfiler::new(StreamEngineMode::from(initial_mode));
        let mut prev_use_ppu = false;

        let mut vram_buf = vec![0u8; 98304];
        let mut pal_buf = vec![0u8; 1024];
        let mut oam_buf = vec![0u8; 1024];
        let mut io_buf = vec![0u8; 128];

        let mut next_frame_time = Instant::now();
        let frame_budget = Duration::from_nanos(16_666_667); // 60.0000 FPS VSYNC-matched clock
        let mut calib_checkpoint: Vec<u8> = vec![0u8; core.state_size()];
        let mut calib_vram = vec![0u8; 98304];
        let mut calib_pal = vec![0u8; 1024];
        let mut calib_oam = vec![0u8; 1024];
        let mut calib_io = vec![0u8; 128];
        let mut was_paused = false;

        loop {
            // Check for pending ROM switch request
            let switch_opt = switch_rom_producer.lock().unwrap().take();
            if let Some(target_rom_id) = switch_opt {
                println!("🔄 Switching active ROM to '{}'...", target_rom_id);
                let available = scan_available_roms();
                if let Some(entry) = available.iter().find(|e| e.id == target_rom_id || e.title.eq_ignore_ascii_case(&target_rom_id) || e.code.eq_ignore_ascii_case(&target_rom_id)) {
                    if let Ok(_) = core.reload_rom(entry.path.to_str().unwrap_or_default()) {
                        *rom_game_code_producer.lock().unwrap() = core.rom_game_code.clone();
                        let new_runahead = core.runahead_frames;
                        runahead_producer.store(new_runahead, Ordering::Relaxed);
                        ppu_enc.force_keyframe();
                        fallback_enc.force_keyframe();
                        force_keyframe_producer.store(true, Ordering::Relaxed);
                        is_paused_producer.store(false, Ordering::Relaxed);
                        was_paused = false;
                        calib_checkpoint = vec![0u8; core.state_size()];
                        println!("🎮 Active Game Switched: '{}' [{}] -> Run-Ahead: {}F", core.rom_title, core.rom_game_code, new_runahead);
                    }
                }
            }

            let is_p = is_paused_producer.load(Ordering::Relaxed);
            if is_p {
                if !was_paused {
                    was_paused = true;
                    core.save_state(&mut calib_checkpoint);
                    core.set_runahead_frames(0); // Calibration MUST run at 0F to accurately measure base engine lag
                    calib_vram.copy_from_slice(&vram_buf);
                    calib_pal.copy_from_slice(&pal_buf);
                    calib_oam.copy_from_slice(&oam_buf);
                    calib_io.copy_from_slice(&io_buf);
                }
                let step_req = step_producer.swap(-1, Ordering::Relaxed);
                if step_req == -1 {
                    std::thread::sleep(Duration::from_millis(8));
                    next_frame_time = Instant::now();
                    continue;
                }

                let (flag, video_payload, _enc_us) = if step_req == -2 {
                    // Rewind: reload checkpoint state and re-render base frame
                    core.load_state(&calib_checkpoint);
                    core.set_runahead_frames(0);
                    core.set_input(0);
                    ppu_enc.force_keyframe();
                    if RetroCore::has_ppu_memory() {
                        vram_buf.copy_from_slice(&calib_vram);
                        pal_buf.copy_from_slice(&calib_pal);
                        oam_buf.copy_from_slice(&calib_oam);
                        io_buf.copy_from_slice(&calib_io);
                        let payload = ppu_enc.encode(&oam_buf, &io_buf, &pal_buf, &vram_buf);
                        (FLAG_PPU_STATE, payload, 0)
                    } else {
                        let raw_frame = crate::core::LAST_FRAME_16.lock().unwrap().clone();
                        let (f, p) = fallback_enc.encode(&raw_frame);
                        (f, p, 0)
                    }
                } else {
                    // Advance exactly 1 frame forward with test input at 0F
                    core.set_runahead_frames(0);
                    core.set_input(step_req as i16);
                    let (_sim_us, _audio_samples, ppu_ok) = core.step_ppu(
                        &mut vram_buf,
                        &mut pal_buf,
                        &mut oam_buf,
                        &mut io_buf,
                    );
                    let cur_m = stream_mode_producer.load(Ordering::Relaxed);
                    profiler.set_setting(StreamEngineMode::from(cur_m));
                    let use_ppu = profiler.should_use_ppu(&io_buf, ppu_ok);
                    if use_ppu != prev_use_ppu {
                        ppu_enc.force_keyframe();
                        fallback_enc.force_keyframe();
                        prev_use_ppu = use_ppu;
                    }
                    if use_ppu {
                        let t_enc = Instant::now();
                        let payload = ppu_enc.encode(&oam_buf, &io_buf, &pal_buf, &vram_buf);
                        let dur = t_enc.elapsed().as_micros() as u32;
                        (FLAG_PPU_STATE, payload, dur)
                    } else {
                        let raw_frame = core.get_last_frame();
                        let t_enc = Instant::now();
                        let (f, p) = fallback_enc.encode(&raw_frame);
                        let dur = t_enc.elapsed().as_micros() as u32;
                        (f, p, dur)
                    }
                };

                let audio_payload = audio_enc.flush_frame_lz4().unwrap_or_default();
                let audio_len = audio_payload.len() as u16;
                let mut packet = Vec::with_capacity(4 + audio_payload.len() + video_payload.len());
                let header_byte0 = flag & 0x3F; // 0F runahead header
                packet.push(header_byte0);
                packet.push(0); // sequence 0 during paused step
                packet.extend_from_slice(&audio_len.to_le_bytes());
                packet.extend_from_slice(&audio_payload);
                packet.extend_from_slice(&video_payload);
                let _ = tx_producer.send(Arc::new(packet));
                next_frame_time = Instant::now();
                continue;
            }

            if was_paused {
                was_paused = false;
                let desired = runahead_producer.load(Ordering::Relaxed);
                core.set_runahead_frames(desired);
                println!("⚡ Resumed emulation: Run-Ahead set to {}F", desired);
            }

            next_frame_time += frame_budget;

            if force_keyframe_producer.swap(false, Ordering::Relaxed) {
                ppu_enc.force_keyframe();
                fallback_enc.force_keyframe();
            }

            let probe_req = probe_producer.swap(-1, Ordering::Relaxed);
            if probe_req >= 0 {
                let detected_lag = core.probe_current_lag(probe_req as i16);
                core.set_runahead_frames(detected_lag);
                runahead_producer.store(detected_lag, Ordering::Relaxed);
                crate::runahead_db::cache_measured_runahead(core.rom_game_code.clone(), detected_lag);
                let reply = vec![0xAA, 0x54, detected_lag];
                let _ = tx_producer.send(Arc::new(reply));
            }

            let matched_seq = seq_producer.load(Ordering::Relaxed);
            let current_mask = mask_producer.load(Ordering::Relaxed);
            let latched = latched_producer.swap(0, Ordering::Relaxed);
            let effective_mask = current_mask | latched;
            let desired_runahead = runahead_producer.load(Ordering::Relaxed);

            if core.runahead_frames != desired_runahead {
                core.set_runahead_frames(desired_runahead);
                println!("⚡ Live Run-Ahead switched to: {}F", desired_runahead);
            }

            core.set_input(effective_mask);
            let (_sim_us, audio_samples, ppu_ok) = core.step_ppu(
                &mut vram_buf,
                &mut pal_buf,
                &mut oam_buf,
                &mut io_buf,
            );

            let current_mode = stream_mode_producer.load(Ordering::Relaxed);
            profiler.set_setting(StreamEngineMode::from(current_mode));
            let use_ppu = profiler.should_use_ppu(&io_buf, ppu_ok);
            if use_ppu != prev_use_ppu {
                ppu_enc.force_keyframe();
                fallback_enc.force_keyframe();
                prev_use_ppu = use_ppu;
            }

            let (flag, video_payload, _enc_us) = if use_ppu {
                let t_enc = Instant::now();
                let payload = ppu_enc.encode(&oam_buf, &io_buf, &pal_buf, &vram_buf);
                let dur = t_enc.elapsed().as_micros() as u32;
                (FLAG_PPU_STATE, payload, dur)
            } else {
                let raw_frame = core.get_last_frame();
                let t_enc = Instant::now();
                let (f, p) = fallback_enc.encode(&raw_frame);
                let dur = t_enc.elapsed().as_micros() as u32;
                (f, p, dur)
            };

            if !audio_samples.is_empty() {
                audio_enc.push_samples(&audio_samples);
            }
            let audio_payload = audio_enc.flush_frame_lz4().unwrap_or_default();

            let audio_len = audio_payload.len() as u16;
            let mut packet = Vec::with_capacity(4 + audio_payload.len() + video_payload.len());
            let header_byte0 = (flag & 0x3F) | ((desired_runahead & 0x03) << 6);
            packet.push(header_byte0);
            packet.push((matched_seq & 0xFF) as u8);
            packet.extend_from_slice(&audio_len.to_le_bytes());
            packet.extend_from_slice(&audio_payload);
            packet.extend_from_slice(&video_payload);

            let _ = tx_producer.send(Arc::new(packet));

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

    let html_route = warp::path::end().map(|| {
        warp::reply::with_header(
            warp::reply::html(BROWSER_HTML),
            "Cache-Control",
            "no-store, no-cache, must-revalidate",
        )
    });
    let ping_route = warp::path("ping").map(|| warp::reply::html("pong"));
    let wasm_route = warp::path("gba_ppu.wasm").map(|| {
        warp::reply::with_header(
            warp::reply::with_header(BROWSER_WASM, "Content-Type", "application/wasm"),
            "Cache-Control",
            "public, max-age=3600",
        )
    });

    let tx_for_ws = tx_arc.clone();
    let seq_consumer = last_input_seq.clone();
    let mask_consumer = input_mask.clone();
    let latched_consumer = latched_mask.clone();
    let runahead_consumer = runahead_frames.clone();
    let keyframe_consumer = force_keyframe.clone();
    let probe_for_ws = probe_trigger.clone();
    let is_paused_for_ws = is_paused.clone();
    let step_for_ws = step_trigger.clone();
    let game_code_ws = rom_game_code.clone();
    let stream_mode_consumer = stream_mode.clone();
    let switch_rom_consumer_ws = switch_rom_req.clone();

    let ws_route = warp::path("ws")
        .and(warp::ws())
        .map(move |ws: Ws| {
            let mut client_rx = tx_for_ws.subscribe();
            let seq = seq_consumer.clone();
            let mask = mask_consumer.clone();
            let latched = latched_consumer.clone();
            let runahead = runahead_consumer.clone();
            let keyframe = keyframe_consumer.clone();
            let probe_signal = probe_for_ws.clone();
            let is_paused_signal = is_paused_for_ws.clone();
            let step_signal_ws = step_for_ws.clone();
            let game_code = game_code_ws.clone();
            let stream_mode_ws = stream_mode_consumer.clone();
            let switch_rom_ws = switch_rom_consumer_ws.clone();

            ws.on_upgrade(move |websocket| async move {
                    let (mut ws_sender, mut ws_receiver) = websocket.split();
                    println!("Browser client connected via WebSocket (TCP_NODELAY active)!");
                    is_paused_signal.store(false, Ordering::Relaxed);
                    keyframe.store(true, Ordering::Relaxed);

                    let keyframe_clone = keyframe.clone();
                    tokio::spawn(async move {
                        while let Some(Ok(msg)) = ws_receiver.next().await {
                            if msg.is_binary() {
                                let bytes = msg.as_bytes();
                                if bytes.len() == 2 && bytes[0] == 0xAA && bytes[1] == 0x4B {
                                    keyframe_clone.store(true, Ordering::Relaxed);
                                } else if bytes.len() == 3 && bytes[0] == 0xAA && bytes[1] == 0x52 {
                                    let target_f = bytes[2].min(2);
                                    runahead.store(target_f, Ordering::Relaxed);
                                    let cur_code = game_code.lock().unwrap().clone();
                                    crate::runahead_db::cache_measured_runahead(cur_code, target_f);
                                    println!("Client set Run-Ahead to: {}F (saved to config)", target_f);
                                } else if bytes.len() >= 3 && bytes[0] == 0xAA && bytes[1] == 0x57 {
                                    let mode = bytes[2];
                                    stream_mode_ws.store(mode, Ordering::Relaxed);
                                    keyframe_clone.store(true, Ordering::Relaxed);
                                    println!("Client switched stream mode to: {}", if mode == 1 { "PPU State (Optional)" } else { "Pixel XOR Delta (Default)" });
                                } else if bytes.len() >= 3 && bytes[0] == 0xAA && bytes[1] == 0x59 {
                                    let rom_id = String::from_utf8_lossy(&bytes[2..]).trim().to_string();
                                    switch_rom_ws.lock().unwrap().replace(rom_id.clone());
                                    println!("Client requested switch to ROM: {}", rom_id);
                                } else if bytes.len() >= 2 && bytes[0] == 0xAA && bytes[1] == 0x5B {
                                    is_paused_signal.store(true, Ordering::Relaxed);
                                    println!("Client exited to ROM selection menu");
                                } else if bytes.len() >= 3 && bytes[0] == 0xAA && bytes[1] == 0x50 {
                                    let p = bytes[2];
                                    if p == 2 {
                                        step_signal_ws.store(-2, Ordering::Relaxed);
                                        println!("Client requested calibration reset/rewind");
                                    } else {
                                        let is_p = p != 0;
                                        is_paused_signal.store(is_p, Ordering::Relaxed);
                                        println!("Client set emulation paused: {}", is_p);
                                    }
                                } else if bytes.len() >= 4 && bytes[0] == 0xAA && bytes[1] == 0x53 {
                                    let m = u16::from_le_bytes([bytes[2], bytes[3]]) as i16;
                                    step_signal_ws.store(m as i32, Ordering::Relaxed);
                                    println!("Client stepped 1 frame with mask: {}", m);
                                } else if bytes.len() >= 2 && bytes[0] == 0xAA && bytes[1] == 0x54 {
                                    let m = if bytes.len() >= 4 {
                                        u16::from_le_bytes(bytes[2..4].try_into().unwrap_or_default()) as i16
                                    } else {
                                        0
                                    };
                                    probe_signal.store(m as i32, Ordering::Relaxed);
                                    println!("Client requested live lag probe (mask: {})", m);
                                } else if bytes.len() >= 3 {
                                    let s = bytes[0] as u32;
                                    let m = u16::from_le_bytes(bytes[1..3].try_into().unwrap_or_default()) as i16;
                                    seq.store(s, Ordering::Relaxed);
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
                            Ok(packet) => {
                                if ws_sender
                                    .send(warp::ws::Message::binary((*packet).clone()))
                                    .await
                                    .is_err()
                                {
                                    break;
                                }
                            }
                            Err(tokio::sync::broadcast::error::RecvError::Lagged(skipped)) => {
                                eprintln!("⚠️ Client receiver lagged by {} frames", skipped);
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

    let roms_route = warp::path!("api" / "roms")
        .and(warp::get())
        .map(|| {
            let roms = scan_available_roms();
            let json = format!(
                "[{}]",
                roms.iter()
                    .map(|r| format!(
                        r#"{{"id":"{}","title":"{}","code":"{}","size_bytes":{}}}"#,
                        r.id, r.title, r.code, r.size_bytes
                    ))
                    .collect::<Vec<_>>()
                    .join(",")
            );
            warp::reply::with_header(
                warp::reply::with_header(json, "Content-Type", "application/json"),
                "Cache-Control",
                "no-cache",
            )
        });

    let switch_rom_for_http = switch_rom_req.clone();
    let switch_route = warp::path!("api" / "switch_rom")
        .and(warp::post())
        .and(warp::query::<std::collections::HashMap<String, String>>())
        .map(move |params: std::collections::HashMap<String, String>| {
            if let Some(rom_id) = params.get("rom") {
                switch_rom_for_http.lock().unwrap().replace(rom_id.clone());
                warp::reply::html(format!("{{\"status\":\"ok\",\"rom\":\"{}\"}}", rom_id))
            } else {
                warp::reply::html("{\"error\":\"Missing rom parameter\"}".to_string())
            }
        });

    let upload_route = warp::path!("api" / "upload" / String)
        .and(warp::post())
        .and(warp::body::bytes())
        .map(|filename: String, bytes: warp::hyper::body::Bytes| {
            let safe_name = PathBuf::from(filename).file_name().unwrap_or_default().to_string_lossy().to_string();
            let dest_dir = PathBuf::from("./roms");
            let _ = std::fs::create_dir_all(&dest_dir);
            let dest = dest_dir.join(&safe_name);
            match std::fs::write(&dest, &bytes) {
                Ok(_) => warp::reply::html(format!("{{\"status\":\"ok\",\"file\":\"{}\"}}", safe_name)),
                Err(e) => warp::reply::html(format!("{{\"error\":\"{}\"}}", e)),
            }
        });

    let routes = html_route
        .or(ping_route)
        .or(wasm_route)
        .or(roms_route)
        .or(switch_route)
        .or(upload_route)
        .or(ws_route);
    let listener = tokio::net::TcpListener::bind(&addr).await?;
    println!("⚡ Socket TCP_NODELAY active on all incoming connections!");
    let incoming = tokio_stream::wrappers::TcpListenerStream::new(listener).map(|res| {
        if let Ok(ref stream) = res {
            let _ = stream.set_nodelay(true);
        }
        res
    });
    warp::serve(routes).run_incoming(incoming).await;

    Ok(())
}
