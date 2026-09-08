import asyncio
import websockets
import sys

WS_URI = "ws://100.73.151.90:48500/ws"

async def test_unpause_on_new_connection():
    print("[1] Connecting client 1...")
    async with websockets.connect(WS_URI) as ws1:
        # Pause emulation
        print("[1] Sending Pause command [0xAA, 0x50, 1]...")
        await ws1.send(bytes([0xAA, 0x50, 1]))
        await asyncio.sleep(0.2)
    print("[1] Client 1 closed.")

    print("[2] Connecting client 2 (expecting immediate live video packets)...")
    async with websockets.connect(WS_URI) as ws2:
        try:
            msg = await asyncio.wait_for(ws2.recv(), timeout=1.5)
            assert isinstance(msg, bytes) and len(msg) > 4, f"Expected video packet, got {type(msg)} len={len(msg) if isinstance(msg, bytes) else 0}"
            print(f"✅ Success: Client 2 received live video frame: len={len(msg)} bytes")
        except asyncio.TimeoutError:
            print("❌ Failure: Client 2 timed out waiting for frames (server stuck in paused state)")
            sys.exit(1)

if __name__ == "__main__":
    asyncio.run(test_unpause_on_new_connection())
