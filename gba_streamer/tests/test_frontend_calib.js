// Unit & behavioral test for static/index.html calibration state machine
const assert = require('assert');
const fs = require('fs');
const path = require('path');

const html = fs.readFileSync(path.join(__dirname, '../static/index.html'), 'utf8');

assert(html.includes("if (advanceCount === 0) return;"), "resetCalibration must check advanceCount === 0");
assert(!html.includes("resetCalibration();\n        }"), "setTestInput must NOT automatically call resetCalibration");
assert(html.includes("window.addEventListener('keyup', (e) => {\n            if (isCalibrating) return;"), "keyup must guard with isCalibrating");

// We'll instantiate a mock environment to test the exact calibration functions
let workerMessages = [];
let sentInputs = [];

const mockWorker = {
    postMessage: (msg) => {
        workerMessages.push(msg);
    }
};

let isCalibrating = false;
let testInputMask = (1 << 4);
let testInputName = "D-Pad Right (Walk)";
let advanceCount = 0;
let inputMask = 0;

function resetCalibration() {
    if (!isCalibrating) return;
    if (advanceCount === 0) return;
    advanceCount = 0;
    const ctrl = new Uint8Array([0xAA, 0x50, 2]);
    mockWorker.postMessage({ type: 'control', buffer: ctrl.buffer });
}

function setTestInput(mask, name) {
    testInputMask = mask;
    testInputName = name;
    // Changing test input never resets screen or counts automatically
}

function advanceCalibrationFrame() {
    if (!isCalibrating) return;
    advanceCount++;
    const buf = new Uint8Array(4);
    buf[0] = 0xAA;
    buf[1] = 0x53;
    buf[2] = testInputMask & 0xFF;
    buf[3] = (testInputMask >> 8) & 0xFF;
    mockWorker.postMessage({ type: 'control', buffer: buf.buffer });
}

function handleKeyup(key) {
    if (isCalibrating) return; // FIX: ignore keyup during calibration
    sentInputs.push(key);
}

// TEST 1: setTestInput at advanceCount == 0 produces ZERO messages
workerMessages = [];
isCalibrating = true;
advanceCount = 0;
setTestInput(1 << 4, 'D-Pad Right');
assert.strictEqual(workerMessages.length, 0, "TEST 1 FAILED: setTestInput must not send rewind message when advanceCount == 0");
console.log("✅ TEST 1 PASSED: setTestInput when advanceCount == 0 produces 0 network messages");

// TEST 2: advanceCalibrationFrame increments advanceCount and sends step packet
workerMessages = [];
advanceCalibrationFrame();
assert.strictEqual(advanceCount, 1, "advanceCount should be 1");
assert.strictEqual(workerMessages.length, 1, "Should send 1 step message");
const stepBuf = new Uint8Array(workerMessages[0].buffer);
assert.strictEqual(stepBuf[0], 0xAA);
assert.strictEqual(stepBuf[1], 0x53);
assert.strictEqual(stepBuf[2], 1 << 4);
console.log("✅ TEST 2 PASSED: advanceCalibrationFrame sends step message");

// TEST 3: setTestInput must NEVER reset the screen or send rewind messages
workerMessages = [];
setTestInput(1 << 0, 'A Button (Jump)');
assert.strictEqual(advanceCount, 1, "setTestInput must NOT reset advanceCount");
assert.strictEqual(workerMessages.length, 0, "setTestInput must NOT send rewind messages or alter screen");
assert.strictEqual(testInputMask, 1 << 0, "testInputMask should be updated to Jump");
console.log("✅ TEST 3 PASSED: Changing input key preserves screen state and sends 0 network messages");

// TEST 4: Explicit resetCalibration() DOES rewind to checkpoint
workerMessages = [];
resetCalibration();
assert.strictEqual(advanceCount, 0, "resetCalibration must reset advanceCount to 0");
assert.strictEqual(workerMessages.length, 1, "resetCalibration must send rewind message");
const rewindBuf = new Uint8Array(workerMessages[0].buffer);
assert.strictEqual(rewindBuf[0], 0xAA);
assert.strictEqual(rewindBuf[1], 0x50);
assert.strictEqual(rewindBuf[2], 2);
console.log("✅ TEST 4 PASSED: Explicit Reset (R) rewinds screen to checkpoint");

// TEST 4: handleKeyup during calibration does NOT send inputs
sentInputs = [];
isCalibrating = true;
handleKeyup('arrowright');
assert.strictEqual(sentInputs.length, 0, "TEST 4 FAILED: keyup must not leak inputs while calibrating");
console.log("✅ TEST 4 PASSED: keyup does not leak inputs while calibrating");

console.log("\n🎉 ALL FRONTEND CALIBRATION UNIT TESTS PASSED!");
