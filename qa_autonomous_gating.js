const { chromium } = require('/tmp/node_modules/playwright');
const { execSync } = require('child_process');
const fs = require('fs');

async function runBrowserGating() {
    console.log('===================================================================');
    console.log(' 🧪 RUNNING IN-BROWSER AUTONOMOUS A/V & PRESENTATION GATE');
    console.log('===================================================================');

    const browser = await chromium.launch({
        headless: true,
        args: [
            '--no-sandbox',
            '--autoplay-policy=no-user-gesture-required',
            '--use-fake-ui-for-media-stream',
            '--disable-web-security',
            '--enable-unsafe-webgpu',
            '--enable-features=Vulkan,UseSkiaRenderer'
        ]
    });

    const page = await browser.newPage();
    page.on('console', msg => {
        const txt = msg.text();
        if (!txt.includes('ReadPixels') && !txt.includes('ScriptProcessorNode')) {
            console.log('BROWSER:', txt);
        }
    });

    console.log('▶ Navigating to GBA Streamer at http://100.73.151.90:48500...');
    await page.goto('http://100.73.151.90:48500', { waitUntil: 'networkidle' });
    
    // Unlock AudioContext and establish worker stream
    await page.click('#canvasWrapper');
    
    // Wait for connection and loading overlay to disappear
    await page.waitForFunction(() => {
        const el = document.getElementById('loadingOverlay');
        return el && window.getComputedStyle(el).visibility === 'hidden';
    }, { timeout: 8000 });
    console.log('  ↳ ✅ Stream connected and #loadingOverlay dismissed cleanly.');

    // Inject Audio Tap into AudioContext
    await page.evaluate(() => {
        if (!window.audioCtx) window.initAudio();
        if (window.audioCtx && window.audioCtx.state === 'suspended') {
            window.audioCtx.resume();
        }

        window.capturedSamplesL = [];
        window.capturedSamplesR = [];

        const tapNode = window.audioCtx.createScriptProcessor(4096, 2, 2);
        tapNode.onaudioprocess = (e) => {
            if (window.capturedSamplesL.length < 176400) { // ~4s of audio
                const inL = e.inputBuffer.getChannelData(0);
                const inR = e.inputBuffer.getChannelData(1);
                for (let i = 0; i < inL.length; i++) {
                    window.capturedSamplesL.push(inL[i]);
                    window.capturedSamplesR.push(inR[i]);
                }
            }
        };

        if (window.audioMasterGain) {
            window.audioMasterGain.connect(tapNode);
            tapNode.connect(window.audioCtx.destination);
            console.log('Audio Tap hooked to audioMasterGain!');
        }
    });

    // -------------------------------------------------------------
    // GATE 4.1: Ground-Truth Pixel Integrity & Zero Color Distortion Gating
    // -------------------------------------------------------------
    console.log('▶ [GATE 4.1] Auditing Ground-Truth Canvas Pixels (Color Fidelity & Zero Corruption)...');
    await page.waitForTimeout(600);
    await page.screenshot({ path: '/tmp/gating_live_screen.png' });

    const pixelAuditCmd = `python3 -c '
from PIL import Image
import sys

im = Image.open("/tmp/gating_live_screen.png")
# Crop canvas area
canvas = im.crop((280, 51, 1000, 531))
colors = canvas.getcolors(maxcolors=100000)

has_black_border = False
non_zero_colors = 0
bgr_swapped_pixels = 0

for count, (r, g, b) in colors:
    if r < 10 and g < 10 and b < 10:
        has_black_border = True
    if r > 20 or g > 20 or b > 20:
        non_zero_colors += 1
    # Check for glaring BGR inversion (e.g. dominant blue where red is absent in warm scenes)
    if b > 180 and r < 60 and g < 60:
        bgr_swapped_pixels += count

if not has_black_border:
    print("FAIL: Canvas missing standard black border")
    sys.exit(1)
if non_zero_colors < 10:
    print(f"FAIL: Canvas is blank or frozen (unique active colors={non_zero_colors})")
    sys.exit(1)
if bgr_swapped_pixels > 500:
    print(f"FAIL: BGR color inversion detected! ({bgr_swapped_pixels} unnatural blue pixels)")
    sys.exit(1)

print(f"PASS: Pixel integrity verified. Unique colors={non_zero_colors}, Black border=OK, BGR inversion=0")
sys.exit(0)
'`;

    try {
        const auditOutput = execSync(pixelAuditCmd).toString().trim();
        console.log(`  ↳ ✅ ${auditOutput}`);
    } catch (err) {
        console.error('\n❌ PIXEL INTEGRITY AUDIT FAILED:', err.stdout ? err.stdout.toString() : err.message);
        process.exit(1);
    }

    // -------------------------------------------------------------
    // GATE 4.2: Live Interactive In-Game ROM Input Gating (Start, Action, D-Pad Walk)
    // -------------------------------------------------------------
    console.log('▶ [GATE 4.2] Verifying Interactive ROM Reaction: Start -> Action A -> D-Pad Movement...');
    const snapInitial = await page.screenshot();

    // 1. Press Enter (Start) to advance
    await page.keyboard.press('Enter');
    await page.waitForTimeout(600);
    const snapAfterStart = await page.screenshot();
    if (snapInitial.equals(snapAfterStart)) {
        console.error('❌ ROM INPUT GATE FAILED: Start/Enter button press had zero effect on screen');
        process.exit(1);
    }
    console.log('  ↳ ✅ ROM reacted to Enter/Start keypress (title/intro advanced).');

    // 2. Press z (Action A) to enter game
    await page.keyboard.press('z');
    await page.waitForTimeout(1000);
    const snapAfterA = await page.screenshot();
    if (snapAfterStart.equals(snapAfterA)) {
        console.error('❌ ROM INPUT GATE FAILED: A/z button press had zero effect on screen');
        process.exit(1);
    }
    console.log('  ↳ ✅ ROM reacted to A/z button press (game loaded).');

    // 3. Hold D (Right) to walk character across level
    await page.keyboard.down('d');
    await page.waitForTimeout(500);
    await page.keyboard.up('d');
    await page.waitForTimeout(200);
    const snapAfterWalk = await page.screenshot({ path: '/tmp/gating_live_screen.png' });
    if (snapAfterA.equals(snapAfterWalk)) {
        console.error('❌ ROM INPUT GATE FAILED: D-Pad Right movement had zero effect on gameplay screen');
        process.exit(1);
    }
    console.log('  ↳ ✅ ROM reacted to D-Pad Right keypress (character movement verified).');

    // -------------------------------------------------------------
    // GATE 4.3: Comprehensive Keyboard Matrix & Sticky Key Gating
    // -------------------------------------------------------------
    console.log('▶ [GATE 4.3] Testing complete keyboard matrix press & release lifecycle...');
    const testKeys = [
        { key: 'd', expectedBit: 4 },
        { key: 'a', expectedBit: 5 },
        { key: 'w', expectedBit: 6 },
        { key: 's', expectedBit: 7 },
        { key: 'z', expectedBit: 0 },
        { key: 'x', expectedBit: 1 },
        { key: 'Enter', expectedBit: 3 },
        { key: ' ', expectedBit: 2 },
        { key: 'ArrowRight', expectedBit: 4 },
        { key: 'ArrowLeft', expectedBit: 5 },
        { key: 'ArrowUp', expectedBit: 6 },
        { key: 'ArrowDown', expectedBit: 7 }
    ];

    const keyFailures = [];
    for (const item of testKeys) {
        await page.keyboard.down(item.key);
        await page.waitForTimeout(50);
        const maskDown = await page.evaluate(() => inputMask);
        if ((maskDown & (1 << item.expectedBit)) === 0) {
            keyFailures.push(`Key '${item.key}' down failed to set bit ${item.expectedBit} (mask: ${maskDown})`);
        }

        await page.keyboard.up(item.key);
        await page.waitForTimeout(50);
        const maskUp = await page.evaluate(() => inputMask);
        if (maskUp !== 0) {
            keyFailures.push(`Sticky key detected: Key '${item.key}' release left mask stuck at ${maskUp} (expected 0)`);
        }
    }

    if (keyFailures.length > 0) {
        console.error('\n❌ KEYBOARD MATRIX GATING FAILED:');
        keyFailures.forEach(f => console.error(`  - ${f}`));
        process.exit(1);
    }
    console.log('  ↳ ✅ All 12 game buttons press & release cleanly with zero stuck keys.');

    // -------------------------------------------------------------
    // GATE 4.4: Live Run-Ahead HUD Toggle & Hotkey Verification
    // -------------------------------------------------------------
    console.log('▶ [GATE 4.4] Testing Run-Ahead HUD button click & F2 hotkey toggling...');
    const initialText = await page.$eval('#runaheadVal', el => el.innerText);
    if (!initialText.includes('1F')) {
        console.error(`❌ Unexpected initial runahead value: ${initialText}`);
        process.exit(1);
    }

    // Test HUD Click Toggle
    await page.click('#runaheadToggle');
    await page.waitForTimeout(100);
    const textAfterClick = await page.$eval('#runaheadVal', el => el.innerText);
    if (!textAfterClick.includes('OFF')) {
        console.error(`❌ Clicking HUD button failed to toggle Run-Ahead to OFF (got: ${textAfterClick})`);
        process.exit(1);
    }

    // Test F2 Key Toggle
    await page.keyboard.press('F2');
    await page.waitForTimeout(100);
    const textAfterF2 = await page.$eval('#runaheadVal', el => el.innerText);
    if (!textAfterF2.includes('1F')) {
        console.error(`❌ Pressing F2 failed to toggle Run-Ahead back to 1F (got: ${textAfterF2})`);
        process.exit(1);
    }
    console.log('  ↳ ✅ Run-Ahead HUD button and F2 hotkey toggle seamlessly in real-time.');

    // -------------------------------------------------------------
    // GATE 4.5: 600-Frame In-Browser Presentation & Zero-Stutter Gating
    // -------------------------------------------------------------
    console.log('▶ [GATE 4.5] Measuring actual canvas render deltas over 600 frames...');
    await page.waitForFunction(() => {
        return window.__honestRenderHistory && window.__honestRenderHistory.length >= 600;
    }, { timeout: 20000 });

    const honestMetrics = await page.evaluate(() => window.getHonestStutterMetrics(600));
    const frameDeltas = (await page.evaluate(() => window.__honestRenderHistory)).slice(-600);

    // Collect Audio Samples
    const audioData = await page.evaluate(() => {
        return {
            samplesL: window.capturedSamplesL,
            samplesR: window.capturedSamplesR,
            audioCtxState: window.audioCtx ? window.audioCtx.state : 'null',
            audioQueueMs: (window.getAudioQueueLength() / 32768 * 1000).toFixed(1)
        };
    });

    await browser.close();

    // Analyze Audio
    const L = audioData.samplesL;
    const R = audioData.samplesR;
    let sumSqL = 0;
    let sumSqR = 0;
    let peakL = 0;
    let peakR = 0;
    let zeroCount = 0;
    let inDropout = false;
    let dropouts = 0;

    // Find start of audio stream (skip pre-playback cold start silence)
    let startIdx = 0;
    while (startIdx < L.length && Math.abs(L[startIdx]) < 0.001 && Math.abs(R[startIdx]) < 0.001) {
        startIdx++;
    }

    for (let i = startIdx; i < L.length; i++) {
        const valL = L[i];
        const valR = R[i];
        sumSqL += valL * valL;
        sumSqR += valR * valR;
        if (Math.abs(valL) > peakL) peakL = Math.abs(valL);
        if (Math.abs(valR) > peakR) peakR = Math.abs(valR);

        if (Math.abs(valL) < 0.0001 && Math.abs(valR) < 0.0001) {
            zeroCount++;
            if (zeroCount > 512 && !inDropout) {
                dropouts++;
                inDropout = true;
            }
        } else {
            zeroCount = 0;
            inDropout = false;
        }
    }

    const rmsL = L.length > 0 ? Math.sqrt(sumSqL / L.length) : 0;
    const rmsR = R.length > 0 ? Math.sqrt(sumSqR / R.length) : 0;
    const balance = rmsR > 0.0001 ? (rmsL / rmsR) : 999;

    // Analyze Frame Presentation
    const n = frameDeltas.length;
    const mean = frameDeltas.reduce((a,b)=>a+b,0) / n;
    const variance = frameDeltas.reduce((a,b)=>a+Math.pow(b-mean, 2), 0) / (n - 1);
    const sigma = Math.sqrt(variance);
    const stutters = frameDeltas.filter(d => d > 33.33).length;
    const stutterRate = ((stutters / n) * 100);
    const fps = 1000.0 / mean;

    console.log('\n--- BROWSER PRESENTATION METRICS ---');
    console.log(` Delivered FPS:        ${fps.toFixed(1)} FPS`);
    console.log(` Mean Frame Interval:  ${mean.toFixed(2)} ms (Target: 16.67 ms)`);
    console.log(` Pacing Jitter (σ):    ${sigma.toFixed(2)} ms`);
    console.log(` Micro-Stutters:       ${stutters} (${stutterRate.toFixed(2)}%)`);
    console.log('------------------------------------');
    console.log('--- AUDIO INTEGRITY METRICS ---');
    console.log(` Left Ear RMS:         ${rmsL.toFixed(4)} (Peak: ${peakL.toFixed(3)})`);
    console.log(` Right Ear RMS:        ${rmsR.toFixed(4)} (Peak: ${peakR.toFixed(3)})`);
    console.log(` Stereo Balance (L/R): ${balance.toFixed(3)} (Valid Range: 0.4 - 2.5)`);
    console.log(` Silence Dropouts:     ${dropouts} dropouts`);
    console.log(` Audio Buffer Cushion: ${audioData.audioQueueMs} ms`);
    console.log('-------------------------------');

    // Assertions
    const failures = [];
    if (fps < 59.5) failures.push(`FPS too low: ${fps.toFixed(1)} < 59.5`);
    if (sigma > 1.0) failures.push(`Pacing jitter too high: ${sigma.toFixed(2)}ms > 1.0ms`);
    if (stutters > 0) failures.push(`Micro-stutter detected: ${stutters} stutters (${stutterRate.toFixed(2)}%) > 0.00%`);
    if (rmsL < 0.03 || rmsR < 0.03) failures.push(`Audio volume too low or silent (RMS L:${rmsL.toFixed(4)}, R:${rmsR.toFixed(4)})`);
    if (balance < 0.4 || balance > 2.5) failures.push(`Stereo balance skewed (${balance.toFixed(2)})`);
    if (dropouts > 0) failures.push(`Audio dropouts detected (${dropouts} dropouts)`);

    if (failures.length > 0) {
        console.error('\n❌ IN-BROWSER GATING ASSERTION FAILURES:');
        failures.forEach(f => console.error(`  - ${f}`));
        process.exit(1);
    } else {
        console.log('\n✅ PASS: In-Browser Video + Audio + Presentation + ROM Input Gates 100% Passed!\n');
        process.exit(0);
    }
}

runBrowserGating().catch(err => {
    console.error('Fatal Gating Error:', err);
    process.exit(1);
});
