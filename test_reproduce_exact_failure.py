import subprocess
import time
import glob
import os
import sys

HOST_IP = '192.168.1.111'

def reproduce_exact_error_minus_1():
    print('=== RED PHASE: REPRODUCING EXACT CONNECTION TERMINATED: -1 BUG ===')
    log_before = set(glob.glob('/tmp/Moonlight-*.log'))
    cmd = [
        '/Applications/Moonlight.app/Contents/MacOS/Moonlight',
        'stream', HOST_IP, 'Desktop',
        '--1080', '--fps', '60', '--bitrate', '20000',
        '--display-mode', 'windowed'
    ]
    
    t_str = time.strftime('%Y-%m-%d %H:%M:%S')
    print(f'[{t_str}] Launching Moonlight GUI stream against {HOST_IP}...')
    proc = subprocess.Popen(cmd, stdout=subprocess.PIPE, stderr=subprocess.PIPE)

    # Monitor until it crashes (should happen at ~9-10s)
    terminated_early = False
    exit_code = None
    
    for sec in range(1, 16):
        time.sleep(1)
        ret = proc.poll()
        now_str = time.strftime('%Y-%m-%d %H:%M:%S')
        if ret is not None:
            terminated_early = True
            exit_code = ret
            print(f'[{now_str}] REPRODUCED: Moonlight process terminated prematurely at second {sec} with return code {ret}!')
            break
        print(f'[{now_str}] Second {sec}/15: stream still running...')

    if proc.poll() is None:
        proc.kill()

    log_after = set(glob.glob('/tmp/Moonlight-*.log'))
    new_logs = list(log_after - log_before) or sorted(list(log_after), key=os.path.getmtime, reverse=True)
    target_log = new_logs[0]
    print(f'Target Log: {target_log}')
    
    with open(target_log) as f:
        log_content = f.read()

    print('=== CRASH LOG EVIDENCE DUMP ===')
    for line in log_content.splitlines():
        if any(term in line for term in ['unexpected disconnect', 'Connection terminated', 'Error code', 'Transaction failed', 'No audio traffic']):
            print(' >>', line)

    if 'Connection terminated: -1' in log_content:
        print('[RED PHASE CONFIRMED] Successfully reproduced exact error: Connection terminated Error code: -1')
        sys.exit(1)
    else:
        print('Did not reproduce error!')
        sys.exit(0)

if __name__ == '__main__':
    reproduce_exact_error_minus_1()