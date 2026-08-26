import subprocess
import time
import glob
import os
import xml.etree.ElementTree as ET

HOST_IP = '192.168.1.111'

def test_stream_endurance():
    print('=== STEP 1: Verify Sunshine Host Readiness ===')
    res = subprocess.getoutput(f'curl -s http://{HOST_IP}:47989/serverinfo')
    root = ET.fromstring(res)
    state = root.find('state').text
    print(f'Server State: {state}')
    assert state == 'SUNSHINE_SERVER_FREE', f'Expected SUNSHINE_SERVER_FREE, got {state}'

    print('=== STEP 2: Launch Real Moonlight Stream Session (15s Endurance Test) ===')
    log_before = set(glob.glob('/tmp/Moonlight-*.log'))
    cmd = [
        '/Applications/Moonlight.app/Contents/MacOS/Moonlight',
        'stream', HOST_IP, 'Desktop',
        '--1080', '--fps', '60', '--bitrate', '20000',
        '--display-mode', 'windowed'
    ]
    proc = subprocess.Popen(cmd, stdout=subprocess.PIPE, stderr=subprocess.PIPE)

    # Monitor stream health every second for 15 seconds
    for sec in range(1, 16):
        time.sleep(1)
        ret = proc.poll()
        assert ret is None, f'CRASH: Moonlight process exited unexpectedly at second {sec} with return code {ret}'
        print(f'Stream alive and active at second {sec}/15...')

    print('=== STEP 3: Request Graceful Stream Shutdown ===')
    proc.terminate()
    try:
        proc.wait(timeout=4)
    except:
        proc.kill()

    log_after = set(glob.glob('/tmp/Moonlight-*.log'))
    new_logs = list(log_after - log_before)
    if not new_logs:
        new_logs = sorted(list(log_after), key=os.path.getmtime, reverse=True)
    target_log = new_logs[0]
    print(f'Target Log: {target_log}')

    with open(target_log) as f:
        content = f.read()

    print('=== STEP 4: Asserting Real Video Pipeline Invariants ===')
    assert 'Starting RTSP handshake' in content, 'FAIL: No RTSP handshake found'
    print('PASS: RTSP handshake established.')

    assert 'Using Metal renderer with hardware decoding' in content, 'FAIL: Metal hardware decode not active'
    print('PASS: Metal hardware acceleration verified.')

    assert 'Output frame with POC' in content, 'FAIL: No video frames decoded'
    print('PASS: Real video frames decoded and rendered.')

    assert 'Connection terminated: -1' not in content, 'FAIL: Connection terminated unexpectedly with -1'
    assert 'Found unexpected PC' not in content, 'FAIL: UUID mismatch / unexpected PC conflict'
    print('PASS: Zero crash events or unexpected disconnects.')
    print('SUCCESS: ALL STREAMING PIPELINE ASSERTIONS PASSED!')

if __name__ == '__main__':
    test_stream_endurance()