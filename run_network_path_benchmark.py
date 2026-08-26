import subprocess
import time
import re
import os
import glob
import json

LAN_IP = "192.168.1.111"
TAILSCALE_IP = "100.73.151.90"
DEV = "wlp0s20f3"

NETWORK_PATHS = [
    {
        "id": "1_local_lan",
        "name": "1. Local LAN (Direct Wi-Fi 5GHz)",
        "host": LAN_IP,
        "type": "Direct LAN",
        "net_delay": None,
        "net_loss": None
    },
    {
        "id": "2_tailscale_direct",
        "name": "2. Tailscale WireGuard P2P (Overlay Mesh)",
        "host": TAILSCALE_IP,
        "type": "Tailscale Direct P2P",
        "net_delay": None,
        "net_loss": None
    },
    {
        "id": "3_simulated_wan_broadband",
        "name": "3. Remote WAN Broadband (+25ms RTT, ±2ms jitter)",
        "host": TAILSCALE_IP,
        "type": "WAN Broadband",
        "net_delay": "25ms 2ms",
        "net_loss": None
    },
    {
        "id": "4_simulated_cellular_4g",
        "name": "4. Remote 4G/5G Cellular (+45ms RTT, ±6ms jitter, 1% loss)",
        "host": TAILSCALE_IP,
        "type": "Cellular 4G/5G",
        "net_delay": "45ms 6ms",
        "net_loss": "1%"
    },
    {
        "id": "5_simulated_derp_relay",
        "name": "5. Tailscale DERP Relay Fallback (+70ms RTT, ±10ms jitter, 2% loss)",
        "host": TAILSCALE_IP,
        "type": "DERP Relay / Symmetric NAT",
        "net_delay": "70ms 10ms",
        "net_loss": "2%"
    },
    {
        "id": "6_wifi_interference_loss",
        "name": "6. Local Wi-Fi Interference (Direct LAN + 3% Packet Loss)",
        "host": LAN_IP,
        "type": "Degraded Wi-Fi LAN",
        "net_delay": None,
        "net_loss": "3%"
    }
]

def reset_host():
    subprocess.run(["ssh", f"cyc@{LAN_IP}", f"sudo tc qdisc del dev {DEV} root 2>/dev/null || true; systemctl --user restart ephemeral-sunshine 2>/dev/null || true"], check=False)
    time.sleep(2)

def set_conditions(sc):
    net_cmds = []
    if sc.get("net_delay") and sc.get("net_loss"):
        net_cmds.append(f"sudo tc qdisc add dev {DEV} root netem delay {sc['net_delay']} loss {sc['net_loss']}")
    elif sc.get("net_delay"):
        net_cmds.append(f"sudo tc qdisc add dev {DEV} root netem delay {sc['net_delay']}")
    elif sc.get("net_loss"):
        net_cmds.append(f"sudo tc qdisc add dev {DEV} root netem loss {sc['net_loss']}")
    
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

print("=== STARTING MULTI-NETWORK PATH BENCHMARK ===")
for sc in NETWORK_PATHS:
    print(f"\n--- [RUNNING] {sc['name']} ---")
    reset_host()
    set_conditions(sc)
    
    pre_logs = set(glob.glob('/tmp/Moonlight-*.log'))
    cmd = [
        '/Applications/Moonlight.app/Contents/MacOS/Moonlight',
        'stream', sc['host'], 'Desktop',
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

with open("/tmp/network_path_matrix_results.json", "w") as f:
    json.dump(results, f, indent=2)

print("\n=== NETWORK PATH BENCHMARK COMPLETE ===")
