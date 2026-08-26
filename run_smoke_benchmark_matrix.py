import subprocess
import time
import re
import os
import glob
import json

LAN_IP = "192.168.1.111"
TAILSCALE_HOST = "100.73.151.90"
DEV = "wlp0s20f3"

BENCH_SCENARIOS = [
    {
        "id": "1_lan_ipv4_baseline",
        "name": "1. Direct Local LAN IPv4 (Wi-Fi 5GHz)",
        "target": LAN_IP,
        "motion": False,
        "net_delay": None,
        "net_loss": None
    },
    {
        "id": "2_tailscale_p2p_mesh",
        "name": "2. Tailscale WireGuard P2P (Overlay Mesh)",
        "target": TAILSCALE_HOST,
        "motion": False,
        "net_delay": None,
        "net_loss": None
    },
    {
        "id": "3_wan_broadband_sim",
        "name": "3. Remote WAN Broadband (+25ms RTT, ±2ms jitter)",
        "target": TAILSCALE_HOST,
        "motion": False,
        "net_delay": "25ms 2ms",
        "net_loss": None
    },
    {
        "id": "4_cellular_4g_sim",
        "name": "4. Remote 4G/5G Cellular (+45ms RTT, ±6ms jitter, 1% loss)",
        "target": TAILSCALE_HOST,
        "motion": False,
        "net_delay": "45ms 6ms",
        "net_loss": "1%"
    },
    {
        "id": "5_congested_derp_sim",
        "name": "5. Congested Relay / High Latency (+60ms RTT, ±8ms jitter, 2% loss)",
        "target": TAILSCALE_HOST,
        "motion": False,
        "net_delay": "60ms 8ms",
        "net_loss": "2%"
    },
    {
        "id": "6_wifi_interference_3pct",
        "name": "6. Degraded Local Wi-Fi (LAN + 3% Packet Loss)",
        "target": LAN_IP,
        "motion": False,
        "net_delay": None,
        "net_loss": "3%"
    },
    {
        "id": "7_high_motion_glxgears",
        "name": "7. High Motion Graphics Load (LAN + glxgears 3D animation)",
        "target": LAN_IP,
        "motion": True,
        "net_delay": None,
        "net_loss": None
    }
]

def reset_host():
    subprocess.run(["ssh", f"cyc@{LAN_IP}", f"sudo tc qdisc del dev {DEV} root 2>/dev/null || true; pkill -9 -x glxgears 2>/dev/null || true; systemctl --user restart ephemeral-sunshine 2>/dev/null || true"], check=False)
    time.sleep(2)

def set_conditions(sc):
    net_cmds = []
    if sc.get("net_delay") and sc.get("net_loss"):
        net_cmds.append(f"sudo tc qdisc add dev {DEV} root netem delay {sc['net_delay']} loss {sc['net_loss']}")
    elif sc.get("net_delay"):
        net_cmds.append(f"sudo tc qdisc add dev {DEV} root netem delay {sc['net_delay']}")
    elif sc.get("net_loss"):
        net_cmds.append(f"sudo tc qdisc add dev {DEV} root netem loss {sc['net_loss']}")
    
    if sc.get("motion"):
        net_cmds.append("DISPLAY=:99 nohup glxgears -geometry 800x600+500+200 >/dev/null 2>&1 &")
    
    if net_cmds:
        subprocess.run(["ssh", f"cyc@{LAN_IP}", "; ".join(net_cmds)], check=False)
        time.sleep(1)

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

print("=== STARTING COMPREHENSIVE SMOKE BENCHMARK MATRIX ===")
for sc in BENCH_SCENARIOS:
    print(f"\n--- [RUNNING] {sc['name']} ---")
    reset_host()
    set_conditions(sc)
    
    pre_logs = set(glob.glob('/tmp/Moonlight-*.log'))
    cmd = [
        '/Applications/Moonlight.app/Contents/MacOS/Moonlight',
        'stream', sc['target'], 'Desktop',
        '--1080', '--fps', '60',
        '--bitrate', '20000',
        '--video-codec', 'H.264',
        '--video-decoder', 'software',
        '--display-mode', 'windowed'
    ]
    
    subprocess.run(['pkill', '-9', '-f', 'Moonlight'])
    time.sleep(1)
    
    proc = subprocess.Popen(cmd)
    time.sleep(13)
    
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
        print(f"Stats Extracted: {stats}")
        
    results.append({
        "scenario": sc,
        "stats": stats,
        "log": target_log
    })

reset_host()

with open("/tmp/smoke_benchmark_final_matrix.json", "w") as f:
    json.dump(results, f, indent=2)

print("\n=== SMOKE BENCHMARK COMPLETE ===")
