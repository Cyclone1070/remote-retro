import subprocess
import time
import glob
import os
import xml.etree.ElementTree as ET

HOST_IP = '192.168.1.111'

def test_genuine_uninterrupted_stream():
    print('=== STEP 1: Verify Host Server State ===')
    res = subprocess.getoutput(f'curl -s http://{HOST_IP}:47989/serverinfo')
    root = ET.fromstring(res)
    state = root.find('state').text
    print(f'Server State: {state}')
    assert state == 'SUNSHINE_SERVER_FREE', f'Expected SUNSHINE_SERVER_FREE, got {state}'

    print('=== STEP 2: Launch Real Moonlight Stream Session (20s Test) ===')
    log_before = set(glob.glob('/tmp/Moonlight-*.log'))
    cmd = [
        '/Applications/Moonlight.app/Contents/MacOS/Moonlight',
        'stream', HOST_IP, 'Desktop',
        '--1080', '--fps', '60', '--bitrate', '20000',
        '--display-mode', 'windowed'
    ]
    proc = subprocess.Popen(cmd, stdout=subprocess.PIPE, stderr=subprocess.PIPE)

    # Poll every 500ms for 20 seconds
    for half_sec in range(1, 41):
        time.sleep(0.5)
        ret = proc.poll()
        if ret is not None:
            # Moonlight died prematurely!
            log_after = set(glob.glob('/tmp/Moonlight-*.log'))
            new_logs = list(log_after - log_before) or sorted(list(log_after), key=os.path.getmtime, reverse=True)
            with open(new_logs[0]) as f:
                print('--- CRASH LOG DUMP ---')
                print(''.join(f.readlines()[-30:]))
            assert False, f'FAIL: Moonlight crashed / terminated prematurely at second {half_sec/2:.1f}s with exit code {ret}!'
        if half_sec % 4 == 0:
            print(f'Stream actively running at {half_sec/2:.0f}/20 seconds...')

    print('=== STEP 3: Request Clean Shutdown after Full 20s Window ===')
    proc.terminate()
    try:
        proc.wait(timeout=4)
    except:
        proc.kill()

    log_after = set(glob.glob('/tmp/Moonlight-*.log'))
    new_logs = list(log_after - log_before) or sorted(list(log_after), key=os.path.getmtime, reverse=True)
    target_log = new_logs[0]
    print(f'Target Log: {target_log}')
    with open(target_log) as f:
        content = f.read()

    print('=== STEP 4: Assert Pipeline Invariants ===')
    assert 'Using Metal renderer with hardware decoding' in content, 'FAIL: Metal hardware decode not active'
    assert 'Output frame with POC' in content, 'FAIL: No video frames decoded'
    print('PASS: Metal hardware decode and active frame stream verified.')
    print('SUCCESS: Stream sustained uninterrupted for full 20 seconds!')

if __name__ == '__main__':
    test_genuine_uninterrupted_stream()