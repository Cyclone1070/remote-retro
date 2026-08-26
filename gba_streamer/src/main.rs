use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use futures_util::{SinkExt, StreamExt};
use libloading::{Library, Symbol};
use std::collections::HashMap;
use std::ffi::{c_char, c_uint, c_void, CString};
use std::fs;
use std::net::UdpSocket;
use std::ptr;
use std::sync::{atomic::{AtomicI16, AtomicU32, AtomicU64, Ordering}, Arc, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use tokio::sync::Notify;
use warp::ws::Ws;
use warp::Filter;

const GBA_WIDTH: usize = 240;
const GBA_HEIGHT: usize = 160;
const TOTAL_PIXELS: usize = GBA_WIDTH * GBA_HEIGHT; // 38,400

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
static INPUT_STATE: AtomicI16 = AtomicI16::new(0);
static LAST_INPUT_SEQ: AtomicU32 = AtomicU32::new(0);
static LAST_INPUT_TIMESTAMP_US: AtomicU64 = AtomicU64::new(0);

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

unsafe extern "C" fn audio_sample_callback(_left: i16, _right: i16) {}
unsafe extern "C" fn audio_sample_batch_callback(_data: *const i16, frames: usize) -> usize {
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

const BROWSER_HTML: &str = r#"<!DOCTYPE html>
<html>
<head>
    <title>⚡ Dynamic 8-Bit Palette GBA Streamer</title>
    <style>
        body { margin: 0; background: #09090b; color: #f4f4f5; font-family: -apple-system, BlinkMacSystemFont, 'Segoe UI', Roboto, monospace; display: flex; flex-direction: column; align-items: center; justify-content: center; height: 100vh; overflow: hidden; }
        h1 { margin: 0 0 8px 0; font-size: 22px; color: #22c55e; letter-spacing: -0.5px; }
        .main-container { display: flex; flex-direction: column; align-items: center; position: relative; }
        .canvas-wrapper { position: relative; width: 720px; height: 480px; }
        canvas { image-rendering: pixelated; border: 3px solid #27272a; border-radius: 8px; box-shadow: 0 12px 40px rgba(0,0,0,0.9); background: #000; width: 100%; height: 100%; }
        
        #loadingOverlay {
            position: absolute;
            top: 0; left: 0; right: 0; bottom: 0;
            background: rgba(9, 9, 11, 0.92);
            backdrop-filter: blur(8px);
            border-radius: 8px;
            display: flex;
            flex-direction: column;
            align-items: center;
            justify-content: center;
            z-index: 50;
            transition: opacity 0.2s ease, visibility 0.2s ease;
        }
        .spinner {
            width: 44px; height: 44px;
            border: 4px solid #27272a;
            border-top: 4px solid #22c55e;
            border-radius: 50%;
            animation: spin 0.8s linear infinite;
            margin-bottom: 14px;
        }
        @keyframes spin { 0% { transform: rotate(0deg); } 100% { transform: rotate(360deg); } }
        .load-title { font-size: 16px; font-weight: bold; color: #f4f4f5; margin-bottom: 6px; }
        .load-sub { font-size: 13px; color: #a1a1aa; }

        .hud-banner {
            margin-top: 14px;
            display: flex;
            align-items: center;
            gap: 16px;
            background: #18181b;
            border: 1px solid #3f3f46;
            padding: 10px 24px;
            border-radius: 8px;
            font-size: 14px;
        }
        .hud-item { display: flex; flex-direction: column; align-items: center; }
        .hud-label { font-size: 10px; color: #a1a1aa; text-transform: uppercase; font-weight: bold; }
        .hud-value { font-size: 16px; font-weight: 700; color: #fafafa; }
        .hud-value.highlight { color: #4ade80; }
        .divider { width: 1px; height: 26px; background: #3f3f46; }

        .controls-card { margin-top: 12px; background: #141417; border: 1px solid #27272a; border-radius: 8px; padding: 10px 20px; display: grid; grid-template-columns: 1fr 1fr; gap: 8px; font-size: 12px; color: #a1a1aa; }
        kbd { background: #27272a; border: 1px solid #52525b; border-radius: 4px; padding: 1px 6px; font-weight: bold; color: #fafafa; }
    </style>
</head>
<body>
    <div class="main-container">
        <h1>⚡ GBA Dynamic Palette Streamer (Bit-Exact Lossless)</h1>
        
        <div class="canvas-wrapper">
            <canvas id="gbaCanvas" width="240" height="160"></canvas>
            <div id="loadingOverlay">
                <div class="spinner"></div>
                <div class="load-title">⚡ Connecting to GBA Stream...</div>
                <div class="load-sub" id="connStatus">Receiving 8-bit dynamic palette frame buffer</div>
            </div>
        </div>

        <div class="hud-banner">
            <div class="hud-item">
                <span class="hud-label">Total M2P Lag</span>
                <span class="hud-value highlight" id="totalLatency">-- ms</span>
            </div>
            <div class="divider"></div>
            <div class="hud-item">
                <span class="hud-label">Network RTT</span>
                <span class="hud-value" id="netLatency">-- ms</span>
            </div>
            <div class="divider"></div>
            <div class="hud-item">
                <span class="hud-label">Host Palette+LZ4</span>
                <span class="hud-value" id="hostLatency">0.34 ms</span>
            </div>
            <div class="divider"></div>
            <div class="hud-item">
                <span class="hud-label">Client Decode</span>
                <span class="hud-value" id="clientLatency">0.06 ms</span>
            </div>
            <div class="divider"></div>
            <div class="hud-item">
                <span class="hud-label">Delivered FPS</span>
                <span class="hud-value highlight" id="fpsVal">60.0</span>
            </div>
        </div>

        <div class="controls-card">
            <div><strong>D-Pad:</strong> <kbd>W</kbd><kbd>A</kbd><kbd>S</kbd><kbd>D</kbd> or <kbd>↑</kbd><kbd>←</kbd><kbd>↓</kbd><kbd>→</kbd></div>
            <div><strong>A Button:</strong> <kbd>Z</kbd> or <kbd>J</kbd></div>
            <div><strong>B Button:</strong> <kbd>X</kbd> or <kbd>K</kbd></div>
            <div><strong>Start / Select:</strong> <kbd>Enter</kbd> / <kbd>Space</kbd></div>
        </div>
    </div>

    <!-- Fast pure JS LZ4 block decompressor -->
    <script>
        function lz4Decompress(input, uncompressedSize) {
            const output = new Uint8Array(uncompressedSize);
            let inputOffset = 4;
            let outputOffset = 0;

            while (inputOffset < input.length && outputOffset < uncompressedSize) {
                const token = input[inputOffset++];
                let literalLength = token >> 4;

                if (literalLength === 15) {
                    let s;
                    do {
                        s = input[inputOffset++];
                        literalLength += s;
                    } while (s === 255);
                }

                for (let i = 0; i < literalLength; i++) {
                    output[outputOffset++] = input[inputOffset++];
                }

                if (outputOffset >= uncompressedSize) break;

                const offset = input[inputOffset++] | (input[inputOffset++] << 8);
                if (offset === 0) break;

                let matchLength = (token & 0x0F) + 4;
                if (matchLength === 19) {
                    let s;
                    do {
                        s = input[inputOffset++];
                        matchLength += s;
                    } while (s === 255);
                }

                let matchSrc = outputOffset - offset;
                for (let i = 0; i < matchLength; i++) {
                    output[outputOffset++] = output[matchSrc++];
                }
            }
            return output;
        }

        const canvas = document.getElementById('gbaCanvas');
        const ctx = canvas.getContext('2d');
        const imgData = ctx.createImageData(240, 160);
        const data32 = new Uint32Array(imgData.data.buffer);
        
        const totalEl = document.getElementById('totalLatency');
        const netEl = document.getElementById('netLatency');
        const hostEl = document.getElementById('hostLatency');
        const clientEl = document.getElementById('clientLatency');
        const fpsEl = document.getElementById('fpsVal');

        const protocol = window.location.protocol === 'https:' ? 'wss:' : 'ws:';
        const wsUrl = protocol + '//' + window.location.host + '/ws';
        const ws = new WebSocket(wsUrl);
        ws.binaryType = 'arraybuffer';

        let inputMask = 0;
        let inputSeq = 0;
        let frameCount = 0;
        let lastTime = performance.now();
        let decompTimes = [];
        let networkRtt = 10.0;
        let latestPacket = null;
        let isReady = false;

        async function updatePing() {
            const t0 = performance.now();
            try {
                const res = await fetch('/ping', { cache: 'no-store' });
                if (res.ok) {
                    networkRtt = performance.now() - t0;
                }
            } catch(e) {}
        }
        setInterval(updatePing, 1000);
        updatePing();

        setInterval(() => {
            if (isReady && ws.readyState === WebSocket.OPEN) {
                inputSeq++;
                const nowUs = Math.round(performance.now() * 1000);
                const buf = new Uint8Array(14);
                buf[0] = inputSeq & 0xFF;
                buf[1] = (inputSeq >> 8) & 0xFF;
                buf[2] = (inputSeq >> 16) & 0xFF;
                buf[3] = (inputSeq >> 24) & 0xFF;
                for (let i = 0; i < 8; i++) {
                    buf[4 + i] = (nowUs / Math.pow(2, 8 * i)) & 0xFF;
                }
                buf[12] = inputMask & 0xFF;
                buf[13] = (inputMask >> 8) & 0xFF;
                ws.send(buf);
            }
        }, 16);

        window.addEventListener('keydown', (e) => {
            const k = e.key.toLowerCase();
            if (k === 'z' || k === 'j') { inputMask |= (1 << 0); e.preventDefault(); }
            else if (k === 'x' || k === 'k') { inputMask |= (1 << 1); e.preventDefault(); }
            else if (k === ' ') { inputMask |= (1 << 2); e.preventDefault(); }
            else if (k === 'enter') { inputMask |= (1 << 3); e.preventDefault(); }
            else if (k === 'd' || k === 'arrowright') { inputMask |= (1 << 4); e.preventDefault(); }
            else if (k === 'a' || k === 'arrowleft') { inputMask |= (1 << 5); e.preventDefault(); }
            else if (k === 'w' || k === 'arrowup') { inputMask |= (1 << 6); e.preventDefault(); }
            else if (k === 's' || k === 'arrowdown') { inputMask |= (1 << 7); e.preventDefault(); }
            else if (k === 'e') { inputMask |= (1 << 8); e.preventDefault(); }
            else if (k === 'q') { inputMask |= (1 << 9); e.preventDefault(); }
        });

        window.addEventListener('keyup', (e) => {
            const k = e.key.toLowerCase();
            if (k === 'z' || k === 'j') { inputMask &= ~(1 << 0); e.preventDefault(); }
            else if (k === 'x' || k === 'k') { inputMask &= ~(1 << 1); e.preventDefault(); }
            else if (k === ' ') { inputMask &= ~(1 << 2); e.preventDefault(); }
            else if (k === 'enter') { inputMask &= ~(1 << 3); e.preventDefault(); }
            else if (k === 'd' || k === 'arrowright') { inputMask &= ~(1 << 4); e.preventDefault(); }
            else if (k === 'a' || k === 'arrowleft') { inputMask &= ~(1 << 5); e.preventDefault(); }
            else if (k === 'w' || k === 'arrowup') { inputMask &= ~(1 << 6); e.preventDefault(); }
            else if (k === 's' || k === 'arrowdown') { inputMask &= ~(1 << 7); e.preventDefault(); }
            else if (k === 'e') { inputMask &= ~(1 << 8); e.preventDefault(); }
            else if (k === 'q') { inputMask &= ~(1 << 9); e.preventDefault(); }
        });

        ws.onmessage = (event) => {
            latestPacket = new Uint8Array(event.data);
        };

        function renderLoop() {
            if (latestPacket && latestPacket.length >= 33) {
                if (!isReady) {
                    isReady = true;
                    const overlay = document.getElementById('loadingOverlay');
                    if (overlay) {
                        overlay.style.opacity = '0';
                        setTimeout(() => { overlay.style.visibility = 'hidden'; }, 200);
                    }
                }

                const t0 = performance.now();
                const bytes = latestPacket;
                latestPacket = null;

                const hostSimUs = (bytes[12] | (bytes[13] << 8) | (bytes[14] << 16) | (bytes[15] << 24));
                const hostEncUs = (bytes[20] | (bytes[21] << 8) | (bytes[22] << 16) | (bytes[23] << 24));
                const hostTotalMs = (hostSimUs + hostEncUs) / 1000.0;

                const flag = bytes[32];
                const payload = bytes.subarray(33);

                if (flag === 2) {
                    // Dynamic 8-Bit Palette Frame (Bit-Exact Lossless)
                    const decomp = lz4Decompress(payload, 2 + 512 + 38400);
                    const palLen = decomp[0] | (decomp[1] << 8);
                    const pal16 = new Uint16Array(decomp.buffer, decomp.byteOffset + 2, palLen);
                    const indices = decomp.subarray(2 + palLen * 2);

                    for (let i = 0; i < 38400; i++) {
                        const p16 = pal16[indices[i]];
                        const r = ((p16 & 0x7C00) >> 10) * 255 / 31;
                        const g = ((p16 & 0x03E0) >> 5) * 255 / 31;
                        const b = (p16 & 0x001F) * 255 / 31;
                        data32[i] = (255 << 24) | (b << 16) | (g << 8) | r;
                    }
                } else if (flag === 1) {
                    // Raw 16-Bit RGB555 Frame
                    const raw = lz4Decompress(payload, 38400 * 2);
                    const src16 = new Uint16Array(raw.buffer, raw.byteOffset, 38400);
                    for (let i = 0; i < 38400; i++) {
                        const p16 = src16[i];
                        const r = ((p16 & 0x7C00) >> 10) * 255 / 31;
                        const g = ((p16 & 0x03E0) >> 5) * 255 / 31;
                        const b = (p16 & 0x001F) * 255 / 31;
                        data32[i] = (255 << 24) | (b << 16) | (g << 8) | r;
                    }
                }

                ctx.putImageData(imgData, 0, 0);
                const t1 = performance.now();

                const clientDur = t1 - t0;
                decompTimes.push(clientDur);
                frameCount++;

                if (frameCount % 30 === 0) {
                    const now = performance.now();
                    const fps = (30 / ((now - lastTime) / 1000)).toFixed(1);
                    lastTime = now;
                    const avgClient = decompTimes.reduce((a,b)=>a+b,0)/decompTimes.length;
                    const totalM2P = networkRtt + hostTotalMs + avgClient;

                    totalEl.innerText = totalM2P.toFixed(1) + ' ms';
                    netEl.innerText = networkRtt.toFixed(1) + ' ms';
                    hostEl.innerText = hostTotalMs.toFixed(2) + ' ms';
                    clientEl.innerText = avgClient.toFixed(2) + ' ms';
                    fpsEl.innerText = fps;
                    decompTimes = [];
                }
            }
            requestAnimationFrame(renderLoop);
        }
        requestAnimationFrame(renderLoop);
    </script>
</body>
</html>"#;

#[derive(Parser)]
#[command(name = "gba_streamer")]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    Host {
        #[arg(long, default_value = "/usr/lib64/libretro/mgba_libretro.so")]
        core: String,
        #[arg(long, default_value = "/tmp/test_rom.gba")]
        rom: String,
        #[arg(long, default_value = "0.0.0.0:48500")]
        bind: String,
        #[arg(long)]
        client: Option<String>,
        #[arg(long, default_value_t = 0)]
        frames: u64,
    },
    WebHost {
        #[arg(long, default_value = "/usr/lib64/libretro/mgba_libretro.so")]
        core: String,
        #[arg(long, default_value = "/tmp/test_rom.gba")]
        rom: String,
        #[arg(long, default_value = "0.0.0.0:48500")]
        bind: String,
        #[arg(long, default_value_t = 0)]
        frames: u64,
    },
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();

    match cli.command {
        Commands::Host {
            core,
            rom,
            bind,
            client,
            frames,
        } => {
            println!("=== Starting GBA Streaming Host (Native Dynamic Palette UDP) ===");
            let lib: &'static Library = Box::leak(Box::new(
                unsafe { Library::new(&core) }
                    .context(format!("Failed to load core: {}", core))?,
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

                let rom_data = fs::read(&rom)?;
                let c_path = CString::new(rom.as_str())?;
                let info = RetroGameInfo {
                    path: c_path.as_ptr(),
                    data: rom_data.as_ptr() as *const c_void,
                    size: rom_data.len(),
                    meta: ptr::null(),
                };
                retro_load_game(&info);

                let socket = UdpSocket::bind(&bind)?;
                socket.set_nonblocking(true)?;
                println!("Host listening on UDP: {}", bind);

                let mut client_target = client;
                let mut frame_idx = 0u64;

                let mut color_map: HashMap<u16, u8> = HashMap::with_capacity(256);
                let mut pal_table: Vec<u16> = Vec::with_capacity(256);
                let mut indexed_pixels = vec![0u8; TOTAL_PIXELS];
                let mut pal_payload = Vec::with_capacity(2 + 512 + TOTAL_PIXELS);

                while frames == 0 || frame_idx < frames {
                    let frame_start = Instant::now();

                    let mut input_buf = [0u8; 16];
                    while let Ok((len, src)) = socket.recv_from(&mut input_buf) {
                        if len >= 14 {
                            let seq = u32::from_le_bytes(input_buf[0..4].try_into().unwrap_or_default());
                            let t_us = u64::from_le_bytes(input_buf[4..12].try_into().unwrap_or_default());
                            let mask = (input_buf[12] as i16) | ((input_buf[13] as i16) << 8);
                            LAST_INPUT_SEQ.store(seq, Ordering::Relaxed);
                            LAST_INPUT_TIMESTAMP_US.store(t_us, Ordering::Relaxed);
                            INPUT_STATE.store(mask, Ordering::Relaxed);
                            client_target = Some(src.to_string());
                        }
                    }

                    let t_sim = Instant::now();
                    retro_run();
                    let sim_us = t_sim.elapsed().as_micros() as u32;

                    let raw_frame = LAST_FRAME_16.lock().unwrap().clone();
                    if !raw_frame.is_empty() {
                        let t_enc = Instant::now();
                        color_map.clear();
                        pal_table.clear();

                        let mut fits_palette = true;
                        for p in 0..TOTAL_PIXELS {
                            let c = raw_frame[p];
                            if let Some(&idx) = color_map.get(&c) {
                                indexed_pixels[p] = idx;
                            } else if pal_table.len() < 256 {
                                let idx = pal_table.len() as u8;
                                color_map.insert(c, idx);
                                pal_table.push(c);
                                indexed_pixels[p] = idx;
                            } else {
                                fits_palette = false;
                                break;
                            }
                        }

                        let (flag, payload) = if fits_palette {
                            pal_payload.clear();
                            pal_payload.extend_from_slice(&(pal_table.len() as u16).to_le_bytes());
                            for c in &pal_table { pal_payload.extend_from_slice(&c.to_le_bytes()); }
                            pal_payload.extend_from_slice(&indexed_pixels);
                            (2u8, lz4_flex::compress_prepend_size(&pal_payload))
                        } else {
                            let byte_slice = std::slice::from_raw_parts(raw_frame.as_ptr() as *const u8, raw_frame.len() * 2);
                            (1u8, lz4_flex::compress_prepend_size(byte_slice))
                        };
                        let enc_us = t_enc.elapsed().as_micros() as u32;

                        let matched_seq = LAST_INPUT_SEQ.load(Ordering::Relaxed);

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
            }
        }
        Commands::WebHost {
            core,
            rom,
            bind,
            frames: _,
        } => {
            println!("=== Starting GBA Streaming WebHost (Dynamic 8-Bit Palette Drop Queue) ===");
            let lib: &'static Library = Box::leak(Box::new(
                unsafe { Library::new(&core) }
                    .context(format!("Failed to load core: {}", core))?,
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

                let rom_data = fs::read(&rom)?;
                let c_path = CString::new(rom.as_str())?;
                let info = RetroGameInfo {
                    path: c_path.as_ptr(),
                    data: rom_data.as_ptr() as *const c_void,
                    size: rom_data.len(),
                    meta: ptr::null(),
                };
                retro_load_game(&info);

                let latest_slot: Arc<Mutex<Option<Vec<u8>>>> = Arc::new(Mutex::new(None));
                let notifier = Arc::new(Notify::new());

                let slot_producer = latest_slot.clone();
                let notifier_producer = notifier.clone();

                std::thread::spawn(move || {
                    let mut color_map: HashMap<u16, u8> = HashMap::with_capacity(256);
                    let mut pal_table: Vec<u16> = Vec::with_capacity(256);
                    let mut indexed_pixels = vec![0u8; TOTAL_PIXELS];
                    let mut pal_payload = Vec::with_capacity(2 + 512 + TOTAL_PIXELS);

                    loop {
                        let frame_start = Instant::now();
                        
                        let t_sim = Instant::now();
                        retro_run();
                        let sim_us = t_sim.elapsed().as_micros() as u32;

                        let raw_frame = LAST_FRAME_16.lock().unwrap().clone();
                        if !raw_frame.is_empty() {
                            let t_enc = Instant::now();
                            color_map.clear();
                            pal_table.clear();

                            let mut fits_palette = true;
                            for p in 0..TOTAL_PIXELS {
                                let c = raw_frame[p];
                                if let Some(&idx) = color_map.get(&c) {
                                    indexed_pixels[p] = idx;
                                } else if pal_table.len() < 256 {
                                    let idx = pal_table.len() as u8;
                                    color_map.insert(c, idx);
                                    pal_table.push(c);
                                    indexed_pixels[p] = idx;
                                } else {
                                    fits_palette = false;
                                    break;
                                }
                            }

                            let (flag, payload) = if fits_palette {
                                pal_payload.clear();
                                pal_payload.extend_from_slice(&(pal_table.len() as u16).to_le_bytes());
                                for c in &pal_table { pal_payload.extend_from_slice(&c.to_le_bytes()); }
                                pal_payload.extend_from_slice(&indexed_pixels);
                                (2u8, lz4_flex::compress_prepend_size(&pal_payload))
                            } else {
                                let byte_slice = std::slice::from_raw_parts(raw_frame.as_ptr() as *const u8, raw_frame.len() * 2);
                                (1u8, lz4_flex::compress_prepend_size(byte_slice))
                            };
                            let enc_us = t_enc.elapsed().as_micros() as u32;

                            let now_us = SystemTime::now()
                                .duration_since(UNIX_EPOCH)
                                .unwrap_or_default()
                                .as_micros() as u64;

                            let matched_seq = LAST_INPUT_SEQ.load(Ordering::Relaxed);
                            let matched_t_us = LAST_INPUT_TIMESTAMP_US.load(Ordering::Relaxed);

                            let mut packet = Vec::with_capacity(33 + payload.len());
                            packet.extend_from_slice(&matched_seq.to_le_bytes());
                            packet.extend_from_slice(&matched_t_us.to_le_bytes());
                            packet.extend_from_slice(&sim_us.to_le_bytes());
                            packet.extend_from_slice(&0u32.to_le_bytes());
                            packet.extend_from_slice(&enc_us.to_le_bytes());
                            packet.extend_from_slice(&now_us.to_le_bytes());
                            packet.push(flag);
                            packet.extend_from_slice(&payload);

                            {
                                let mut guard = slot_producer.lock().unwrap();
                                *guard = Some(packet);
                            }
                            notifier_producer.notify_waiters();
                        }

                        let frame_budget = Duration::from_micros(16742);
                        let elapsed = frame_start.elapsed();
                        if elapsed < frame_budget {
                            std::thread::sleep(frame_budget - elapsed);
                        }
                    }
                });

                let addr: std::net::SocketAddr = bind.parse()?;
                println!("WebHost running on http://{}", addr);

                let html_route = warp::path::end().map(|| warp::reply::html(BROWSER_HTML));
                let ping_route = warp::path("ping").map(|| warp::reply::html("pong"));

                let slot_consumer = latest_slot.clone();
                let notifier_consumer = notifier.clone();

                let ws_route = warp::path("ws")
                    .and(warp::ws())
                    .map(move |ws: Ws| {
                        let slot = slot_consumer.clone();
                        let notif = notifier_consumer.clone();

                        ws.on_upgrade(move |websocket| async move {
                            let (mut ws_sender, mut ws_receiver) = websocket.split();
                            println!("Browser client connected via WebSocket!");

                            tokio::spawn(async move {
                                while let Some(Ok(msg)) = ws_receiver.next().await {
                                    if msg.is_binary() {
                                        let bytes = msg.as_bytes();
                                        if bytes.len() >= 14 {
                                            let seq = u32::from_le_bytes(bytes[0..4].try_into().unwrap_or_default());
                                            let t_us = u64::from_le_bytes(bytes[4..12].try_into().unwrap_or_default());
                                            let mask = (bytes[12] as i16) | ((bytes[13] as i16) << 8);
                                            LAST_INPUT_SEQ.store(seq, Ordering::Relaxed);
                                            LAST_INPUT_TIMESTAMP_US.store(t_us, Ordering::Relaxed);
                                            INPUT_STATE.store(mask, Ordering::Relaxed);
                                        } else if bytes.len() >= 2 {
                                            let mask = (bytes[0] as i16) | ((bytes[1] as i16) << 8);
                                            INPUT_STATE.store(mask, Ordering::Relaxed);
                                        }
                                    }
                                }
                            });

                            loop {
                                notif.notified().await;
                                let maybe_frame = {
                                    let mut guard = slot.lock().unwrap();
                                    guard.take()
                                };

                                if let Some(packet) = maybe_frame {
                                    if ws_sender
                                        .send(warp::ws::Message::binary(packet))
                                        .await
                                        .is_err()
                                    {
                                        break;
                                    }
                                }
                            }
                            println!("Client disconnected.");
                        })
                    });

                let routes = html_route.or(ping_route).or(ws_route);
                warp::serve(routes).run(addr).await;
            }
        }
    }

    Ok(())
}
