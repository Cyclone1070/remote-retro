import subprocess
import time
import re
import os
import glob
import json

HOST_IP = "192.168.1.111"
DEV = "wlp0s20f3"

SCENARIOS = [
    {
        "id": "1_baseline_hevc_hw",
        "name": "Baseline (HEVC, HW Metal, 20 Mbps, Static)",
        "codec": "HEVC",
        "decoder": "hardware",
        "bitrate": "20000",
        "motion": False,
        "net_delay": None,
        "net_loss": None
    },
    {
        "id": "2_baseline_h264_hw",
        "name": "Baseline (H.264, HW Metal, 20 Mbps, Static)",
        "codec": "H.264",
        "decoder": "hardware",
        "bitrate": "20000",
        "motion": False,
        "net_delay": None,
        "net_loss": None
    },
    {
        "id": "3_baseline_hevc_sw",
        "name": "Software Decoder (HEVC, SW, 20 Mbps, Static)",
        "codec": "HEVC",
        "decoder": "software",
        "bitrate": "20000",
        "motion": False,
        "net_delay": None,
        "net_loss": None
    },
    {
        "id": "4_high_bitrate_50m",
        "name": "High Bitrate (HEVC, HW Metal, 50 Mbps, Static)",
        "codec": "HEVC",
        "decoder": "hardware",
        "bitrate": "50000",
        "motion": False,
        "net_delay": None,
        "net_loss": None
    },
    {
        "id": "5_low_bitrate_5m",
        "name": "Low Bitrate (HEVC, HW Metal, 5 Mbps, Static)",
        "codec": "HEVC",
        "decoder": "hardware",
        "bitrate": "5000",
        "motion": False,
        "net_delay": None,
        "net_loss": None
    },
    {
        "id": "6_high_motion_glxgears",
        "name": "High Motion / Load (HEVC, HW Metal, 20 Mbps, glxgears)",
        "codec": "HEVC",
        "decoder": "hardware",
        "bitrate": "20000",
        "motion": True,
        "net_delay": None,
        "net_loss": None
    },
    {
        "id": "7_simulated_jitter_delay",
        "name": "Network Delay & Jitter (+15ms ± 3ms delay)",
        "codec": "HEVC",
        "decoder": "hardware",
        "bitrate": "20000",
        "motion": False,
        "net_delay": "15ms 3ms",
        "net_loss": None
    },
    {
        "id": "8_simulated_packet_loss",
        "name": "Network Packet Loss (2% loss)",
        "codec": "HEVC",
        "decoder": "hardware",
        "bitrate": "20000",
        "motion": False,
        "net_delay": None,
        "net_loss": "2%"
    },
    {
        "id": "9_congested_loss_delay_motion",
        "name": "Congested (15ms delay + 2% loss + High Motion)",
        "codec": "HEVC",
        "decoder": "hardware",
        "bitrate": "20000",
        "motion": True,
        "net_delay": "15ms 3ms",
        "net_loss": "2%"
    }
]

def clean_host():
    subprocess.run(["ssh", f"cyc@{HOST_IP}", f"sudo tc qdisc del dev {DEV} root 2>/dev/null || true; pkill -9 -x glxgears 2>/dev/null || true; systemctl --user restart ephemeral-sunshine 2>/dev/null || true"], check=False)
    time.sleep(2)

def set_host_conditions(scenario):
    net_cmds = []
    if scenario.get("net_delay") and scenario.get("net_loss"):
        net_cmds.append(f"sudo tc qdisc add dev {DEV} root netem delay {scenario['net_delay']} loss {scenario['net_loss']}")
    elif scenario.get("net_delay"):
        net_cmds.append(f"sudo tc qdisc add dev {DEV} root netem delay {scenario['net_delay']}")
    elif scenario.get("net_loss"):
        net_cmds.append(f"sudo tc qdisc add dev {DEV} root netem loss {scenario['net_loss']}")
    
    if scenario.get("motion"):
        net_cmds.append("DISPLAY=:99 nohup glxgears -geometry 800x600+500+200 >/dev/null 2>&1 &")
    
    if net_cmds:
        full_cmd = "; ".join(net_cmds)
        subprocess.run(["ssh", f"cyc@{HOST_IP}", full_cmd], check=False)
        time.sleep(1)

def parse_moonlight_log(log_path):
    with open(log_path, 'r', errors='ignore') as f:
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
    
    if all(k in res for k in ['host_avg', 'net_avg', 'dec_avg', 'queue_avg', 'rend_avg']):
        res['total_e2e_avg'] = round(res['host_avg'] + res['net_avg'] + res['dec_avg'] + res['queue_avg'] + res['rend_avg'], 2)
    return res

results = []

print("=== STARTING COMPREHENSIVE STREAM BENCHMARK MATRIX ===")
for sc in SCENARIOS:
    print(f"\n--- Running Scenario: {sc['name']} ---")
    clean_host()
    set_host_conditions(sc)
    
    # Identify pre-existing logs
    pre_logs = set(glob.glob('/tmp/Moonlight-*.log'))
    
    cmd = [
        '/Applications/Moonlight.app/Contents/MacOS/Moonlight',
        'stream', HOST_IP, 'Desktop',
        '--1080', '--fps', '60',
        '--bitrate', sc['bitrate'],
        '--video-codec', sc['codec'],
        '--video-decoder', sc['decoder'],
        '--display-mode', 'windowed'
    ]
    
    subprocess.run(['pkill', '-9', '-f', 'Moonlight'])
    time.sleep(1)
    
    start_time = time.time()
    proc = subprocess.Popen(cmd)
    
    # Run stream for 14 seconds
    time.sleep(14)
    
    proc.terminate()
    try:
        proc.wait(timeout=4)
    except:
        proc.kill()
        
    time.sleep(1)
    post_logs = set(glob.glob('/tmp/Moonlight-*.log'))
    new_logs = list(post_logs - pre_logs)
    target_log = new_logs[0] if new_logs else (max(post_logs, key=os.path.getctime) if post_logs else None)
    
    stats = {}
    if target_log:
        stats = parse_moonlight_log(target_log)
        print(f"Log: {target_log} -> Stats: {stats}")
    
    results.append({
        "scenario": sc,
        "stats": stats,
        "log_path": target_log
    })

clean_host()

with open("/tmp/benchmark_results.json", "w") as f:
    json.dump(results, f, indent=2)

print("\n=== BENCHMARK SUITE COMPLETE ===")
