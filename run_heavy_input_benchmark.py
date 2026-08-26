import subprocess
import time
import re
import os
import glob
import json

HOST = "100.73.151.90"

def parse_log(path):
    with open(path, 'r', errors='ignore') as f:
        text = f.read()
    res = {}
    fps_in = re.search(r'Incoming frame rate from network:\s*([0-9.]+)\s*FPS', text)
    fps_dec = re.search(r'Decoding frame rate:\s*([0-9.]+)\s*FPS', text)
    fps_rend = re.search(r'Rendering frame rate:\s*([0-9.]+)\s*FPS', text)
    host_lat = re.search(r'Host processing latency min/max/average:\s*([0-9.]+)/([0-9.]+)/([0-9.]+)\s*ms', text)
    net_lat = re.search(r'Average network latency:\s*([0-9.]+)\s*ms\s*\(variance:\s*([0-9.]+)\s*ms\)', text)
    dec_time = re.search(r'Average decoding time:\s*([0-9.]+)\s*ms', text)
    queue_delay = re.search(r'Average frame queue delay:\s*([0-9.]+)\s*ms', text)
    rend_time = re.search(r'Average rendering time.*?:\s*([0-9.]+)\s*ms', text)
    loss_net = re.search(r'Frames dropped by your network connection:\s*([0-9.]+)%', text)
    loss_jitter = re.search(r'Frames dropped due to network jitter:\s*([0-9.]+)%', text)

    if fps_in: res['fps_in'] = float(fps_in.group(1))
    if fps_dec: res['fps_dec'] = float(fps_dec.group(1))
    if fps_rend: res['fps_rend'] = float(fps_rend.group(1))
    if host_lat:
        res['host_min'] = float(host_lat.group(1))
        res['host_max'] = float(host_lat.group(2))
        res['host_avg'] = float(host_lat.group(3))
    if net_lat:
        res['net_avg'] = float(net_lat.group(1))
        res['net_var'] = float(net_lat.group(2))
    if dec_time: res['dec_avg'] = float(dec_time.group(1))
    if queue_delay: res['queue_avg'] = float(queue_delay.group(1))
    if rend_time: res['rend_avg'] = float(rend_time.group(1))
    if loss_net: res['drop_net'] = float(loss_net.group(1))
    if loss_jitter: res['drop_jitter'] = float(loss_jitter.group(1))
    
    if all(k in res for k in ['host_avg', 'net_avg', 'dec_avg', 'queue_avg', 'rend_avg']):
        res['total_e2e_avg'] = round(res['host_avg'] + res['net_avg'] + res['dec_avg'] + res['queue_avg'] + res['rend_avg'], 2)
    return res

scenarios = [
    {"name": "1. Zero Input (Idle Baseline)", "input": False, "motion": False},
    {"name": "2. Heavy Input (250 Hz Mouse Injection)", "input": True, "motion": False},
    {"name": "3. Heavy Input + 3D Motion (250 Hz + glxgears)", "input": True, "motion": True}
]

results = []

for sc in scenarios:
    print(f"\n--- [RUNNING] {sc['name']} ---")
    if sc['motion']:
        subprocess.run(['ssh', f'cyc@{HOST}', 'DISPLAY=:99 nohup glxgears -geometry 800x600+500+200 >/dev/null 2>&1 &; systemctl --user restart ephemeral-sunshine'], check=False)
    else:
        subprocess.run(['ssh', f'cyc@{HOST}', 'pkill -9 -x glxgears 2>/dev/null || true; systemctl --user restart ephemeral-sunshine'], check=False)
    time.sleep(2)

    pre = set(glob.glob('/tmp/Moonlight-*.log'))
    cmd = [
        '/Applications/Moonlight.app/Contents/MacOS/Moonlight',
        'stream', HOST, 'Desktop',
        '--1080', '--fps', '60',
        '--bitrate', '10000',
        '--packet-size', '1024',
        '--video-codec', 'H.264',
        '--video-decoder', 'software',
        '--display-mode', 'windowed'
    ]
    subprocess.run(['pkill', '-9', '-f', 'Moonlight'])
    time.sleep(1)
    
    proc = subprocess.Popen(cmd)
    time.sleep(3)
    
    # Bring Moonlight to front
    subprocess.run(['open', '-a', 'Moonlight'])
    
    if sc['input']:
        print("Starting 250 Hz mouse event injection into Moonlight...")
        injector = subprocess.Popen(['swift', '/tmp/inject_mouse.swift'])
        time.sleep(12)
        injector.terminate()
    else:
        print("Running idle stream...")
        time.sleep(12)
        
    proc.terminate()
    try:
        proc.wait(timeout=4)
    except:
        proc.kill()
        
    time.sleep(1)
    post = set(glob.glob('/tmp/Moonlight-*.log'))
    new_logs = list(post - pre)
    target = new_logs[0] if new_logs else None
    stats = {}
    if target:
        stats = parse_log(target)
        print(f"Stats Extracted: {stats}")
    results.append({"scenario": sc, "stats": stats})

subprocess.run(['ssh', f'cyc@{HOST}', 'pkill -9 -x glxgears 2>/dev/null || true; systemctl --user restart ephemeral-sunshine'], check=False)

with open("/tmp/heavy_input_benchmark_results.json", "w") as f:
    json.dump(results, f, indent=2)

print("\n=== INPUT BENCHMARK COMPLETE ===")
