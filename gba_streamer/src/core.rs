use anyhow::{Context, Result};
use libloading::{Library, Symbol};
use std::ffi::{c_char, c_uint, c_void, CString};
use std::fs;
use std::ptr;
use std::sync::{atomic::{AtomicI16, Ordering}, Mutex};
use std::time::Instant;

const RETRO_DEVICE_JOYPAD: c_uint = 1;
const RETRO_DEVICE_ID_JOYPAD_B: c_uint = 0;
const RETRO_DEVICE_ID_JOYPAD_SELECT: c_uint = 2;
const RETRO_DEVICE_ID_JOYPAD_START: c_uint = 3;
const RETRO_DEVICE_ID_JOYPAD_UP: c_uint = 4;
const RETRO_DEVICE_ID_JOYPAD_DOWN: c_uint = 5;
const RETRO_DEVICE_ID_JOYPAD_LEFT: c_uint = 6;
const RETRO_DEVICE_ID_JOYPAD_RIGHT: c_uint = 7;
const RETRO_DEVICE_ID_JOYPAD_A: c_uint = 8;
const RETRO_DEVICE_ID_JOYPAD_L: c_uint = 10;
const RETRO_DEVICE_ID_JOYPAD_R: c_uint = 11;

#[repr(C)]
struct RetroGameInfo {
    path: *const c_char,
    data: *const c_void,
    size: usize,
    meta: *const c_char,
}

static LAST_FRAME_16: Mutex<Vec<u16>> = Mutex::new(Vec::new());
static AUDIO_BUFFER: Mutex<Vec<i16>> = Mutex::new(Vec::new());
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

unsafe extern "C" fn audio_sample_callback(left: i16, right: i16) {
    let mut guard = AUDIO_BUFFER.lock().unwrap();
    guard.push(left);
    guard.push(right);
}

unsafe extern "C" fn audio_sample_batch_callback(data: *const i16, frames: usize) -> usize {
    if !data.is_null() && frames > 0 {
        let mut guard = AUDIO_BUFFER.lock().unwrap();
        let samples = unsafe { std::slice::from_raw_parts(data, frames * 2) };
        guard.extend_from_slice(samples);
    }
    frames
}

unsafe extern "C" fn input_poll_callback() {}
unsafe extern "C" fn input_state_callback(
    _port: c_uint,
    device: c_uint,
    _index: c_uint,
    id: c_uint,
) -> i16 {
    if device != RETRO_DEVICE_JOYPAD {
        return 0;
    }
    let mask = INPUT_STATE.load(Ordering::Relaxed);
    let is_pressed = match id {
        RETRO_DEVICE_ID_JOYPAD_A => (mask & (1 << 0)) != 0,
        RETRO_DEVICE_ID_JOYPAD_B => (mask & (1 << 1)) != 0,
        RETRO_DEVICE_ID_JOYPAD_SELECT => (mask & (1 << 2)) != 0,
        RETRO_DEVICE_ID_JOYPAD_START => (mask & (1 << 3)) != 0,
        RETRO_DEVICE_ID_JOYPAD_RIGHT => (mask & (1 << 4)) != 0,
        RETRO_DEVICE_ID_JOYPAD_LEFT => (mask & (1 << 5)) != 0,
        RETRO_DEVICE_ID_JOYPAD_UP => (mask & (1 << 6)) != 0,
        RETRO_DEVICE_ID_JOYPAD_DOWN => (mask & (1 << 7)) != 0,
        RETRO_DEVICE_ID_JOYPAD_R => (mask & (1 << 8)) != 0,
        RETRO_DEVICE_ID_JOYPAD_L => (mask & (1 << 9)) != 0,
        _ => false,
    };
    if is_pressed { 1 } else { 0 }
}

use std::sync::atomic::AtomicPtr;

pub const RETRO_ENVIRONMENT_SET_MEMORY_MAPS: c_uint = 36 | 0x10000; // 65572

#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct RetroMemoryDescriptor {
    pub flags: u64,
    pub ptr: *mut c_void,
    pub offset: usize,
    pub start: usize,
    pub select: usize,
    pub disconnect: usize,
    pub len: usize,
    pub addrspace: *const c_char,
}

#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct RetroMemoryMap {
    pub descriptors: *const RetroMemoryDescriptor,
    pub num_descriptors: c_uint,
}

static VRAM_PTR: AtomicPtr<u8> = AtomicPtr::new(ptr::null_mut());
static PAL_PTR: AtomicPtr<u8> = AtomicPtr::new(ptr::null_mut());
static OAM_PTR: AtomicPtr<u8> = AtomicPtr::new(ptr::null_mut());
static IO_PTR: AtomicPtr<u8> = AtomicPtr::new(ptr::null_mut());

unsafe extern "C" fn environment_callback(cmd: c_uint, data: *mut c_void) -> bool {
    const RETRO_ENVIRONMENT_SET_PIXEL_FORMAT: c_uint = 10;
    const RETRO_PIXEL_FORMAT_RGB565: c_uint = 2;
    const RETRO_PIXEL_FORMAT_0RGB1555: c_uint = 0;

    if cmd == RETRO_ENVIRONMENT_SET_PIXEL_FORMAT && !data.is_null() {
        let fmt = *(data as *const c_uint);
        return fmt == RETRO_PIXEL_FORMAT_0RGB1555 || fmt == RETRO_PIXEL_FORMAT_RGB565;
    }

    if cmd == RETRO_ENVIRONMENT_SET_MEMORY_MAPS && !data.is_null() {
        let mmaps = &*(data as *const RetroMemoryMap);
        for i in 0..mmaps.num_descriptors {
            let desc = *mmaps.descriptors.add(i as usize);
            if desc.ptr.is_null() { continue; }
            if desc.start == 0x06000000 {
                VRAM_PTR.store(desc.ptr as *mut u8, Ordering::SeqCst);
            } else if desc.start == 0x05000000 {
                PAL_PTR.store(desc.ptr as *mut u8, Ordering::SeqCst);
            } else if desc.start == 0x07000000 {
                OAM_PTR.store(desc.ptr as *mut u8, Ordering::SeqCst);
            } else if desc.start == 0x04000000 {
                IO_PTR.store(desc.ptr as *mut u8, Ordering::SeqCst);
            }
        }
        return true;
    }

    false
}

pub struct RetroCore {
    _lib: &'static Library,
    retro_run: Symbol<'static, unsafe extern "C" fn()>,
    retro_serialize: Option<Symbol<'static, unsafe extern "C" fn(*mut c_void, usize) -> bool>>,
    retro_unserialize: Option<Symbol<'static, unsafe extern "C" fn(*const c_void, usize) -> bool>>,
    state_buffer: Vec<u8>,
    pub runahead_frames: u8,
    pub rom_title: String,
    pub rom_game_code: String,
}

impl RetroCore {
    pub fn load(core_path: &str, rom_path: &str) -> Result<Self> {
        let rom_info = crate::runahead_db::inspect_gba_rom(rom_path);
        println!(
            "Loaded GBA ROM: '{}' [{}] -> Auto Runahead: {}F",
            rom_info.title, rom_info.game_code, rom_info.recommended_runahead
        );

        let lib: &'static Library = Box::leak(Box::new(
            unsafe { Library::new(core_path) }
                .context(format!("Failed to load core: {}", core_path))?,
        ));

        unsafe {
            let retro_init: Symbol<unsafe extern "C" fn()> = lib.get(b"retro_init")?;
            let retro_set_environment: Symbol<
                unsafe extern "C" fn(unsafe extern "C" fn(c_uint, *mut c_void) -> bool),
            > = lib.get(b"retro_set_environment")?;
            let retro_set_video_refresh: Symbol<
                unsafe extern "C" fn(
                    unsafe extern "C" fn(*const c_void, c_uint, c_uint, usize),
                ),
            > = lib.get(b"retro_set_video_refresh")?;
            let retro_set_audio_sample: Symbol<
                unsafe extern "C" fn(unsafe extern "C" fn(i16, i16)),
            > = lib.get(b"retro_set_audio_sample")?;
            let retro_set_audio_sample_batch: Symbol<
                unsafe extern "C" fn(unsafe extern "C" fn(*const i16, usize) -> usize),
            > = lib.get(b"retro_set_audio_sample_batch")?;
            let retro_set_input_poll: Symbol<
                unsafe extern "C" fn(unsafe extern "C" fn()),
            > = lib.get(b"retro_set_input_poll")?;
            let retro_set_input_state: Symbol<
                unsafe extern "C" fn(
                    unsafe extern "C" fn(c_uint, c_uint, c_uint, c_uint) -> i16,
                ),
            > = lib.get(b"retro_set_input_state")?;
            let retro_load_game: Symbol<
                unsafe extern "C" fn(*const RetroGameInfo) -> bool,
            > = lib.get(b"retro_load_game")?;
            let retro_run: Symbol<unsafe extern "C" fn()> = lib.get(b"retro_run")?;

            let retro_serialize_size: Option<Symbol<unsafe extern "C" fn() -> usize>> = lib.get(b"retro_serialize_size").ok();
            let retro_serialize: Option<Symbol<unsafe extern "C" fn(*mut c_void, usize) -> bool>> = lib.get(b"retro_serialize").ok();
            let retro_unserialize: Option<Symbol<unsafe extern "C" fn(*const c_void, usize) -> bool>> = lib.get(b"retro_unserialize").ok();

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
            println!("🎮 Core Loaded. PPU Memory Mapped: {}", Self::has_ppu_memory());

            let state_size = if let Some(ref get_sz) = retro_serialize_size {
                get_sz()
            } else {
                0
            };
            let mut state_buffer = vec![0u8; state_size];

            // Auto-calibrate runahead if not already cached
            let final_runahead = if let Some(cached) = crate::runahead_db::lookup_cached(&rom_info.game_code) {
                cached
            } else if let Some(verified) = crate::runahead_db::lookup_verified_db(&rom_info.game_code) {
                verified
            } else if let (Some(ref ser), Some(ref unser)) = (&retro_serialize, &retro_unserialize) {
                // Perform inline 6-frame headless probe (<1ms)
                // 1. Advance 60 frames past boot
                for _ in 0..60 {
                    retro_run();
                }
                
                // 2. Checkpoint state
                let sz = state_buffer.len();
                ser(state_buffer.as_mut_ptr() as *mut c_void, sz);

                // 3. Baseline idle frame
                INPUT_STATE.store(0, Ordering::Relaxed);
                retro_run();
                let baseline = LAST_FRAME_16.lock().unwrap().clone();

                // 4. Restore and probe with input
                unser(state_buffer.as_ptr() as *const c_void, sz);
                INPUT_STATE.store((1 << 4) | (1 << 0), Ordering::Relaxed); // Right + A

                let mut measured_lag = 0u8;
                for frame_idx in 0..6 {
                    retro_run();
                    let current = LAST_FRAME_16.lock().unwrap().clone();
                    let diff_count = baseline
                        .iter()
                        .zip(current.iter())
                        .filter(|(a, b)| a != b)
                        .count();
                    if diff_count > 50 {
                        measured_lag = frame_idx as u8;
                        break;
                    }
                }

                // 5. Restore back to clean state
                unser(state_buffer.as_ptr() as *const c_void, sz);
                INPUT_STATE.store(0, Ordering::Relaxed);
                AUDIO_BUFFER.lock().unwrap().clear();

                println!("🎯 Auto-probed ROM '{}' -> Measured Lag: {}F (saved to ~/.config)", rom_info.title, measured_lag);
                crate::runahead_db::cache_measured_runahead(rom_info.game_code.clone(), measured_lag);
                measured_lag
            } else {
                1
            };

            println!(
                "⚡ GBA Core Active: '{}' [{}] -> Run-Ahead: {}F",
                rom_info.title, rom_info.game_code, final_runahead
            );

            Ok(Self {
                _lib: lib,
                retro_run,
                retro_serialize,
                retro_unserialize,
                state_buffer,
                runahead_frames: final_runahead,
                rom_title: rom_info.title,
                rom_game_code: rom_info.game_code,
            })
        }
    }

    pub fn set_input(&self, mask: i16) {
        INPUT_STATE.store(mask, Ordering::Relaxed);
    }

    pub fn set_runahead_frames(&mut self, frames: u8) {
        self.runahead_frames = frames.min(2);
    }

    pub fn step(&mut self) -> (u32, Vec<u16>, Vec<i16>) {
        let t0 = Instant::now();
        
        if self.runahead_frames > 0 && self.retro_serialize.is_some() && self.retro_unserialize.is_some() && !self.state_buffer.is_empty() {
            // ==========================================
            // MULTI-FRAME RUN-AHEAD INPUT LAG ELIMINATION
            // ==========================================
            // 1. Advance canonical frame with user input
            unsafe { (self.retro_run)(); }
            
            // 2. Capture canonical audio stream for real-time fidelity
            let canonical_audio = {
                let mut audio = AUDIO_BUFFER.lock().unwrap();
                let samples = audio.clone();
                audio.clear();
                samples
            };

            // 3. Checkpoint canonical state
            let sz = self.state_buffer.len();
            unsafe {
                if let Some(ref ser) = self.retro_serialize {
                    ser(self.state_buffer.as_mut_ptr() as *mut c_void, sz);
                }
            }

            // 4. Fast-forward N frames ahead into the future
            for _ in 0..self.runahead_frames {
                unsafe { (self.retro_run)(); }
            }

            // 5. Capture future video frame (instant button reaction)
            let future_frame = LAST_FRAME_16.lock().unwrap().clone();

            // Discard speculative fast-forward audio
            {
                let mut audio = AUDIO_BUFFER.lock().unwrap();
                audio.clear();
            }

            // 6. Rollback state to canonical frame
            unsafe {
                if let Some(ref unser) = self.retro_unserialize {
                    unser(self.state_buffer.as_ptr() as *const c_void, sz);
                }
            }

            let sim_us = t0.elapsed().as_micros() as u32;
            (sim_us, future_frame, canonical_audio)
        } else {
            // Standard Emulation (0 Run-Ahead Frames)
            unsafe {
                (self.retro_run)();
            }
            let sim_us = t0.elapsed().as_micros() as u32;
            let frame = LAST_FRAME_16.lock().unwrap().clone();
            let mut audio = AUDIO_BUFFER.lock().unwrap();
            let audio_samples = audio.clone();
            audio.clear();
            (sim_us, frame, audio_samples)
        }
    }

    pub fn has_ppu_memory() -> bool {
        !VRAM_PTR.load(Ordering::Relaxed).is_null()
            && !PAL_PTR.load(Ordering::Relaxed).is_null()
            && !OAM_PTR.load(Ordering::Relaxed).is_null()
            && !IO_PTR.load(Ordering::Relaxed).is_null()
    }

    pub fn read_ppu_state(
        vram_out: &mut [u8],
        pal_out: &mut [u8],
        oam_out: &mut [u8],
        io_out: &mut [u8],
    ) -> bool {
        let vram = VRAM_PTR.load(Ordering::Relaxed);
        let pal = PAL_PTR.load(Ordering::Relaxed);
        let oam = OAM_PTR.load(Ordering::Relaxed);
        let io = IO_PTR.load(Ordering::Relaxed);

        if vram.is_null() || pal.is_null() || oam.is_null() || io.is_null() {
            return false;
        }

        unsafe {
            if vram_out.len() <= 0x18000 {
                ptr::copy_nonoverlapping(vram, vram_out.as_mut_ptr(), vram_out.len());
            }
            if pal_out.len() <= 0x400 {
                ptr::copy_nonoverlapping(pal, pal_out.as_mut_ptr(), pal_out.len());
            }
            if oam_out.len() <= 0x400 {
                ptr::copy_nonoverlapping(oam, oam_out.as_mut_ptr(), oam_out.len());
            }
            if io_out.len() <= 0x400 {
                ptr::copy_nonoverlapping(io, io_out.as_mut_ptr(), io_out.len());
            }
        }
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_retro_memory_struct_layout() {
        assert_eq!(std::mem::size_of::<RetroMemoryDescriptor>(), 64);
        assert_eq!(std::mem::size_of::<RetroMemoryMap>(), 16);
    }

    #[test]
    fn test_ppu_memory_pointers_initial_and_update() {
        let dummy_vram = vec![0x11u8; 98304];
        let dummy_pal = vec![0x22u8; 1024];
        let dummy_oam = vec![0x33u8; 1024];
        let dummy_io = vec![0x44u8; 128];

        let descs = vec![
            RetroMemoryDescriptor {
                flags: 0,
                ptr: dummy_vram.as_ptr() as *mut c_void,
                offset: 0,
                start: 0x06000000,
                select: 0,
                disconnect: 0,
                len: 98304,
                addrspace: ptr::null(),
            },
            RetroMemoryDescriptor {
                flags: 0,
                ptr: dummy_pal.as_ptr() as *mut c_void,
                offset: 0,
                start: 0x05000000,
                select: 0,
                disconnect: 0,
                len: 1024,
                addrspace: ptr::null(),
            },
            RetroMemoryDescriptor {
                flags: 0,
                ptr: dummy_oam.as_ptr() as *mut c_void,
                offset: 0,
                start: 0x07000000,
                select: 0,
                disconnect: 0,
                len: 1024,
                addrspace: ptr::null(),
            },
            RetroMemoryDescriptor {
                flags: 0,
                ptr: dummy_io.as_ptr() as *mut c_void,
                offset: 0,
                start: 0x04000000,
                select: 0,
                disconnect: 0,
                len: 1024,
                addrspace: ptr::null(),
            },
        ];

        let mmap = RetroMemoryMap {
            descriptors: descs.as_ptr(),
            num_descriptors: descs.len() as c_uint,
        };

        unsafe {
            let res = environment_callback(
                RETRO_ENVIRONMENT_SET_MEMORY_MAPS,
                &mmap as *const _ as *mut c_void,
            );
            assert!(res);
        }

        assert!(RetroCore::has_ppu_memory());

        let mut read_v = vec![0u8; 98304];
        let mut read_p = vec![0u8; 1024];
        let mut read_o = vec![0u8; 1024];
        let mut read_i = vec![0u8; 128];

        let ok = RetroCore::read_ppu_state(&mut read_v, &mut read_p, &mut read_o, &mut read_i);
        assert!(ok);
        assert_eq!(read_v[0], 0x11);
        assert_eq!(read_p[0], 0x22);
        assert_eq!(read_o[0], 0x33);
        assert_eq!(read_i[0], 0x44);
    }
}
