import asyncio
import json
import subprocess
import urllib.request
import websockets
import sys
import os
import hashlib

CHROME_BIN = "/Applications/Google Chrome.app/Contents/MacOS/Google Chrome"
URL = "http://100.73.151.90:48500"
DEBUG_PORT = 9225

async def send_cdp(ws, method, params=None, req_id=[0]):
    req_id[0] += 1
    msg = {"id": req_id[0], "method": method, "params": params or {}}
    await ws.send(json.dumps(msg))
    while True:
        resp = json.loads(await ws.recv())
        if resp.get("id") == req_id[0]:
            return resp.get("result", {})

async def eval_js(ws, expr):
    res = await send_cdp(ws, "Runtime.evaluate", {"expression": expr, "returnByValue": True})
    return res.get("result", {}).get("value")

async def key_event(ws, evt_type, key, code, vk):
    await send_cdp(ws, "Input.dispatchKeyEvent", {
        "type": evt_type,
        "key": key,
        "code": code,
        "windowsVirtualKeyCode": vk,
        "nativeVirtualKeyCode": vk,
    })

async def press_key(ws, key, code, vk):
    await key_event(ws, "rawKeyDown", key, code, vk)
    await asyncio.sleep(0.05)
    await key_event(ws, "keyUp", key, code, vk)
    await asyncio.sleep(0.05)

async def capture_canvas_screenshot(ws):
    rect = await eval_js(ws, """(() => {
        const r = document.getElementById('gbaCanvas').getBoundingClientRect();
        return { x: r.x, y: r.y, width: r.width, height: r.height, scale: 1 };
    })()""")
    scr = await send_cdp(ws, "Page.captureScreenshot", {"clip": rect})
    return hashlib.sha256(scr["data"].encode()).hexdigest()

async def main():
    chrome_proc = subprocess.Popen([
        CHROME_BIN,
        "--headless=new",
        f"--remote-debugging-port={DEBUG_PORT}",
        "--no-first-run",
        "--no-default-browser-check",
        "--use-gl=angle",
        URL
    ], stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)

    try:
        # Find websocket debugger target with retry
        targets = None
        for _ in range(20):
            try:
                targets = json.loads(urllib.request.urlopen(f"http://127.0.0.1:{DEBUG_PORT}/json/list").read())
                if targets:
                    break
            except Exception:
                await asyncio.sleep(0.2)
        assert targets, "Failed to connect to Chrome debugging port"
        page_target = [t for t in targets if t.get("type") == "page"][0]
        ws_url = page_target["webSocketDebuggerUrl"]

        async with websockets.connect(ws_url) as ws:
            print("[E2E] Connected to Headless Chrome via CDP.")
            await send_cdp(ws, "Page.enable")
            await send_cdp(ws, "Runtime.enable")

            # 1. Verify loading overlay disappears automatically (Bug 1 Proof)
            print("[E2E 1] Waiting for stream to connect and #loadingOverlay to hide...")
            loaded = False
            for i in range(40):
                overlay_opacity = await eval_js(ws, "document.getElementById('loadingOverlay').style.opacity")
                overlay_vis = await eval_js(ws, "document.getElementById('loadingOverlay').style.visibility")
                overlay_text = await eval_js(ws, "document.getElementById('loadingOverlay').innerText")
                if i % 10 == 0:
                    print(f"    [Polling] opacity={overlay_opacity}, vis={overlay_vis}, text={overlay_text}")
                if overlay_opacity == "0" or overlay_vis == "hidden":
                    loaded = True
                    break
                await asyncio.sleep(0.2)
            if not loaded:
                print("    [Failed overlay inspection]:", overlay_text)
            assert loaded, "❌ E2E Failed: #loadingOverlay never disappeared! Stream is stuck on connect."
            print("    ✅ Passed: Stream connected and loading spinner hidden automatically without pressing P!")

            await eval_js(ws, "window.initAudio()")
            await asyncio.sleep(1.0) # Let game run for 1s with audio

            # 2. Press 'P' to pause & enter calibration mode
            print("[E2E 2] Pressing 'P' to pause and open calibration dock...")
            await press_key(ws, "p", "KeyP", 80)
            await asyncio.sleep(0.5)

            is_calib = await eval_js(ws, "isCalibrating")
            assert is_calib is True, f"❌ E2E Failed: isCalibrating is {is_calib}"
            cnt0 = await eval_js(ws, "advanceCount")
            assert cnt0 == 0, f"❌ E2E Failed: initial advanceCount is {cnt0}"
            print("    ✅ Passed: Calibration mode active, advanceCount is 0.")

            # Capture paused canvas screenshot
            hash1 = await capture_canvas_screenshot(ws)
            print(f"    [Baseline Canvas SHA256]: {hash1[:16]}")

            # 3. Press ArrowRight to register test input (Walk Right)
            print("[E2E 3] Pressing 'ArrowRight' to register Right Walk test input...")
            await press_key(ws, "ArrowRight", "ArrowRight", 39)
            await asyncio.sleep(0.5)

            cnt_after_arrow = await eval_js(ws, "advanceCount")
            assert cnt_after_arrow == 0, f"❌ E2E Failed: advanceCount changed to {cnt_after_arrow} on arrow press!"
            pill_text = await eval_js(ws, "document.getElementById('currentTestPill').innerText")
            assert "Right" in pill_text, f"❌ Pill text unexpected: {pill_text}"

            # Capture canvas screenshot after arrow press
            hash2 = await capture_canvas_screenshot(ws)
            print(f"    [After Arrow Canvas SHA256]: {hash2[:16]}")

            # Check canvas equality (Bug 2 Proof: ZERO automatic advance)
            assert hash1 == hash2, "❌ E2E Failed: Canvas altered or advanced when pressing arrow key!"
            print("    ✅ Passed: Canvas is 100% BIT-EXACT after pressing Arrow key. Zero frame advance!")

            # 4. Check Audio Mute during pause
            print("[E2E 4] Verifying audio is muted and queue is drained...")
            audio_gain = await eval_js(ws, "window.audioMasterGain ? window.audioMasterGain.gain.value : 0")
            audio_q = await eval_js(ws, "window.getAudioQueueLength()")
            assert audio_gain == 0, f"❌ Audio gain expected 0, got {audio_gain}"
            assert audio_q == 0, f"❌ Audio queue length expected 0, got {audio_q}"
            print("    ✅ Passed: Audio is 100% muted and queue length is 0 (no looping notes).")

            # 5. Press 'Enter' to step 1st frame forward
            print("[E2E 5] Pressing 'Enter' to step 1st frame forward...")
            await press_key(ws, "Enter", "Enter", 13)
            await asyncio.sleep(0.5)

            cnt1 = await eval_js(ws, "advanceCount")
            assert cnt1 == 1, f"❌ E2E Failed: advanceCount is {cnt1}, expected 1"
            guide_text = await eval_js(ws, "document.getElementById('guideBox').innerText")
            print(f"    Guidebox text: '{guide_text}'")
            assert "1st advance" in guide_text and "0F Lag" in guide_text, f"❌ Unexpected guidebox text: {guide_text}"
            print("    ✅ Passed: Frame stepped by 1. Guide correctly suggests 0F Lag for 1st advance!")

            # Capture stepped canvas screenshot
            hash3 = await capture_canvas_screenshot(ws)

            # 6. Change input key to 'Z' (Jump) - canvas MUST NOT reset to previous state
            print("[E2E 6] Changing input key to 'Z' (Jump) - verifying canvas does NOT reset...")
            await press_key(ws, "z", "KeyZ", 90)
            await asyncio.sleep(0.3)

            cnt_after_z = await eval_js(ws, "advanceCount")
            assert cnt_after_z == 1, f"❌ E2E Failed: advanceCount reset to {cnt_after_z} on input change!"
            pill_after_z = await eval_js(ws, "document.getElementById('currentTestPill').innerText")
            assert "Jump" in pill_after_z, f"❌ Pill text unexpected: {pill_after_z}"

            hash4 = await capture_canvas_screenshot(ws)
            assert hash3 == hash4, "❌ E2E Failed: Changing input key reset the canvas to previous state!"
            print("    ✅ Passed: Changing input key preserved canvas frame with zero screen jump/reset!")

            # 7. Press 'R' to explicitly reset back to paused baseline
            print("[E2E 7] Pressing 'R' to explicitly reset back to paused baseline...")
            await press_key(ws, "r", "KeyR", 82)
            await asyncio.sleep(0.5)

            cnt_after_r = await eval_js(ws, "advanceCount")
            assert cnt_after_r == 0, f"❌ E2E Failed: advanceCount after R is {cnt_after_r}"
            hash5 = await capture_canvas_screenshot(ws)
            print(f"    [Hashes] hash1={hash1[:16]} hash2={hash2[:16]} hash3={hash3[:16]} hash4={hash4[:16]} hash5={hash5[:16]}")
            assert hash5 == hash1, f"❌ E2E Failed: Explicit reset did not restore baseline paused frame! (hash1={hash1[:16]} hash5={hash5[:16]})"
            print("    ✅ Passed: Explicit Reset (R) cleanly rewound canvas to initial checkpoint!")

            # 8. Press 'Space' to apply calibration & resume
            print("[E2E 8] Pressing 'Space' to apply calibration & resume...")
            await press_key(ws, " ", "Space", 32)
            await asyncio.sleep(0.5)

            is_calib_resumed = await eval_js(ws, "isCalibrating")
            assert is_calib_resumed is False, "❌ E2E Failed: Calibration did not close on Space"
            audio_gain_resumed = await eval_js(ws, "window.audioMasterGain ? window.audioMasterGain.gain.value : 0")
            assert audio_gain_resumed == 1.0, f"❌ Audio gain expected 1.0 on resume, got {audio_gain_resumed}"
            print("    ✅ Passed: Emulation resumed cleanly with audio unmuted.")

            print("\n🎉 COMPLETE END-TO-END BROWSER QA PASSED!")

    finally:
        chrome_proc.terminate()
        chrome_proc.wait()

if __name__ == "__main__":
    asyncio.run(main())
