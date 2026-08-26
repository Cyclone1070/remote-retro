#!/usr/bin/env python3
import subprocess
import time
import json
import os
import glob
import plistlib
import xml.etree.ElementTree as ET

HOST_IP = '192.168.1.111'

def cleanup_all():
    print('=== AUTO-CLEANUP: PURGING LOCAL & REMOTE PROCESSES ===', flush=True)
    subprocess.run(['pkill', '-9', '-f', 'Moonlight'], stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
    remote_clean = 'sudo pkill -9 -x sunshine 2>/dev/null || true; sudo pkill -9 -x Xvfb 2>/dev/null || true; sudo pkill -9 -x Xorg 2>/dev/null || true; sudo rm -rf /tmp/sunshine.log /tmp/xvfb.log /home/cyc/.config/sunshine; sudo firewall-cmd --reload 2>/dev/null || true; sudo iw dev wlp0s20f3 set power_save on 2>/dev/null || true'
    subprocess.run(['ssh', f'cyc@{HOST_IP}', remote_clean], stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
    print('=== CLEANUP COMPLETE ===', flush=True)

try:
    print('=== 1. PREPARING AUTHENTICATION & CERTIFICATES ===', flush=True)
    plist_path = os.path.expanduser('~/Library/Preferences/com.moonlight-stream.Moonlight.plist')
    with open(plist_path, 'rb') as f:
        pl = plistlib.load(f)

    cert_str = pl['certificate'].decode('utf-8')

    server_info = subprocess.getoutput(f'curl -s http://{HOST_IP}:47989/serverinfo')
    root = ET.fromstring(server_info)
    uniqueid = root.find('uniqueid').text
    print('Sunshine Host UniqueID:', uniqueid, flush=True)

    # 2. Sync state to remote Sunshine
    state_json = {
        'root': {
            'uniqueid': uniqueid,
            'named_devices': [
                {
                    'name': 'macOS-Client',
                    'cert': cert_str,
                    'uuid': '0123456789ABCDEF',
                    'enabled': 'true'
                }
            ]
        }
    }
    with open('/tmp/sunshine_state.json', 'w') as f:
        json.dump(state_json, f, indent=2)
    subprocess.run(['scp', '/tmp/sunshine_state.json', f'cyc@{HOST_IP}:/home/cyc/.config/sunshine/sunshine_state.json'], check=True)
    
    # Restart Sunshine to load authenticated state
    subprocess.run(['ssh', f'cyc@{HOST_IP}', 'pkill -9 -x sunshine 2>/dev/null || true; DISPLAY=:99 nohup /usr/bin/sunshine /home/cyc/.config/sunshine/sunshine.conf > /tmp/sunshine.log 2>&1 &'])
    time.sleep(3)

    # 3. Sync server cert into local Moonlight plist
    srv_cert_pem = subprocess.getoutput(f'openssl s_client -connect {HOST_IP}:47984 -cert /tmp/client.crt -key /tmp/client.key </dev/null 2>/dev/null | openssl x509')
    srv_cert_bytes = srv_cert_pem.encode('utf-8')

    pl['hosts.1.address'] = HOST_IP
    pl['hosts.1.hostname'] = 'elitedesk'
    pl['hosts.1.uuid'] = uniqueid
    pl['hosts.1.srvcert'] = srv_cert_bytes
    pl['hosts.1.apps.1.name'] = 'Desktop'
    pl['hosts.1.apps.1.id'] = 881448767
    pl['hosts.1.apps.1.hdr'] = False
    pl['hosts.1.apps.1.hidden'] = False
    pl['hosts.1.apps.1.directlaunch'] = False
    pl['hosts.1.apps.1.appcollector'] = False
    pl['hosts.size'] = 1

    with open(plist_path, 'wb') as f:
        plistlib.dump(pl, f)

    print('=== 2. LAUNCHING REAL 1080p60 STREAM SESSION (8 SECONDS) ===', flush=True)
    before_logs = set(glob.glob('/tmp/Moonlight-*.log'))

    stream_cmd = ['/Applications/Moonlight.app/Contents/MacOS/Moonlight', 'stream', HOST_IP, 'Desktop', '--fps', '60', '--bitrate', '20000', '--width', '1920', '--height', '1080']
    stream_proc = subprocess.Popen(stream_cmd, stdout=subprocess.PIPE, stderr=subprocess.PIPE)
    time.sleep(8)
    stream_proc.terminate()
    try:
        stream_proc.wait(timeout=2)
    except:
        stream_proc.kill()

    print('=== 3. EXTRACTING REAL STREAM TELEMETRY FROM MOONLIGHT LOG ===', flush=True)
    after_logs = set(glob.glob('/tmp/Moonlight-*.log'))
    new_logs = list(after_logs - before_logs) or list(after_logs)
    new_logs.sort(key=lambda x: os.path.getmtime(x), reverse=True)

    if new_logs:
        latest_log = new_logs[0]
        print(f'Reading telemetry from {latest_log}:', flush=True)
        with open(latest_log, 'r') as f:
            lines = f.readlines()
        for l in lines:
            if any(k in l for k in ['Video stream', 'Average network latency', 'Average decode', 'Frames', 'packets', 'latency', 'RTT', 'Encoder', 'Audio stream', 'dropped', 'variance', 'FPS', 'Stream', 'Video bitrate', 'RTSP', 'Connected', 'Received frame', 'Loss rate']):
                print(l.strip(), flush=True)
    else:
        print('No new Moonlight log found.', flush=True)

    print('=== 4. EXTRACTING ENCODER TELEMETRY FROM SUNSHINE LOG ===', flush=True)
    sun_log = subprocess.getoutput(f'ssh cyc@{HOST_IP} "cat /tmp/sunshine.log | grep -a -i -E "encode|frame|fps|bitrate|vaapi|rtt|latency" | tail -30"')
    print(sun_log, flush=True)

finally:
    cleanup_all()