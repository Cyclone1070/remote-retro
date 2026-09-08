import asyncio
import websockets
import sys

WS_URI = "ws://100.73.151.90:48500/ws"

async def test_calibration_protocol():
    print("[1] Connecting to server...")
    async with websockets.connect(WS_URI) as ws:
        # Drain initial frames
        await asyncio.sleep(0.3)
        while True:
            try:
                await asyncio.wait_for(ws.recv(), timeout=0.1)
            except asyncio.TimeoutError:
                break
        print("[2] Connected and drained initial stream.")

        # 1. Send Pause command [0xAA, 0x50, 1]
        print("[3] Pausing emulation...")
        await ws.send(bytes([0xAA, 0x50, 1]))
        await asyncio.sleep(0.2)
        while True:
            try:
                await asyncio.wait_for(ws.recv(), timeout=0.1)
            except asyncio.TimeoutError:
                break
        print("    Emulation confirmed paused (0 frames incoming).")

        # 2. Simulate user pressing Arrow keys (regular input packets while paused)
        # Server MUST NOT produce any frames!
        print("[4] Sending regular input mask (D-Pad Right) while paused...")
        input_pkt = bytes([1, 1 << 4, 0])
        await ws.send(input_pkt)
        try:
            extra = await asyncio.wait_for(ws.recv(), timeout=0.3)
            print(f"❌ Failure: Server emitted unexpected frame during pause upon input mask! len={len(extra)}")
            sys.exit(1)
        except asyncio.TimeoutError:
            print("    ✅ Passed: 0 frames produced when regular input is pressed while paused.")

        # 3. Send Step command [0xAA, 0x53, 0x10, 0x00] (Step 1 frame with Right pressed)
        print("[5] Sending 1-frame step command [0xAA, 0x53, 0x10, 0x00]...")
        await ws.send(bytes([0xAA, 0x53, 0x10, 0x00]))
        step1_frame = await asyncio.wait_for(ws.recv(), timeout=1.0)
        assert len(step1_frame) > 4, "Expected valid video frame packet on step 1"
        print(f"    ✅ Passed: Exactly 1 frame received on Step 1 (len={len(step1_frame)})")

        # Verify no extra trailing frames arrive
        try:
            extra = await asyncio.wait_for(ws.recv(), timeout=0.2)
            print(f"❌ Failure: Unexpected extra frame received after step 1! len={len(extra)}")
            sys.exit(1)
        except asyncio.TimeoutError:
            print("    ✅ Passed: Emulation stopped immediately after 1 stepped frame.")

        # 4. Send Step command 2
        print("[6] Sending 1-frame step command 2...")
        await ws.send(bytes([0xAA, 0x53, 0x10, 0x00]))
        step2_frame = await asyncio.wait_for(ws.recv(), timeout=1.0)
        assert len(step2_frame) > 4, "Expected valid video frame packet on step 2"
        print(f"    ✅ Passed: Exactly 1 frame received on Step 2 (len={len(step2_frame)})")

        # 5. Send Rewind command [0xAA, 0x50, 2]
        print("[7] Sending Rewind command [0xAA, 0x50, 2]...")
        await ws.send(bytes([0xAA, 0x50, 2]))
        rewind_frame = await asyncio.wait_for(ws.recv(), timeout=1.0)
        assert len(rewind_frame) > 4, "Expected valid video frame packet on rewind"
        print(f"    ✅ Passed: Reset frame received on rewind (len={len(rewind_frame)})")

        # 6. Unpause [0xAA, 0x50, 0]
        print("[8] Unpausing...")
        await ws.send(bytes([0xAA, 0x50, 0]))
        resumed_frame = await asyncio.wait_for(ws.recv(), timeout=1.0)
        assert len(resumed_frame) > 4, "Expected valid video frame packet after unpause"
        print(f"    ✅ Passed: Resumed normal streaming (len={len(resumed_frame)})")

    print("\n🎉 ALL CALIBRATION PROTOCOL CHECKS PASSED!")

if __name__ == "__main__":
    asyncio.run(test_calibration_protocol())
