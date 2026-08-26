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

// 8x8 Tile Delta Compression for Native 16-bit RGB555 (600 tiles)
const TILE_SIZE: usize = 8;
const TILES_X: usize = GBA_WIDTH / TILE_SIZE; // 30
const TILES_Y: usize = GBA_HEIGHT / TILE_SIZE; // 20
const TOTAL_TILES: usize = TILES_X * TILES_Y; // 600

fn compress_delta_16(curr: &[u16], prev: &[u16], is_keyframe: bool) -> (u8, Vec<u8>) {
    if is_keyframe || prev.is_empty() {
        let byte_slice = unsafe { std::slice::from_raw_parts(curr.as_ptr() as *const u8, curr.len() * 2) };
        return (1, lz4_flex::compress_prepend_size(byte_slice));
    }

    let mut bitmask = vec![0u8; (TOTAL_TILES + 7) / 8]; // 75 bytes
    let mut changed_pixels = Vec::with_capacity(curr.len() * 2);
    let mut changed_count = 0;

    for ty in 0..TILES_Y {
        for tx in 0..TILES_X {
            let tile_idx = ty * TILES_X + tx;
            let mut tile_changed = false;

            for y in 0..TILE_SIZE {
                let py = ty * TILE_SIZE + y;
                let start_idx = py * GBA_WIDTH + tx * TILE_SIZE;
                for x in 0..TILE_SIZE {
                    if curr[start_idx + x] != prev[start_idx + x] {
                        tile_changed = true;
                        break;
                    }
                }
                if tile_changed { break; }
            }

            if tile_changed {
                bitmask[tile_idx / 8] |= 1 << (tile_idx % 8);
                changed_count += 1;
                for y in 0..TILE_SIZE {
                    let py = ty * TILE_SIZE + y;
                    let start_idx = py * GBA_WIDTH + tx * TILE_SIZE;
                    for x in 0..TILE_SIZE {
                        changed_pixels.extend_from_slice(&curr[start_idx + x].to_le_bytes());
                    }
                }
            }
        }
    }

    if changed_count == 0 {
        return (2, Vec::new()); // No change
    }

    let compressed = lz4_flex::compress_prepend_size(&changed_pixels);
    let mut payload = Vec::with_capacity(bitmask.len() + compressed.len());
    payload.extend_from_slice(&bitmask);
    payload.extend_from_slice(&compressed);
    (0, payload)
}

fn decompress_delta_16(flag: u8, payload: &[u8], frame_buffer: &mut [u16]) -> Result<()> {
    if flag == 1 {
        // Keyframe
        let decomp = lz4_flex::decompress_size_prepended(payload)?;
        let src16 = unsafe { std::slice::from_raw_parts(decomp.as_ptr() as *const u16, decomp.len() / 2) };
        frame_buffer.copy_from_slice(src16);
    } else if flag == 0 {
        // Delta
        let bitmask_len = (TOTAL_TILES + 7) / 8;
        let bitmask = &payload[..bitmask_len];
        let decomp = lz4_flex::decompress_size_prepended(&payload[bitmask_len..])?;
        let src16 = unsafe { std::slice::from_raw_parts(decomp.as_ptr() as *const u16, decomp.len() / 2) };

        let mut read_idx = 0;
        for ty in 0..TILES_Y {
            for tx in 0..TILES_X {
                let tile_idx = ty * TILES_X + tx;
                if (bitmask[tile_idx / 8] & (1 << (tile_idx % 8))) != 0 {
                    for y in 0..TILE_SIZE {
                        let py = ty * TILE_SIZE + y;
                        let start_idx = py * GBA_WIDTH + tx * TILE_SIZE;
                        for x in 0..TILE_SIZE {
                            frame_buffer[start_idx + x] = src16[read_idx];
                            read_idx += 1;
                        }
                    }
                }
            }
        }
    }
    Ok(())
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

        println!("=== RUNNING 2,000-FRAME NATIVE 16-BIT TILE DELTA STREAMING BENCHMARK ===");
        let total_frames = 2000;
        let warmup_frames = 120;

        let mut delta_sizes = Vec::with_capacity(total_frames);
        let mut sim_times = Vec::with_capacity(total_frames);
        let mut delta_enc_times = Vec::with_capacity(total_frames);
        let mut delta_dec_times = Vec::with_capacity(total_frames);

        let mut prev_frame = Vec::new();
        let mut client_frame = vec![0u16; GBA_WIDTH * GBA_HEIGHT];

        for i in 0..total_frames {
            // Warmup START/A then active gameplay
            let mask = if i < warmup_frames {
                if i % 20 < 10 { (1 << 3) | (1 << 0) } else { 0 }
            } else {
                let f = i - warmup_frames;
                let mut m = 0;
                if f % 80 < 60 { m |= 1 << 4; } else { m |= 1 << 5; }
                if f % 25 < 8 { m |= 1 << 0; }
                if f % 45 < 12 { m |= 1 << 1; }
                m
            };
            INPUT_STATE.store(mask, Ordering::Relaxed);

            let t_sim = Instant::now();
            retro_run();
            let sim_dur = t_sim.elapsed().as_micros() as f64 / 1000.0;

            let raw_frame = LAST_FRAME_16.lock().unwrap().clone();
            if raw_frame.is_empty() { continue; }

            let is_keyframe = i % 120 == 0; // 1 Keyframe every 2 seconds
            let t_enc = Instant::now();
            let (flag, payload) = compress_delta_16(&raw_frame, &prev_frame, is_keyframe);
            let enc_dur = t_enc.elapsed().as_micros() as f64 / 1000.0;
            prev_frame = raw_frame.clone();

            // Client side decode
            let t_dec = Instant::now();
            decompress_delta_16(flag, &payload, &mut client_frame)?;
            let dec_dur = t_dec.elapsed().as_micros() as f64 / 1000.0;

            // Verify bit-exact fidelity
            assert_eq!(&client_frame[..], &raw_frame[..]);

            if i >= warmup_frames {
                delta_sizes.push(payload.len() + 1);
                sim_times.push(sim_dur);
                delta_enc_times.push(enc_dur);
                delta_dec_times.push(dec_dur);
            }
        }

        let mean = |v: &[f64]| v.iter().sum::<f64>() / v.len() as f64;
        let avg_bytes = delta_sizes.iter().sum::<usize>() as f64 / delta_sizes.len() as f64;
        let avg_kb = avg_bytes / 1024.0;
        let peak_kb = delta_sizes.iter().cloned().max().unwrap() as f64 / 1024.0;
        let mbps_60 = (avg_bytes * 8.0 * 60.0) / 1_000_000.0;
        let under_1400 = delta_sizes.iter().filter(|&&sz| sz <= 1400).count();

        println!("RESULTS_NATIVE_16BIT_TILE_DELTA:");
        println!("  Frames Evaluated: {}", delta_sizes.len());
        println!("  Mean Sim Time:           {:.3} ms", mean(&sim_times));
        println!("  Mean Delta Encode Time:  {:.3} ms", mean(&delta_enc_times));
        println!("  Mean Delta Decode Time:  {:.3} ms", mean(&delta_dec_times));
        println!("  Total Pipeline Compute:  {:.3} ms", mean(&sim_times) + mean(&delta_enc_times) + mean(&delta_dec_times));
        println!("  Average Frame Payload:   {:.2} KB ({:.2} Mbps @ 60 FPS)", avg_kb, mbps_60);
        println!("  Peak Frame Payload:      {:.2} KB", peak_kb);
        println!("  Compression Ratio:       {:.1}x (76.8 KB raw -> {:.2} KB)", 76800.0 / avg_bytes, avg_kb);
        println!("  Frames in Single UDP Pkt (<1400B): {} / {} ({:.1}%)", under_1400, delta_sizes.len(), (under_1400 as f64 / delta_sizes.len() as f64) * 100.0);
    }
    Ok(())
}
