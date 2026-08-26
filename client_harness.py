#!/usr/bin/env python3
import subprocess
import time
import socket
import json
import statistics
import os

HOST_IPV4 = '192.168.1.111'
HOST_IPV6 = '2403:4800:258d:601:7a1d:efa5:4c9f:399c'
HOST_TS_IP = '100.73.151.90'
LOCAL_IP = '192.168.1.107'

INPUT_PORT = 49152
VIDEO_PORT = 49153
CMD_PORT = 49154
NL = bytes([10])

DEBUG_DIR = os.path.join(os.path.dirname(__file__), 'debug_frames')
os.makedirs(DEBUG_DIR, exist_ok=True)

def log(msg):
    print(msg, flush=True)

def run_cmd(cmd):
    p = subprocess.Popen(cmd, shell=True, stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True)
    out, err = p.communicate()
    return out, err, p.returncode

def send_cmd(ip, req):
    try:
        family = socket.AF_INET6 if ':' in ip else socket.AF_INET
        s = socket.socket(family, socket.SOCK_STREAM)
        s.settimeout(5.0)
        s.connect((ip, CMD_PORT))
        s.sendall(json.dumps(req).encode() + NL)
        resp = s.recv(4096).decode().strip()
        s.close()
        return json.loads(resp)
    except Exception as e:
        return {'error': str(e)}

log('=== 1. DEPLOYING HEADLESS SETUP VIA ANSIBLE ===')
out, err, code = run_cmd('ansible-playbook -i inventory.ini setup.yml')
log(out)
if code != 0:
    log(f'Ansible setup failed: {err}')
    exit(1)

time.sleep(1.0)

log('=== 2. STREAM DAEMON HEALTHCHECK ===')
pong = send_cmd(HOST_IPV4, {'action': 'ping'})
log(f'Daemon status: {pong}')

def run_closed_loop_benchmark(host_ip, target_mbps=20.0, duration_sec=4.0, is_remote=False):
    family = socket.AF_INET6 if ':' in host_ip else socket.AF_INET
    
    # 1. Bind and prime client UDP sockets BEFORE starting stream
    vid_sock = socket.socket(family, socket.SOCK_DGRAM)
    vid_sock.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
    bind_ip = '::' if ':' in host_ip else '0.0.0.0'
    vid_sock.bind((bind_ip, VIDEO_PORT))
    vid_sock.settimeout(0.05)
    
    inp_sock = socket.socket(family, socket.SOCK_DGRAM)
    inp_sock.settimeout(0.05)
    
    # Send prime packets to host
    vid_sock.sendto(b'prime_video', (host_ip, VIDEO_PORT))
    inp_sock.sendto(b'0,0', (host_ip, INPUT_PORT))
    
    # Determine client IP relative to target
    c_ip = '2403:4800:258d:601:7a1d:efa5:4c9f:399c' if ':' in host_ip else LOCAL_IP
    if host_ip == HOST_TS_IP:
        c_ip = '100.71.122.56' # Local Mac Tailscale IP
        
    # 2. Command host to start stream
    send_cmd(host_ip, {
        'action': 'start_stream',
        'client_ip': c_ip,
        'client_port': VIDEO_PORT,
        'mbps': target_mbps,
        'duration': duration_sec,
        'is_remote': is_remote
    })
    
    input_rtts = []
    motion_to_photon_latencies = []
    video_transit_times = []
    received_frames = set()
    shards_count = 0
    
    start_time = time.time()
    last_input_time = 0
    input_seq = 0
    pending_inputs = {}
    
    while time.time() - start_time < duration_sec:
        now = time.time()
        # 60 Hz input dispatch
        if now - last_input_time >= 0.016:
            input_seq += 1
            t0_ns = time.time_ns()
            pending_inputs[input_seq] = t0_ns
            try:
                inp_sock.sendto(f'{t0_ns},{input_seq}'.encode(), (host_ip, INPUT_PORT))
            except Exception:
                pass
            last_input_time = now
            
        # Poll input ACK
        try:
            ack_data, _ = inp_sock.recvfrom(1024)
            t_ack_ns = time.time_ns()
            parts = ack_data.decode().split(',')
            orig_t0 = int(parts[0])
            rtt_ms = (t_ack_ns - orig_t0) / 1_000_000.0
            if 0 < rtt_ms < 500:
                input_rtts.append(rtt_ms)
        except socket.timeout:
            pass
        except Exception:
            pass
            
        # Poll video frame shard
        try:
            vid_data, _ = vid_sock.recvfrom(2048)
            t_vid_recv_ns = time.time_ns()
            shards_count += 1
            parts = vid_data.split(b':')
            if len(parts) >= 6:
                frame_idx = int(parts[0])
                shard_idx = int(parts[1])
                total_shards = int(parts[2])
                t_host_send_ns = int(parts[3])
                is_flash = int(parts[4])
                flash_seq = int(parts[5])
                
                received_frames.add(frame_idx)
                
                transit_ms = (t_vid_recv_ns - t_host_send_ns) / 1_000_000.0
                if 0 < transit_ms < 500:
                    video_transit_times.append(transit_ms)
                    
                if is_flash and flash_seq in pending_inputs:
                    t0_input_ns = pending_inputs.pop(flash_seq)
                    m2p_ms = (t_vid_recv_ns - t0_input_ns) / 1_000_000.0
                    if 0 < m2p_ms < 500:
                        motion_to_photon_latencies.append(m2p_ms)
        except socket.timeout:
            pass
        except Exception:
            pass
            
    vid_sock.close()
    inp_sock.close()
    
    def calc_stats(arr):
        if not arr:
            return {'min': 0, 'avg': 0, 'max': 0, 'p95': 0}
        arr_sorted = sorted(arr)
        p95 = arr_sorted[int(len(arr_sorted) * 0.95)]
        return {
            'min': round(min(arr), 2),
            'avg': round(statistics.mean(arr), 2),
            'max': round(max(arr), 2),
            'p95': round(p95, 2)
        }
        
    return {
        'total_frames_received': len(received_frames),
        'total_shards_received': shards_count,
        'input_network_rtt_ms': calc_stats(input_rtts),
        'motion_to_photon_input_ms': calc_stats(motion_to_photon_latencies),
        'video_transit_ms': calc_stats(video_transit_times)
    }

results = {}

log('=== SCENARIO 1: LOCAL IPV4 20 Mbps 60 FPS (LAN) ===')
send_cmd(HOST_IPV4, {'action': 'set_powersave', 'value': 'on'})
time.sleep(0.5)
res1 = run_closed_loop_benchmark(HOST_IPV4, target_mbps=20.0, duration_sec=4.0)
results['lan_ipv4_20mbps'] = res1
log(f'Result: {json.dumps(res1, indent=2)}')

log('=== SCENARIO 2: TAILSCALE DIRECT P2P 20 Mbps 60 FPS ===')
res2 = run_closed_loop_benchmark(HOST_TS_IP, target_mbps=20.0, duration_sec=4.0)
results['tailscale_direct_p2p'] = res2
log(f'Result: {json.dumps(res2, indent=2)}')

log('=== SCENARIO 3: TAILSCALE REMOTE / WAN MODE (1024B MTU) ===')
res3 = run_closed_loop_benchmark(HOST_TS_IP, target_mbps=20.0, duration_sec=4.0, is_remote=True)
results['tailscale_remote_wan'] = res3
log(f'Result: {json.dumps(res3, indent=2)}')

log('=== SCENARIO 4: BITRATE STRESS SWEEP (10 vs 25 vs 50 Mbps) ===')
bitrate_sweep = {}
for mbps in [10.0, 25.0, 50.0]:
    r = run_closed_loop_benchmark(HOST_IPV4, target_mbps=mbps, duration_sec=3.0)
    bitrate_sweep[f'{int(mbps)}mbps'] = r
    log(f'{int(mbps)} Mbps: M2P Avg={r["motion_to_photon_input_ms"]["avg"]} ms | Transit Avg={r["video_transit_ms"]["avg"]} ms')
results['bitrate_sweep'] = bitrate_sweep

log('=== PULLING HOST DEBUG FRAMES FOR MANUAL REVIEW ===')
run_cmd(f'scp -o StrictHostKeyChecking=no -r cyc@{HOST_IPV4}:/tmp/ephemeral_frames/* {DEBUG_DIR}/')
saved_frames = os.listdir(DEBUG_DIR)
log(f'Saved debug frames: {len(saved_frames)} images available in {DEBUG_DIR}')

log('=== TEARDOWN: EXECUTING COMPLETE ANSIBLE PURGE ===')
send_cmd(HOST_IPV4, {'action': 'stop'})
out, err, code = run_cmd('ansible-playbook -i inventory.ini teardown.yml')
log(out)
log(f'Teardown status: {"SUCCESS" if code == 0 else "FAILED"}')

log('=== FINAL COMPREHENSIVE BENCHMARK REPORT ===')
log(json.dumps(results, indent=2))
