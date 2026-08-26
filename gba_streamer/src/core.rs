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

pub struct RetroCore {
    _lib: &'static Library,
    retro_run: Symbol<'static, unsafe extern "C" fn()>,
}

impl RetroCore {
    pub fn load(core_path: &str, rom_path: &str) -> Result<Self> {
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

            Ok(Self {
                _lib: lib,
                retro_run,
            })
        }
    }

    pub fn set_input(&self, mask: i16) {
        INPUT_STATE.store(mask, Ordering::Relaxed);
    }

    pub fn step(&mut self) -> (u32, Vec<u16>, Vec<i16>) {
        let t0 = Instant::now();
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
