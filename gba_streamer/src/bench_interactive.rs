use anyhow::{Context, Result};
use libloading::{Library, Symbol};
use std::ffi::{c_char, c_uint, c_void, CString};
use std::fs;
use std::ptr;
use std::sync::{atomic::{AtomicI16, Ordering}, Mutex};
use std::time::Instant;

const GBA_WIDTH: usize = 240;
const GBA_HEIGHT: usize = 160;
const RETRO_DEVICE_JOYPAD: c_uint = 1;

#[repr(C)]
struct RetroGameInfo {
    path: *const c_char,
    data: *const c_void,
    size: usize,
    meta: *const c_char,
}

static LAST_FRAME: Mutex<Vec<u32>> = Mutex::new(Vec::new());
static INPUT_STATE: AtomicI16 = AtomicI16::new(0);

unsafe extern "C" fn video_refresh_callback(
    data: *const c_void,
    width: c_uint,
    height: c_uint,
    pitch: usize,
) {
    if data.is_null() {
        return;
    }
    let mut guard = LAST_FRAME.lock().unwrap();
    if guard.len() != (width * height) as usize {
        guard.resize((width * height) as usize, 0);
    }
    let src = data as *const u16;
    let pixels_per_pitch = pitch / 2;
    for y in 0..height as usize {
        for x in 0..width as usize {
            let pixel16 = *src.add(y * pixels_per_pitch + x);
            let r = ((pixel16 & 0x7C00) >> 10) as u32;
            let g = ((pixel16 & 0x03E0) >> 5) as u32;
            let b = (pixel16 & 0x001F) as u32;
            let r8 = (r * 255) / 31;
            let g8 = (g * 255) / 31;
            let b8 = (b * 255) / 31;
            guard[y * (width as usize) + x] = (r8 << 16) | (g8 << 8) | b8;
        }
    }
}

unsafe extern "C" fn audio_sample_callback(_left: i16, _right: i16) {}
unsafe extern "C" fn audio_sample_batch_callback(_data: *const i16, frames: usize) -> usize { frames }
unsafe extern "C" fn input_poll_callback() {}
unsafe extern "C" fn input_state_callback(_port: c_uint, device: c_uint, _index: c_uint, id: c_uint) -> i16 {
    if device != RETRO_DEVICE_JOYPAD { return 0; }
    let mask = INPUT_STATE.load(Ordering::Relaxed);
    if (mask & (1 << id)) != 0 { 1 } else { 0 }
}

unsafe extern "C" fn environment_callback(cmd: c_uint, data: *mut c_void) -> bool {
    const RETRO_ENVIRONMENT_SET_PIXEL_FORMAT: c_uint = 10;
    const RETRO_PIXEL_FORMAT_RGB565: c_uint = 2;
    const RETRO_PIXEL_FORMAT_0RGB1555: c_uint = 0;
    if cmd == RETRO_ENVIRONMENT_SET_PIXEL_FORMAT && !data.is_null() {
        let fmt = *(data as *const c_uint);
        return fmt == RETRO_PIXEL_FORMAT_0RGB1555 || fmt == RETRO_PIXEL_FORMAT_RGB565;
    }
    false
}

fn main() -> Result<()> {
    let core_path = "/usr/lib64/libretro/mgba_libretro.so";
    let rom_path = "/tmp/test_rom.gba";

    let lib: &'static Library = Box::leak(Box::new(
        unsafe { Library::new(core_path) }
            .with_context(|| format!("Failed to load core library: {}", core_path))?,
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
        if !retro_load_game(&info) {
            anyhow::bail!("retro_load_game failed!");
        }

        // Warm up 50 frames
        for _ in 0..50 {
            retro_run();
        }

        println!("=== Starting 500-Frame Interactive Benchmark (GBA Streamer) ===");
        let total_frames = 500;
        let mut sim_times = Vec::with_capacity(total_frames);
        let mut encode_times = Vec::with_capacity(total_frames);
        let mut decode_times = Vec::with_capacity(total_frames);
        let mut frame_sizes = Vec::with_capacity(total_frames);
        let mut total_proc_times = Vec::with_capacity(total_frames);

        for frame_idx in 0..total_frames {
            let t_start = Instant::now();

            // 1. Interactive Input Injection (simulate active d-pad + buttons)
            let input_mask = if frame_idx % 30 < 15 { 1 << 4 } else { 1 << 7 } | if frame_idx % 10 == 0 { 1 << 0 } else { 0 };
            INPUT_STATE.store(input_mask, Ordering::Relaxed);

            // 2. Core Simulation
            let t_sim_start = Instant::now();
            retro_run();
            let sim_dur = t_sim_start.elapsed().as_micros() as f64 / 1000.0;
            sim_times.push(sim_dur);

            // 3. Frame Encoding (LZ4)
            let raw_frame = LAST_FRAME.lock().unwrap().clone();
            let t_enc_start = Instant::now();
            let byte_slice = std::slice::from_raw_parts(
                raw_frame.as_ptr() as *const u8,
                raw_frame.len() * 4,
            );
            let compressed = lz4_flex::compress_prepend_size(byte_slice);
            let enc_dur = t_enc_start.elapsed().as_micros() as f64 / 1000.0;
            encode_times.push(enc_dur);
            frame_sizes.push(compressed.len());

            // 4. Client Decoding & Buffer Parsing (LZ4)
            let t_dec_start = Instant::now();
            let decomp = lz4_flex::decompress_size_prepended(&compressed).unwrap();
            let mut dummy_acc = 0u64;
            for i in 0..decomp.len() / 4 {
                dummy_acc += decomp[i * 4] as u64;
            }
            let dec_dur = t_dec_start.elapsed().as_micros() as f64 / 1000.0;
            decode_times.push(dec_dur);

            let total_dur = t_start.elapsed().as_micros() as f64 / 1000.0;
            total_proc_times.push(total_dur);
        }

        let mean = |v: &[f64]| v.iter().sum::<f64>() / v.len() as f64;
        let mut sorted_tot = total_proc_times.clone();
        sorted_tot.sort_by(|a, b| a.partial_cmp(b).unwrap());
        let p95 = sorted_tot[(sorted_tot.len() as f64 * 0.95) as usize];
        let p99 = sorted_tot[(sorted_tot.len() as f64 * 0.99) as usize];
        let avg_bytes = frame_sizes.iter().sum::<usize>() as f64 / frame_sizes.len() as f64;

        println!("RESULTS_GBA_STREAMER:");
        println!("  Frames Tested: {}", total_frames);
        println!("  Mean Sim Time: {:.3} ms", mean(&sim_times));
        println!("  Mean Encode Time (LZ4): {:.3} ms", mean(&encode_times));
        println!("  Mean Decode Time (LZ4): {:.3} ms", mean(&decode_times));
        println!("  Mean Host+Client Processing: {:.3} ms", mean(&total_proc_times));
        println!("  P95 Processing: {:.3} ms", p95);
        println!("  P99 Processing: {:.3} ms", p99);
        println!("  Average Compressed Frame Size: {:.1} KB (Original: 153.6 KB)", avg_bytes / 1024.0);
        println!("  Compression Ratio: {:.1}x", (153600.0) / avg_bytes);
    }
    Ok(())
}
