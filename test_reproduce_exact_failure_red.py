import subprocess
import time
import glob
import os
import sys

HOST_IP = '192.168.1.111'

def test_red_phase_audio_watchdog_failure():
    print('=== [RED PHASE] REPRODUCING CONNECTION TERMINATED ERROR CODE: -1 ===')
    t_start = time.time()
    t_str = time.strftime('%Y-%m-%d %H:%M:%S')
    print(f'[{t_str}] Starting Moonlight stream to trigger the 9-second audio watchdog timeout...')

    # Ensure no audio feed is running on elitedesk
    subprocess.run(['ssh', f'cyc@{HOST_IP}', 'pkill -9 -x pw-cat 2>/dev/null || true'])

    log_before = set(glob.glob('/tmp/Moonlight-*.log'))
    cmd = [
        '/Applications/Moonlight.app/Contents/MacOS/Moonlight',
        'stream', HOST_IP, 'Desktop',
        '--1080', '--fps', '60', '--bitrate', '20000',
        '--display-mode', 'windowed'
    ]
    proc = subprocess.Popen(cmd, stdout=subprocess.PIPE, stderr=subprocess.PIPE)

    # Monitor for the 9s watchdog disconnect
    watchdog_triggered = False
    for sec in range(1, 14):
        time.sleep(1)
        now_str = time.strftime('%Y-%m-%d %H:%M:%S')
        ret = proc.poll()
        if ret is not None:
            watchdog_triggered = True
            print(f'[{now_str}] >>> DISCONNECT DETECTED AT SECOND {sec} (Process exited with return code {ret})')
            break
        print(f'[{now_str}] Second {sec}/13: Streaming video...')

    if proc.poll() is None:
        proc.kill()

    log_after = set(glob.glob('/tmp/Moonlight-*.log'))
    new_logs = list(log_after - log_before) or sorted(list(log_after), key=os.path.getmtime, reverse=True)
    target_log = new_logs[0]
    print(f'Target Log File: {target_log}')

    with open(target_log) as f:
        log_lines = f.readlines()

    print('=== CRASH TRACE EVIDENCE ===')
    matched_lines = []
    for l in log_lines:
        if any(term in l for term in ['unexpected disconnect', 'Connection terminated', 'Error code', 'Transaction failed', 'No audio traffic']):
            print('  >>', l.strip())
            matched_lines.append(l.strip())

    assert len(matched_lines) > 0, 'Did not reproduce error!'
    assert any('Connection terminated: -1' in l for l in matched_lines), 'Connection terminated: -1 not found!'
    assert any('No audio traffic was ever received' in l for l in matched_lines), 'No audio traffic not found!'

    print(f'[RED PHASE CONFIRMED]')
    print(f'Successfully reproduced the exact Error Code: -1 failure caused by 9-second audio watchdog timeout.')
    sys.exit(1)

if __name__ == '__main__':
    test_red_phase_audio_watchdog_failure()