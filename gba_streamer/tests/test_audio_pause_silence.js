const assert = require('assert');

// Simulate the audio ring buffer logic
const GBA_SAMPLE_RATE = 32768;
const outSampleRate = 48000;
const bufferSize = 512;
const TARGET_CUSHION_SAMPLES = 1200;
const RING_SIZE = 65536;
const ringBufL = new Float32Array(RING_SIZE);
const ringBufR = new Float32Array(RING_SIZE);

// Fill with some audible sine wave
for (let i = 0; i < RING_SIZE; i++) {
    ringBufL[i] = Math.sin(2 * Math.PI * 440 * i / GBA_SAMPLE_RATE);
    ringBufR[i] = ringBufL[i];
}

let availableSamples = 1200;
let readPos = 0;
let isCalibrating = false;

function processAudioBlock() {
    const outL = new Float32Array(bufferSize);
    const outR = new Float32Array(bufferSize);

    if (isCalibrating) {
        return { outL, outR, available: 0 };
    }

    const baseRatio = GBA_SAMPLE_RATE / outSampleRate;
    const error = (availableSamples - TARGET_CUSHION_SAMPLES) / TARGET_CUSHION_SAMPLES;
    const speed = baseRatio * Math.max(0.97, Math.min(1.03, 1.0 + error * 0.04));

    for (let i = 0; i < bufferSize; i++) {
        if (availableSamples > 0) {
            const r = Math.floor(readPos) % RING_SIZE;
            outL[i] = ringBufL[r];
            outR[i] = ringBufR[r];
            readPos = (readPos + speed) % RING_SIZE;
            availableSamples -= speed;
            if (availableSamples < 0) availableSamples = 0;
        } else {
            outL[i] = 0;
            outR[i] = 0;
        }
    }
    return { outL, outR, available: availableSamples };
}

// TEST 1: Running audio drains available cushion
let block1 = processAudioBlock();
assert(block1.available < 1200, "Should consume samples");
assert(block1.outL.some(v => v !== 0), "Should output sound while samples available");
console.log("✅ TEST 1 PASSED: Running audio plays sound and drains cushion.");

// TEST 2: After drain, subsequent blocks MUST be 100% silent (never wrap or loop)
// Drain remaining samples
for (let i = 0; i < 10; i++) {
    processAudioBlock();
}
assert.strictEqual(availableSamples, 0, "availableSamples must be 0 after drain");

// Now run 10 blocks during pause (0 new samples incoming)
for (let i = 0; i < 10; i++) {
    const b = processAudioBlock();
    assert.strictEqual(b.available, 0, `available must stay 0, got ${b.available}`);
    const nonZeroL = b.outL.filter(v => v !== 0).length;
    const nonZeroR = b.outR.filter(v => v !== 0).length;
    assert.strictEqual(nonZeroL, 0, "outL must be completely silent during underrun/pause");
    assert.strictEqual(nonZeroR, 0, "outR must be completely silent during underrun/pause");
}
console.log("✅ TEST 2 PASSED: Zero samples output on pause/underrun (no repeating notes).");

// TEST 3: When isCalibrating is true, output is immediately silent even if cushion exists
availableSamples = 1200;
isCalibrating = true;
const calibBlock = processAudioBlock();
assert.strictEqual(calibBlock.outL.filter(v => v !== 0).length, 0, "Calibration mode must mute immediately");
console.log("✅ TEST 3 PASSED: Calibration mode immediately mutes audio.");

console.log("\n🎉 ALL AUDIO TESTS PASSED!");
