const { chromium } = require('/tmp/node_modules/playwright');

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
            '--disable-web-security'
        ]
    });

    const page = await browser.newPage();
    page.on('console', msg => {
        const txt = msg.text();
        if (!txt.includes('ReadPixels') && !txt.includes('ScriptProcessorNode')) {
            console.log('BROWSER:', txt);
        }
    });

    await page.goto('http://100.73.151.90:48500', { waitUntil: 'networkidle' });
    
    // Unlock AudioContext and establish worker stream
    await page.click('#canvasWrapper');
    await page.keyboard.press('Enter');
    await page.waitForTimeout(2000);

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
    // GATE 4.1: Comprehensive Keyboard Matrix & Sticky Key Gating
    // -------------------------------------------------------------
    console.log('  Testing complete keyboard matrix press & release lifecycle...');
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
    // GATE 4.2: Live Run-Ahead HUD Toggle & Hotkey Verification
    // -------------------------------------------------------------
    console.log('  Testing Run-Ahead HUD button click & F2 hotkey toggling...');
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

    // Measure in-browser presentation deltas over 600 frames
    const presentationPromise = page.evaluate(() => {
        return new Promise((resolve) => {
            const deltas = [];
            let last = performance.now();
            let count = 0;
            function onFrame() {
                const now = performance.now();
                deltas.push(now - last);
                last = now;
                count++;
                if (count < 600) {
                    requestAnimationFrame(onFrame);
                } else {
                    resolve(deltas.slice(30)); // skip warm-up
                }
            }
            requestAnimationFrame(onFrame);
        });
    });

    await page.waitForTimeout(4500);
    const frameDeltas = await presentationPromise;

    // Collect Audio Samples
    const audioData = await page.evaluate(() => {
        return {
            samplesL: window.capturedSamplesL,
            samplesR: window.capturedSamplesR,
            audioCtxState: window.audioCtx ? window.audioCtx.state : 'null',
            audioQueueMs: (window.getAudioQueueLength() / 32768 * 1000).toFixed(1)
        };
    });

    await page.screenshot({ path: '/tmp/gating_live_screen.png' });
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

    for (let i = 0; i < L.length; i++) {
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
    const fps = 1000.0 / mean;

    console.log('\n--- BROWSER PRESENTATION METRICS ---');
    console.log(` Delivered FPS:        ${fps.toFixed(1)} FPS`);
    console.log(` Mean Frame Interval:  ${mean.toFixed(2)} ms (Target: 16.67 ms)`);
    console.log(` Pacing Jitter (σ):    ${sigma.toFixed(2)} ms`);
    console.log(` Micro-Stutters:       ${stutters} (${((stutters/n)*100).toFixed(2)}%)`);
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
    if (fps < 59.0) failures.push(`FPS too low: ${fps.toFixed(1)} < 59.0`);
    if (sigma > 2.0) failures.push(`Pacing jitter too high: ${sigma.toFixed(2)}ms > 2.0ms`);
    if (rmsL < 0.03 || rmsR < 0.03) failures.push(`Audio volume too low or silent (RMS L:${rmsL.toFixed(4)}, R:${rmsR.toFixed(4)})`);
    if (balance < 0.4 || balance > 2.5) failures.push(`Stereo balance skewed (${balance.toFixed(2)})`);
    if (dropouts > 3) failures.push(`Audio dropouts too high (${dropouts})`);

    if (failures.length > 0) {
        console.error('\n❌ IN-BROWSER GATING ASSERTION FAILURES:');
        failures.forEach(f => console.error(`  - ${f}`));
        process.exit(1);
    } else {
        console.log('\n✅ PASS: In-Browser Video + Audio + Presentation Gates 100% Passed!\n');
        process.exit(0);
    }
}

runBrowserGating().catch(err => {
    console.error('Fatal Gating Error:', err);
    process.exit(1);
});
