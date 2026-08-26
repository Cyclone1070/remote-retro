use anyhow::{Context, Result};
use libloading::{Library, Symbol};
use std::collections::HashMap;
use std::ffi::{c_char, c_uint, c_void, CString};
use std::fs;
use std::ptr;
use std::sync::{atomic::{AtomicI16, Ordering}, Mutex};
use std::time::Instant;

const GBA_WIDTH: usize = 240;
const GBA_HEIGHT: usize = 160;
const TOTAL_PIXELS: usize = GBA_WIDTH * GBA_HEIGHT;
const RETRO_DEVICE_JOYPAD: c_uint = 1;

#[repr(C)]
struct RetroGameInfo {
    path: *const c_char,
    data: *const c_void,
    size: usize,
    meta: *const c_char,
}

static LAST_FRAME_16: Mutex<Vec<u16>> = Mutex::new(Vec::new());
static INPUT_STATE: AtomicI16 = AtomicI16::new(0);

unsafe extern "C" fn video_refresh_callback(
    data: *const c_void,
    width: c_uint,
    height: c_uint,
    pitch: usize,
) {
    if data.is_null() { return; }
    let mut guard = LAST_FRAME_16.lock().unwrap();
    if guard.len() != (width * height) as usize {
        guard.resize((width * height) as usize, 0);
    }
    let src = data as *const u16;
    let pixels_per_pitch = pitch / 2;
    for y in 0..height as usize {
        for x in 0..width as usize {
            guard[y * (width as usize) + x] = *src.add(y * pixels_per_pitch + x);
        }
    }
}
unsafe extern "C" fn audio_sample_callback(_: i16, _: i16) {}
unsafe extern "C" fn audio_sample_batch_callback(_: *const i16, f: usize) -> usize { f }
unsafe extern "C" fn input_poll_callback() {}
unsafe extern "C" fn input_state_callback(_: c_uint, dev: c_uint, _: c_uint, id: c_uint) -> i16 {
    if dev != RETRO_DEVICE_JOYPAD { return 0; }
    let mask = INPUT_STATE.load(Ordering::Relaxed);
    if (mask & (1 << id)) != 0 { 1 } else { 0 }
}
unsafe extern "C" fn environment_callback(cmd: c_uint, data: *mut c_void) -> bool {
    if cmd == 10 && !data.is_null() { return true; }
    false
}

fn main() -> Result<()> {
    let core_path = "/usr/lib64/libretro/mgba_libretro.so";
    let rom_path = "/tmp/test_rom.gba";

    let lib: &'static Library = Box::leak(Box::new(
        unsafe { Library::new(core_path) }
            .context(format!("Failed to load core: {}", core_path))?,
    ));

    unsafe {
        let retro_init: Symbol<unsafe extern "C" fn()> = lib.get(b"retro_init")?;
        let retro_set_environment: Symbol<unsafe extern "C" fn(unsafe extern "C" fn(c_uint, *mut c_void) -> bool)> = lib.get(b"retro_set_environment")?;
        let retro_set_video_refresh: Symbol<unsafe extern "C" fn(unsafe extern "C" fn(*const c_void, c_uint, c_uint, usize))> = lib.get(b"retro_set_video_refresh")?;
        let retro_set_audio_sample: Symbol<unsafe extern "C" fn(unsafe extern "C" fn(i16, i16))> = lib.get(b"retro_set_audio_sample")?;
        let retro_set_audio_sample_batch: Symbol<unsafe extern "C" fn(unsafe extern "C" fn(*const i16, usize) -> usize)> = lib.get(b"retro_set_audio_sample_batch")?;
        let retro_set_input_poll: Symbol<unsafe extern "C" fn(unsafe extern "C" fn())> = lib.get(b"retro_set_input_poll")?;
        let retro_set_input_state: Symbol<unsafe extern "C" fn(unsafe extern "C" fn(c_uint, c_uint, c_uint, c_uint) -> i16)> = lib.get(b"retro_set_input_state")?;
        let retro_load_game: Symbol<unsafe extern "C" fn(*const RetroGameInfo) -> bool> = lib.get(b"retro_load_game")?;
        let retro_run: Symbol<unsafe extern "C" fn()> = lib.get(b"retro_run")?;

        retro_set_environment(environment_callback);
        retro_set_video_refresh(video_refresh_callback);
        retro_set_audio_sample(audio_sample_callback);
        retro_set_audio_sample_batch(audio_sample_batch_callback);
        retro_set_input_poll(input_poll_callback);
        retro_set_input_state(input_state_callback);
        retro_init();

        let rom_data = fs::read(rom_path)?;
        let c_path = CString::new(rom_path)?;
        let info = RetroGameInfo {
            path: c_path.as_ptr(),
            data: rom_data.as_ptr() as *const c_void,
            size: rom_data.len(),
            meta: ptr::null(),
        };
        retro_load_game(&info);

        println!("=== BENCHMARKING REAL GBA GAMEPLAY COMPRESSION (1,000 FRAMES) ===");

        let mut raw_sizes = Vec::new();
        let mut raw_enc_times = Vec::new();

        let mut planar_sizes = Vec::new();
        let mut planar_enc_times = Vec::new();

        let mut palette_sizes = Vec::new();
        let mut palette_enc_times = Vec::new();
        let mut unique_color_counts = Vec::new();

        // Run 1000 frames of active Level 1 gameplay
        for i in 0..1000 {
            let mask = if i < 120 {
                if i % 20 < 10 { (1 << 3) | (1 << 0) } else { 0 }
            } else {
                let f = i - 120;
                let mut m = 0;
                if f % 80 < 60 { m |= 1 << 4; } else { m |= 1 << 5; }
                if f % 25 < 8 { m |= 1 << 0; }
                m
            };
            INPUT_STATE.store(mask, Ordering::Relaxed);
            retro_run();

            if i < 120 { continue; } // Skip warmup

            let raw16 = LAST_FRAME_16.lock().unwrap().clone();
            if raw16.len() != TOTAL_PIXELS { continue; }

            // 1. Raw 16-bit + LZ4
            let t0 = Instant::now();
            let raw_bytes: &[u8] = std::slice::from_raw_parts(raw16.as_ptr() as *const u8, TOTAL_PIXELS * 2);
            let c_raw = lz4_flex::compress_prepend_size(raw_bytes);
            let t_raw = t0.elapsed().as_micros() as f64 / 1000.0;
            raw_sizes.push(c_raw.len());
            raw_enc_times.push(t_raw);

            // 2. Planar Byte Separation + LZ4
            let t0 = Instant::now();
            let mut planar = vec![0u8; TOTAL_PIXELS * 2];
            for p in 0..TOTAL_PIXELS {
                planar[p] = (raw16[p] & 0xFF) as u8;
                planar[TOTAL_PIXELS + p] = ((raw16[p] >> 8) & 0xFF) as u8;
            }
            let c_planar = lz4_flex::compress_prepend_size(&planar);
            let t_planar = t0.elapsed().as_micros() as f64 / 1000.0;
            planar_sizes.push(c_planar.len());
            planar_enc_times.push(t_planar);

            // 3. Dynamic Frame Palette + LZ4
            let t0 = Instant::now();
            let mut color_map: HashMap<u16, u8> = HashMap::with_capacity(256);
            let mut pal_table: Vec<u16> = Vec::with_capacity(256);
            let mut indexed_pixels = vec![0u8; TOTAL_PIXELS];
            let mut exceeds_256 = false;

            for p in 0..TOTAL_PIXELS {
                let c = raw16[p];
                if let Some(&idx) = color_map.get(&c) {
                    indexed_pixels[p] = idx;
                } else if pal_table.len() < 256 {
                    let idx = pal_table.len() as u8;
                    color_map.insert(c, idx);
                    pal_table.push(c);
                    indexed_pixels[p] = idx;
                } else {
                    exceeds_256 = true;
                    break;
                }
            }

            if !exceeds_256 {
                let mut pal_payload = Vec::with_capacity(2 + pal_table.len() * 2 + TOTAL_PIXELS);
                pal_payload.extend_from_slice(&(pal_table.len() as u16).to_le_bytes());
                for c in &pal_table { pal_payload.extend_from_slice(&c.to_le_bytes()); }
                pal_payload.extend_from_slice(&indexed_pixels);
                let c_pal = lz4_flex::compress_prepend_size(&pal_payload);
                let t_pal = t0.elapsed().as_micros() as f64 / 1000.0;
                palette_sizes.push(c_pal.len());
                palette_enc_times.push(t_pal);
                unique_color_counts.push(pal_table.len());
            }
        }

        let mean = |v: &[f64]| v.iter().sum::<f64>() / v.len() as f64;
        let avg_raw = raw_sizes.iter().sum::<usize>() as f64 / raw_sizes.len() as f64;
        let avg_planar = planar_sizes.iter().sum::<usize>() as f64 / planar_sizes.len() as f64;
        let avg_pal = palette_sizes.iter().sum::<usize>() as f64 / palette_sizes.len() as f64;
        let avg_unique_colors = unique_color_counts.iter().sum::<usize>() as f64 / unique_color_counts.len() as f64;

        println!("RESULTS ON REAL GBA GAMEPLAY (Sushi The Cat, 880 Analyzed Active Frames):");
        println!("  Average Unique Colors/Frame: {:.1} (Max allowed: 256)", avg_unique_colors);
        println!("----------------------------------------------------------------------------------");
        println!("{:<32} | {:<10} | {:<12} | {:<10}", "Format", "Avg Size", "Bitrate@60", "Encode Time");
        println!("----------------------------------------------------------------------------------");
        println!("{:<32} | {:<7.2} KB | {:<9.2} Mbps | {:.3} ms",
            "1. Raw 16-Bit RGB555 + LZ4", avg_raw / 1024.0, (avg_raw * 8.0 * 60.0) / 1_000_000.0, mean(&raw_enc_times));
        println!("{:<32} | {:<7.2} KB | {:<9.2} Mbps | {:.3} ms",
            "2. Planar Byte-Split + LZ4", avg_planar / 1024.0, (avg_planar * 8.0 * 60.0) / 1_000_000.0, mean(&planar_enc_times));
        println!("{:<32} | {:<7.2} KB | {:<9.2} Mbps | {:.3} ms",
            "3. 8-Bit Dynamic Palette + LZ4", avg_pal / 1024.0, (avg_pal * 8.0 * 60.0) / 1_000_000.0, mean(&palette_enc_times));
        println!("==================================================================================");
    }
    Ok(())
}
