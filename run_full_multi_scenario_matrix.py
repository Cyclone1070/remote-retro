#!/usr/bin/env python3
import socket
import time
import json
import statistics
import subprocess
import os

HOST_IP = '192.168.1.111'
TS_IP = '100.71.122.56'
BASE_DIR = '/Users/mac/Downloads/ephemeral_stream_benchmark'

def cleanup():
    print('=== AUTO-CLEANUP: PURGING TEST PROCESSES & RESTORING STATE ===', flush=True)
    subprocess.run(['pkill', '-9', '-f', 'Moonlight'], stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
    remote_clean = 'sudo pkill -9 -x sunshine 2>/dev/null || true; sudo pkill -9 -x Xvfb 2>/dev/null || true; sudo rm -rf /tmp/ephemeral_*; sudo firewall-cmd --reload 2>/dev/null || true; sudo iw dev wlp0s20f3 set power_save on 2>/dev/null || true'
    subprocess.run(['ssh', f'cyc@{HOST_IP}', remote_clean], stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
    print('Cleaned up.', flush=True)

try:
    print('=== 1. DEPLOYING INTERACTIVE STREAMER TO ELITEDESK ===', flush=True)
    subprocess.run(['scp', f'{BASE_DIR}/interactive_canvas.py', f'cyc@{HOST_IP}:/tmp/ephemeral_canvas.py'], check=True)
    start_cmd = 'sudo firewall-cmd --add-port=49152-49155/tcp --add-port=49152-49155/udp 2>/dev/null || true; nohup python3 /tmp/ephemeral_canvas.py > /tmp/canvas.log 2>&1 &'
    subprocess.run(['ssh', f'cyc@{HOST_IP}', start_cmd], check=True)
    time.sleep(2)

    def run_scenario(name, target_ip, power_save='off', count=150):
        print(f'Running: {name}...', flush=True)
        # Configure power save
        subprocess.run(['ssh', f'cyc@{HOST_IP}', f'sudo iw dev wlp0s20f3 set power_save {power_save}'], stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
        time.sleep(0.3)

        s = socket.socket(socket.AF_INET, socket.SOCK_DGRAM)
        s.settimeout(0.25)

        rtts = []
        lost = 0

        for i in range(1, count + 1):
            t_send = time.time()
            msg = f'INPUT,{i},{t_send}'.encode()
            try:
                s.sendto(msg, (target_ip, 49152))
                data, _ = s.recvfrom(1024)
                t_recv = time.time()
                parts = data.decode(errors='ignore').split(',')
                if len(parts) >= 3 and int(parts[1]) == i:
                    rtt = (t_recv - t_send) * 1000.0
                    rtts.append(rtt)
                else:
                    lost += 1
            except socket.timeout:
                lost += 1
            time.sleep(0.016)  # 60 Hz input rate

        s.close()

        if not rtts:
            return {'min_ms': 0, 'avg_ms': 0, 'p95_ms': 0, 'max_ms': 0, 'stddev_ms': 0, 'loss_rate_pct': 100.0, 'samples': 0}

        rtts_sorted = sorted(rtts)
        p95_idx = int(len(rtts_sorted) * 0.95)

        return {
            'min_ms': round(min(rtts_sorted), 2),
            'avg_ms': round(statistics.mean(rtts_sorted), 2),
            'p95_ms': round(rtts_sorted[p95_idx], 2),
            'max_ms': round(max(rtts_sorted), 2),
            'stddev_ms': round(statistics.stdev(rtts_sorted) if len(rtts_sorted) > 1 else 0, 2),
            'loss_rate_pct': round((lost / count) * 100.0, 1),
            'samples': len(rtts)
        }

    matrix = {
        'LAN IPv4 (Power Save OFF)': run_scenario('LAN IPv4 (Power Save OFF)', HOST_IP, 'off', count=150),
        'LAN IPv4 (Power Save ON)': run_scenario('LAN IPv4 (Power Save ON)', HOST_IP, 'on', count=150),
        'Tailscale Direct P2P (WireGuard)': run_scenario('Tailscale Direct P2P', TS_IP, 'off', count=150),
        'Tailscale with Power Save ON': run_scenario('Tailscale Power Save ON', TS_IP, 'on', count=150)
    }

    print('=== COMPREHENSIVE BENCHMARK MATRIX RESULTS ===', flush=True)
    print(json.dumps(matrix, indent=2), flush=True)

    with open(f'{BASE_DIR}/full_benchmark_matrix.json', 'w') as f:
        json.dump(matrix, f, indent=2)

finally:
    cleanup()