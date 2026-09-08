const test = require('node:test');
const assert = require('node:assert');
const { PacingQueue } = require('../gba_streamer/static/pacing_queue.js');

test('Smooth mode: frame is not popped before target cushion time', () => {
    const queue = new PacingQueue({ mode: 'smooth', cushionMs: 16.666 });
    const arrivalTime = 1000.0;
    const dummyFrame = new Uint8Array([1, 2, 3]);

    queue.push(dummyFrame, arrivalTime);

    // At 1010ms (10ms after arrival, before 16.666ms cushion)
    const earlyPop = queue.pop(1010.0);
    assert.strictEqual(earlyPop, null, 'Should not pop before target cushion time');

    // At 1017ms (after 16.666ms cushion)
    const readyPop = queue.pop(1017.0);
    assert.notStrictEqual(readyPop, null, 'Should pop once target cushion time reached');
    assert.deepStrictEqual(readyPop.bytes, dummyFrame);
    assert.strictEqual(readyPop.blit, true);
});

test('Smooth mode: catches up when >= 2 frames backlogged and caps queue depth <= 3', () => {
    const queue = new PacingQueue({ mode: 'smooth', cushionMs: 16.666, maxBacklog: 3 });
    const f1 = new Uint8Array([1]);
    const f2 = new Uint8Array([2]);
    const f3 = new Uint8Array([3]);
    const f4 = new Uint8Array([4]);

    queue.push(f1, 2000.0);
    queue.push(f2, 2000.0);
    queue.push(f3, 2000.0);
    queue.push(f4, 2000.0);

    assert.strictEqual(queue.size(), 3, 'Queue should cap at maxBacklog 3');

    const catchupPop = queue.pop(2005.0);
    assert.notStrictEqual(catchupPop, null, 'Should catch up immediately when backlogged');
    assert.deepStrictEqual(catchupPop.bytes, f2, 'Oldest surviving frame should be popped');
});

test('Direct mode: immediately pops frames with 0 cushion and supports dynamic mode toggle', () => {
    const queue = new PacingQueue({ mode: 'smooth', cushionMs: 16.666 });
    const f1 = new Uint8Array([10]);

    // Push in smooth mode at 3000ms
    queue.push(f1, 3000.0);
    // At 3002ms, smooth mode holds it
    assert.strictEqual(queue.pop(3002.0), null);

    // Toggle to direct mode
    queue.setMode('direct');
    assert.strictEqual(queue.getMode(), 'direct');

    // In direct mode at 3002ms, should pop immediately (0 cushion)
    const directPop = queue.pop(3002.0);
    assert.notStrictEqual(directPop, null, 'Direct mode should pop immediately');
    assert.deepStrictEqual(directPop.bytes, f1);
    assert.strictEqual(directPop.blit, true);
});

test('Smooth mode absorbs +/- 3ms Wi-Fi jitter across 60 frames without starving 60Hz display ticks', () => {
    const queue = new PacingQueue({ mode: 'smooth', cushionMs: 16.666, maxBacklog: 3 });
    const frameInterval = 16.666;
    let deliveredCount = 0;
    let starvedCount = 0;

    // Simulate 60 frames sent by host at 16.666ms intervals, arriving with +/- 3ms network jitter
    const jitters = [
        0, 2.5, -2.0, 1.8, -2.5, 3.0, -1.5, 0.5, -2.8, 2.2,
        -1.0, 1.5, -2.0, 2.8, -3.0, 0.0, 1.2, -1.8, 2.1, -2.4,
        0.8, -1.2, 2.6, -2.9, 1.1, -0.9, 2.3, -2.7, 1.9, -1.4,
        0.0, 2.5, -2.0, 1.8, -2.5, 3.0, -1.5, 0.5, -2.8, 2.2,
        -1.0, 1.5, -2.0, 2.8, -3.0, 0.0, 1.2, -1.8, 2.1, -2.4,
        0.8, -1.2, 2.6, -2.9, 1.1, -0.9, 2.3, -2.7, 1.9, -1.4
    ];

    let currentNetworkTime = 0;
    let frameIdx = 0;

    // Simulation runs over 60 display ticks (0 to 60 * 16.666 ms)
    for (let tick = 0; tick < 60; tick++) {
        const displayTime = tick * frameInterval;

        // Push any network frames that have arrived by displayTime
        while (frameIdx < 60) {
            const nominalSendTime = frameIdx * frameInterval;
            const arrivalTime = nominalSendTime + 4.0 + jitters[frameIdx]; // 4ms base LAN RTT/2 + jitter
            if (arrivalTime <= displayTime) {
                queue.push(new Uint8Array([frameIdx]), arrivalTime);
                frameIdx++;
            } else {
                break;
            }
        }

        // After initial cushion warm-up (tick >= 2, ~33ms)
        if (tick >= 2) {
            const frame = queue.pop(displayTime);
            if (frame) {
                deliveredCount++;
            } else {
                starvedCount++;
            }
        }
    }

    assert.strictEqual(starvedCount, 0, `Should have 0 starved display ticks, but had ${starvedCount}`);
    assert.strictEqual(deliveredCount, 58, `Should have delivered all 58 post-warmup frames, delivered ${deliveredCount}`);
});
