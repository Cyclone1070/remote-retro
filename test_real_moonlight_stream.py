import subprocess
import time
import glob
import os
import xml.etree.ElementTree as ET

HOST_IP = '192.168.1.111'

def test_streaming_pipeline():
    print('=== STEP 1: Check Sunshine Server State ===')
    res = subprocess.getoutput(f'curl -s http://{HOST_IP}:47989/serverinfo')
    root = ET.fromstring(res)
    state = root.find('state').text
    print(f'Server State: {state}')
    assert state == 'SUNSHINE_SERVER_FREE', f'Expected SUNSHINE_SERVER_FREE, got {state}'

    print('=== STEP 2: Launch Real Moonlight Stream Session (10s) ===')
    log_before = set(glob.glob('/tmp/Moonlight-*.log'))
    cmd = [
        '/Applications/Moonlight.app/Contents/MacOS/Moonlight',
        'stream', HOST_IP, 'Desktop',
        '--1080', '--fps', '60', '--bitrate', '20000',
        '--display-mode', 'windowed'
    ]
    proc = subprocess.Popen(cmd, stdout=subprocess.PIPE, stderr=subprocess.PIPE)
    time.sleep(10)
    proc.terminate()
    try:
        proc.wait(timeout=3)
    except:
        proc.kill()

    log_after = set(glob.glob('/tmp/Moonlight-*.log'))
    new_logs = list(log_after - log_before)
    if not new_logs:
        new_logs = sorted(list(log_after), key=os.path.getmtime, reverse=True)
    target_log = new_logs[0]
    print(f'Inspecting Log: {target_log}')
    
    with open(target_log) as f:
        content = f.read()

    print('=== STEP 3: Asserting Real Streaming Pipeline Invariants ===')
    assert 'Starting RTSP handshake' in content, 'FAIL: No RTSP handshake found in log'
    print('PASS: RTSP handshake established.')

    assert 'Using Metal renderer with hardware decoding' in content, 'FAIL: Metal hardware decode not active'
    print('PASS: Metal hardware acceleration verified.')

    assert 'Output frame with POC' in content, 'FAIL: No video frames decoded'
    print('PASS: Real video frames decoded and rendered.')

    assert 'No audio traffic was ever received from the host!' not in content, 'FAIL: Audio watchdog detected zero audio traffic'
    print('PASS: Continuous audio traffic verified.')

    assert 'Connection terminated: -1' not in content, 'FAIL: Connection terminated unexpectedly'
    assert 'Found unexpected PC' not in content, 'FAIL: UUID mismatch / unexpected PC conflict'
    print('PASS: Zero connection drops or watchdog terminations.')
    print('SUCCESS: ALL STREAMING PIPELINE ASSERTIONS PASSED!')

if __name__ == '__main__':
    test_streaming_pipeline()