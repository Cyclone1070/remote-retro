use anyhow::{Context, Result};
use libloading::{Library, Symbol};
use std::collections::HashMap;
use std::ffi::{c_char, c_uint, c_void, CString};
use std::fs;
use std::ptr;
use std::sync::{atomic::{AtomicI16, Ordering}, Mutex};

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
    let rom_path = "/home/cyc/cloud-game-repo/assets/games/gba/anguna.gba";

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

        println!("===================================================================");
        println!(" ⚡ BENCHMARKING ANGUNA (GBA) FOR 4-BIT vs 8-BIT PALETTE GAIN");
        println!("===================================================================");

        let mut size_8bit = Vec::new();
        let mut size_4bit = Vec::new();
        let mut color_counts = Vec::new();

        let mut color_map: HashMap<u16, u8> = HashMap::with_capacity(256);
        let mut pal_table: Vec<u16> = Vec::with_capacity(256);
        let mut indexed_pixels = vec![0u8; TOTAL_PIXELS];
        let mut nibble_packed = vec![0u8; TOTAL_PIXELS / 2];
        let mut pal_payload = Vec::with_capacity(2 + 512 + TOTAL_PIXELS);

        let mut four_bit_eligible_frames = 0;

        for f in 0..1500 {
            // Send Start / A buttons to pass title screens into dungeon gameplay
            let mask = if f % 20 < 10 { (1 << 3) | (1 << 0) } else { 1 << 4 };
            INPUT_STATE.store(mask, Ordering::Relaxed);
            retro_run();

            if f < 60 { continue; }

            let raw16 = LAST_FRAME_16.lock().unwrap().clone();
            if raw16.len() != TOTAL_PIXELS { continue; }

            color_map.clear();
            pal_table.clear();

            for p in 0..TOTAL_PIXELS {
                let c = raw16[p];
                if let Some(&idx) = color_map.get(&c) {
                    indexed_pixels[p] = idx;
                } else if pal_table.len() < 256 {
                    let idx = pal_table.len() as u8;
                    color_map.insert(c, idx);
                    pal_table.push(c);
                    indexed_pixels[p] = idx;
                }
            }

            color_counts.push(pal_table.len());

            // 1. Standard 8-Bit Dynamic Palette
            pal_payload.clear();
            pal_payload.extend_from_slice(&(pal_table.len() as u16).to_le_bytes());
            for c in &pal_table { pal_payload.extend_from_slice(&c.to_le_bytes()); }
            pal_payload.extend_from_slice(&indexed_pixels);
            let c_8bit = lz4_flex::compress_prepend_size(&pal_payload);
            size_8bit.push(c_8bit.len());

            // 2. 4-Bit Nibble Packed (If <= 16 colors, else 8-bit)
            if pal_table.len() <= 16 {
                four_bit_eligible_frames += 1;
                for i in 0..TOTAL_PIXELS / 2 {
                    nibble_packed[i] = (indexed_pixels[i * 2] & 0x0F) | ((indexed_pixels[i * 2 + 1] & 0x0F) << 4);
                }
                pal_payload.clear();
                pal_payload.extend_from_slice(&(pal_table.len() as u16).to_le_bytes());
                for c in &pal_table { pal_payload.extend_from_slice(&c.to_le_bytes()); }
                pal_payload.extend_from_slice(&nibble_packed);
                let c_4bit = lz4_flex::compress_prepend_size(&pal_payload);
                size_4bit.push(c_4bit.len());
            } else {
                size_4bit.push(c_8bit.len());
            }
        }

        let mean_col = color_counts.iter().sum::<usize>() as f64 / color_counts.len() as f64;
        let avg_8 = size_8bit.iter().sum::<usize>() as f64 / size_8bit.len() as f64;
        let avg_4 = size_4bit.iter().sum::<usize>() as f64 / size_4bit.len() as f64;

        println!("RESULTS ON ANGUNA (1,440 FRAMES):");
        println!("  Average Unique Colors/Frame: {:.1}", mean_col);
        println!("  Frames with <= 16 Colors:    {} / 1440 ({:.1}%)", four_bit_eligible_frames, (four_bit_eligible_frames as f64 / 1440.0) * 100.0);
        println!("-------------------------------------------------------------------");
        println!("{:<32} | {:<10} | {:<12}", "Codec", "Avg Size", "Bitrate@60");
        println!("-------------------------------------------------------------------");
        println!("{:<32} | {:<7.2} KB | {:<9.2} Mbps", "Standard 8-Bit Palette", avg_8 / 1024.0, (avg_8 * 8.0 * 60.0) / 1_000_000.0);
        println!("{:<32} | {:<7.2} KB | {:<9.2} Mbps", "Adaptive 4-Bit/8-Bit Palette", avg_4 / 1024.0, (avg_4 * 8.0 * 60.0) / 1_000_000.0);
        println!("-------------------------------------------------------------------");
        println!("{:<32} | {:<7.2} KB | {:<9.2} Mbps ({:.1}% size reduction)",
            "DELTA GAIN:", (avg_4 - avg_8) / 1024.0, (avg_4 - avg_8) * 8.0 * 60.0 / 1_000_000.0,
            (1.0 - avg_4 / avg_8) * 100.0);
        println!("===================================================================\n");
    }
    Ok(())
}
