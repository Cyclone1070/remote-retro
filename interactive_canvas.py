#!/usr/bin/env python3
import socket
import time
import threading
import json
import math
import subprocess
import os
import io
from PIL import Image, ImageDraw, ImageFont

INPUT_PORT = 49152
VIDEO_PORT = 49153
CMD_PORT = 49154
NL = bytes([10])

FRAME_DIR = '/tmp/ephemeral_frames'
os.makedirs(FRAME_DIR, exist_ok=True)

stop_event = threading.Event()
active_input_flash = {'active': False, 'seq': 0, 't_recv_ns': 0}
lock = threading.Lock()

# 1. Input Receiver & Optical Flash Trigger
def input_server():
    sock = socket.socket(socket.AF_INET, socket.SOCK_DGRAM)
    sock.bind(('0.0.0.0', INPUT_PORT))
    sock.settimeout(0.5)
    while not stop_event.is_set():
        try:
            data, addr = sock.recvfrom(1024)
            t_recv = time.time_ns()
            parts = data.decode().split(',')
            input_seq = int(parts[1])
            with lock:
                active_input_flash['active'] = True
                active_input_flash['seq'] = input_seq
                active_input_flash['t_recv_ns'] = t_recv
            # Echo ACK immediately
            resp = data + b',' + str(t_recv).encode()
            sock.sendto(resp, addr)
        except socket.timeout:
            continue
        except Exception:
            pass
    sock.close()

# 2. Dynamic 1080p60 Frame Generator (Renders 3D cube, gradients, timestamps, and optical flash)
def render_frame(frame_idx, angle, flash_state):
    width, height = 1920, 1080
    img = Image.new('RGB', (width, height), color=(20, 24, 33))
    draw = ImageDraw.Draw(img)
    
    # Background gradient bars (forces H.264 macroblock complexity)
    for i in range(0, width, 60):
        color_val = int(128 + 127 * math.sin(math.radians(angle * 2 + i)))
        draw.rectangle([i, 0, i + 50, 80], fill=(color_val, 60, 200 - color_val // 2))
        
    # Rotating 3D wireframe cube
    cx, cy = 960, 540
    size = 220
    rad = math.radians(angle)
    nodes = [
        [-1, -1, -1], [1, -1, -1], [1, 1, -1], [-1, 1, -1],
        [-1, -1, 1], [1, -1, 1], [1, 1, 1], [-1, 1, 1]
    ]
    edges = [
        (0,1), (1,2), (2,3), (3,0),
        (4,5), (5,6), (6,7), (7,4),
        (0,4), (1,5), (2,6), (3,7)
    ]
    rot_nodes = []
    for x, y, z in nodes:
        # Rotate around X and Y
        xz = x * math.cos(rad) - z * math.sin(rad)
        zz = x * math.sin(rad) + z * math.cos(rad)
        yz = y * math.cos(rad) - zz * math.sin(rad)
        zz2 = y * math.sin(rad) + zz * math.cos(rad)
        focal = 4.0
        pz = zz2 + 5.0
        px = int(cx + (xz * focal / pz) * size)
        py = int(cy + (yz * focal / pz) * size)
        rot_nodes.append((px, py))
        
    for u, v in edges:
        draw.line([rot_nodes[u], rot_nodes[v]], fill=(0, 230, 255), width=4)
        
    # Optical Flash Target Box (Top-Right: 1650, 100 to 1850, 300)
    flash_active = flash_state.get('active', False)
    flash_seq = flash_state.get('seq', 0)
    if flash_active:
        # Bright White Flash (#FFFFFF) on optical sensor area
        draw.rectangle([1650, 100, 1850, 300], fill=(255, 255, 255), outline=(255, 255, 0), width=6)
        draw.text((1670, 180), f'FLASH #{flash_seq}', fill=(0, 0, 0))
    else:
        # Dark Black (#050505) when idle
        draw.rectangle([1650, 100, 1850, 300], fill=(5, 5, 5), outline=(60, 60, 60), width=4)
        draw.text((1670, 180), 'IDLE TARGET', fill=(100, 100, 100))
        
    # Timestamp & Frame Info Banner
    t_now = time.strftime('%H:%M:%S') + f'.{int(time.time()*1000)%1000:03d}'
    banner = f'ELITEDESK HEADLESS | Frame: #{frame_idx:06d} | Time: {t_now} | Optical Flash: {flash_active}'
    draw.rectangle([100, 960, 1820, 1040], fill=(35, 40, 55), outline=(0, 180, 220), width=2)
    draw.text((130, 985), banner, fill=(255, 255, 255))
    
    return img

# 3. Sunshine 1-to-1 Frame Burst Video Worker
def sunshine_streaming_worker(client_ip, client_port, target_mbps, duration_sec, is_remote=False, fps=60.0):
    sock = socket.socket(socket.AF_INET, socket.SOCK_DGRAM)
    packet_size = 1024 if is_remote else 1392
    frame_interval = 1.0 / fps
    bytes_per_frame_raw = (target_mbps * 1_000_000 / 8) / fps
    bytes_per_frame_fec = bytes_per_frame_raw * 1.20
    shards_per_frame = max(1, math.ceil(bytes_per_frame_fec / packet_size))
    
    burst_window_sec = 0.0015
    inter_shard_delay = burst_window_sec / shards_per_frame
    
    total_frames = int(duration_sec * fps)
    angle = 0.0
    
    for frame_idx in range(total_frames):
        if stop_event.is_set():
            break
        frame_start = time.perf_counter()
        t_send_ns = time.time_ns()
        
        with lock:
            flash_state = dict(active_input_flash)
            # Reset flash after one frame display (optical frame reaction)
            if active_input_flash['active']:
                active_input_flash['active'] = False
                
        # Render frame
        img = render_frame(frame_idx, angle, flash_state)
        angle = (angle + 3.0) % 360.0
        
        # Save snapshot for first 3 frames and when flash active
        if frame_idx < 3 or flash_state.get('active'):
            snap_path = f'{FRAME_DIR}/frame_{frame_idx:04d}_host.png'
            if not os.path.exists(snap_path):
                img.save(snap_path)
                
        # Send Sunshine 1-to-1 Shard Burst
        is_flash_int = 1 if flash_state.get('active') else 0
        flash_seq_int = flash_state.get('seq', 0)
        
        for shard_idx in range(shards_per_frame):
            hdr = f'{frame_idx}:{shard_idx}:{shards_per_frame}:{t_send_ns}:{is_flash_int}:{flash_seq_int}:'.encode()
            padding = b'V' * max(0, packet_size - len(hdr))
            try:
                sock.sendto(hdr + padding, (client_ip, client_port))
            except Exception:
                pass
            if inter_shard_delay > 0:
                time.sleep(inter_shard_delay)
                
        elapsed = time.perf_counter() - frame_start
        sleep_rem = frame_interval - elapsed
        if sleep_rem > 0:
            time.sleep(sleep_rem)
            
    sock.close()

def cmd_server():
    sock = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
    sock.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
    sock.bind(('0.0.0.0', CMD_PORT))
    sock.listen(5)
    sock.settimeout(1.0)
    while not stop_event.is_set():
        try:
            conn, addr = sock.accept()
            raw = conn.recv(4096).decode().strip()
            if not raw:
                conn.close()
                continue
            req = json.loads(raw)
            action = req.get('action')
            if action == 'start_stream':
                c_ip = req['client_ip']
                c_port = int(req['client_port'])
                mbps = float(req.get('mbps', 20.0))
                dur = float(req.get('duration', 5.0))
                remote = bool(req.get('is_remote', False))
                t = threading.Thread(target=sunshine_streaming_worker, args=(c_ip, c_port, mbps, dur, remote))
                t.daemon = True
                t.start()
                conn.sendall(json.dumps({'status': 'started'}).encode() + NL)
            elif action == 'set_powersave':
                val = req.get('value', 'on')
                cmd = f'sudo iw dev wlp0s20f3 set power_save {val}'
                out = subprocess.getoutput(cmd)
                conn.sendall(json.dumps({'status': 'ok', 'output': out}).encode() + NL)
            elif action == 'get_frame_list':
                files = sorted(os.listdir(FRAME_DIR)) if os.path.exists(FRAME_DIR) else []
                conn.sendall(json.dumps({'status': 'ok', 'files': files}).encode() + NL)
            elif action == 'ping':
                conn.sendall(json.dumps({'status': 'pong'}).encode() + NL)
            elif action == 'stop':
                stop_event.set()
                conn.sendall(json.dumps({'status': 'stopping'}).encode() + NL)
                conn.close()
                break
            conn.close()
        except socket.timeout:
            continue
        except Exception:
            pass
    sock.close()

if __name__ == '__main__':
    t_in = threading.Thread(target=input_server)
    t_in.daemon = True
    t_in.start()
    cmd_server()
