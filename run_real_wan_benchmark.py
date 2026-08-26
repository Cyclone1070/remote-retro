import subprocess
import time
import re
import os
import glob
import json

HOST_TAILSCALE_V4 = "100.73.151.90"
HOST_TAILSCALE_V6 = "fd7a:115c:a1e0::1938:975a"

SCENARIOS = [
    {
        "id": "1_real_wan_ts_v4_20m",
        "name": "1. Real WAN: Tailscale IPv4 (20 Mbps, Static)",
        "target": HOST_TAILSCALE_V4,
        "codec": "H.264",
        "decoder": "software",
        "bitrate": "20000",
        "motion": False
    },
    {
        "id": "2_real_wan_ts_v4_10m",
        "name": "2. Real WAN: Tailscale IPv4 (10 Mbps, Static)",
        "target": HOST_TAILSCALE_V4,
        "codec": "H.264",
        "decoder": "software",
        "bitrate": "10000",
        "motion": False
    },
    {
        "id": "3_real_wan_ts_v4_motion",
        "name": "3. Real WAN: Tailscale IPv4 (20 Mbps, glxgears 3D)",
        "target": HOST_TAILSCALE_V4,
        "codec": "H.264",
        "decoder": "software",
        "bitrate": "20000",
        "motion": True
    },
    {
        "id": "4_real_wan_ts_v6_20m",
        "name": "4. Real WAN: Tailscale IPv6 (20 Mbps, Static)",
        "target": HOST_TAILSCALE_V6,
        "codec": "H.264",
        "decoder": "software",
        "bitrate": "20000",
        "motion": False
    }
]

def reset_host(motion=False):
    cmds = [
        "sudo tc qdisc del dev wlp0s20f3 root 2>/dev/null || true",
        "pkill -9 -x glxgears 2>/dev/null || true",
        "systemctl --user restart ephemeral-sunshine"
    ]
    if motion:
        cmds.append("DISPLAY=:99 nohup glxgears -geometry 800x600+500+200 >/dev/null 2>&1 &")
    subprocess.run(["ssh", f"cyc@{HOST_TAILSCALE_V4}", "; ".join(cmds)], check=False)
    time.sleep(2)

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

results = []

print("=== STARTING REAL WAN BENCHMARK SUITE ===")
for sc in SCENARIOS:
    print(f"\n--- [RUNNING] {sc['name']} ---")
    reset_host(sc['motion'])
    
    pre_logs = set(glob.glob('/tmp/Moonlight-*.log'))
    cmd = [
        '/Applications/Moonlight.app/Contents/MacOS/Moonlight',
        'stream', sc['target'], 'Desktop',
        '--1080', '--fps', '60',
        '--bitrate', sc['bitrate'],
        '--video-codec', sc['codec'],
        '--video-decoder', sc['decoder'],
        '--display-mode', 'windowed'
    ]
    
    subprocess.run(['pkill', '-9', '-f', 'Moonlight'])
    time.sleep(1)
    
    proc = subprocess.Popen(cmd)
    time.sleep(15)
    
    proc.terminate()
    try:
        proc.wait(timeout=4)
    except:
        proc.kill()
        
    time.sleep(1)
    post_logs = set(glob.glob('/tmp/Moonlight-*.log'))
    new_logs = list(post_logs - pre_logs)
    target_log = new_logs[0] if new_logs else None
    
    stats = {}
    if target_log:
        stats = parse_log(target_log)
        print(f"Real WAN Stats Extracted: {stats}")
        
    results.append({
        "scenario": sc,
        "stats": stats,
        "log": target_log
    })

reset_host(False)

with open("/tmp/real_wan_benchmark_results.json", "w") as f:
    json.dump(results, f, indent=2)

print("\n=== REAL WAN BENCHMARK COMPLETE ===")
