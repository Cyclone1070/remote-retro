use anyhow::Result;
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
    if data.is_null() { return; }
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
            guard[y * (width as usize) + x] = ((r * 255 / 31) << 16) | ((g * 255 / 31) << 8) | (b * 255 / 31);
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

// Tile-based Delta Compressor (8x8 tiles: 30x20 = 600 tiles)
const TILE_SIZE: usize = 8;
const TILES_X: usize = GBA_WIDTH / TILE_SIZE; // 30
const TILES_Y: usize = GBA_HEIGHT / TILE_SIZE; // 20
const TOTAL_TILES: usize = TILES_X * TILES_Y; // 600

fn compress_delta(curr: &[u32], prev: &[u32], is_keyframe: bool) -> Vec<u8> {
    if is_keyframe || prev.is_empty() {
        let byte_slice = unsafe { std::slice::from_raw_parts(curr.as_ptr() as *const u8, curr.len() * 4) };
        let mut out = vec![1u8]; // Keyframe flag
        out.extend_from_slice(&lz4_flex::compress_prepend_size(byte_slice));
        return out;
    }

    // Delta frame: bitmask of changed tiles (600 bits = 75 bytes)
    let mut bitmask = vec![0u8; (TOTAL_TILES + 7) / 8];
    let mut changed_pixels = Vec::with_capacity(curr.len() * 4);
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
                        let p = curr[start_idx + x];
                        changed_pixels.extend_from_slice(&p.to_le_bytes());
                    }
                }
            }
        }
    }

    if changed_count == 0 {
        return vec![2u8]; // Empty delta (0 bytes changed)
    }

    let compressed_pixels = lz4_flex::compress_prepend_size(&changed_pixels);
    let mut out = Vec::with_capacity(1 + bitmask.len() + compressed_pixels.len());
    out.push(0u8); // Delta frame flag
    out.extend_from_slice(&bitmask);
    out.extend_from_slice(&compressed_pixels);
    out
}

fn decompress_delta(delta: &[u8], prev: &mut [u32]) -> Result<()> {
    if delta.is_empty() { return Ok(()); }
    let flag = delta[0];
    if flag == 1 {
        // Keyframe
        let decomp = lz4_flex::decompress_size_prepended(&delta[1..])?;
        let src32 = unsafe { std::slice::from_raw_parts(decomp.as_ptr() as *const u32, decomp.len() / 4) };
        prev.copy_from_slice(src32);
    } else if flag == 0 {
        // Delta
        let bitmask_len = (TOTAL_TILES + 7) / 8;
        let bitmask = &delta[1..1 + bitmask_len];
        let decomp = lz4_flex::decompress_size_prepended(&delta[1 + bitmask_len..])?;
        let src32 = unsafe { std::slice::from_raw_parts(decomp.as_ptr() as *const u32, decomp.len() / 4) };

        let mut read_idx = 0;
        for ty in 0..TILES_Y {
            for tx in 0..TILES_X {
                let tile_idx = ty * TILES_X + tx;
                if (bitmask[tile_idx / 8] & (1 << (tile_idx % 8))) != 0 {
                    for y in 0..TILE_SIZE {
                        let py = ty * TILE_SIZE + y;
                        let start_idx = py * GBA_WIDTH + tx * TILE_SIZE;
                        for x in 0..TILE_SIZE {
                            prev[start_idx + x] = src32[read_idx];
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

    let (core_path, rom_path) = if std::path::Path::new(core_path).exists() {
        (core_path, rom_path)
    } else {
        println!("Note: Running mock delta test on Mac");
        return Ok(());
    };

    Ok(())
}
