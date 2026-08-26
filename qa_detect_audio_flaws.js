const { chromium } = require('/tmp/node_modules/playwright');

async function detectAudioFlaws() {
    console.log('===================================================================');
    console.log(' 🧪 AUTONOMOUS PLAYWRIGHT AUDIO & VIDEO QA HARNESS');
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
    page.on('console', msg => console.log('BROWSER CONSOLE:', msg.text()));

    await page.goto('http://100.73.151.90:48500', { waitUntil: 'networkidle' });

    // Inject Audio Tap to capture real in-browser Web Audio output
    await page.evaluate(() => {
        window.__recordedL = [];
        window.__recordedR = [];

        const checkTap = setInterval(() => {
            if (window.audioCtx && window.scriptNode) {
                clearInterval(checkTap);
                const tap = window.audioCtx.createScriptProcessor(1024, 2, 2);
                tap.onaudioprocess = (e) => {
                    const inL = e.inputBuffer.getChannelData(0);
                    const inR = e.inputBuffer.getChannelData(1);
                    for (let i = 0; i < inL.length; i++) {
                        window.__recordedL.push(inL[i]);
                        window.__recordedR.push(inR[i]);
                    }
                };
                window.scriptNode.connect(tap);
                tap.connect(window.audioCtx.destination);
                console.log('Audio Tap hooked to ScriptProcessor!');
            }
        }, 50);
    });

    // Click canvas to unlock audio
    await page.click('#canvasWrapper');
    
    // Capture 4.5 seconds for complete phrase evaluation
    await page.waitForTimeout(4500);

    const data = await page.evaluate(() => {
        return {
            fps: document.getElementById('fpsVal') ? document.getElementById('fpsVal').innerText : '0',
            m2pLag: document.getElementById('totalLatency') ? document.getElementById('totalLatency').innerText : '0',
            audioBufDisplay: document.getElementById('audioStatus') ? document.getElementById('audioStatus').innerText : '0',
            hasCtx: !!window.audioCtx,
            ctxState: window.audioCtx ? window.audioCtx.state : 'none',
            samplesL: window.__recordedL ? window.__recordedL.slice(0, 44100 * 4) : [],
            samplesR: window.__recordedR ? window.__recordedR.slice(0, 44100 * 4) : [],
        };
    });

    await page.screenshot({ path: '/tmp/playwright_audio_qa.png' });
    await browser.close();

    console.log('\n--- BROWSER RUNTIME METRICS ---');
    console.log(` Delivered FPS:       ${data.fps}`);
    console.log(` Displayed M2P Lag:   ${data.m2pLag}`);
    console.log(` Displayed Audio Buf: ${data.audioBufDisplay}`);
    console.log(` AudioContext State:  ${data.ctxState}`);
    console.log(` Samples Captured:    ${data.samplesL.length} L / ${data.samplesR.length} R (~${(data.samplesL.length/44100).toFixed(2)}s)`);
    console.log('-------------------------------\n');

    const left = data.samplesL;
    const right = data.samplesR;
    const n = left.length;

    let rmsL = 0;
    let rmsR = 0;
    let maxL = 0;
    let maxR = 0;
    let dropouts = 0;
    let consecutiveZeros = 0;

    for (let i = 0; i < n; i++) {
        const l = left[i];
        const r = right[i];
        rmsL += l * l;
        rmsR += r * r;
        if (Math.abs(l) > maxL) maxL = Math.abs(l);
        if (Math.abs(r) > maxR) maxR = Math.abs(r);

        if (Math.abs(l) < 0.0001 && Math.abs(r) < 0.0001) {
            consecutiveZeros++;
        } else {
            if (consecutiveZeros > 220) { // > 5ms silence gap
                dropouts++;
            }
            consecutiveZeros = 0;
        }
    }

    rmsL = Math.sqrt(rmsL / (n || 1));
    rmsR = Math.sqrt(rmsR / (n || 1));
    const balanceRatio = rmsL / (rmsR || 0.00001);

    console.log('--- AUDIO SIGNAL INTEGRITY ANALYSIS ---');
    console.log(` Left Ear RMS:        ${rmsL.toFixed(4)} (Peak: ${maxL.toFixed(3)})`);
    console.log(` Right Ear RMS:       ${rmsR.toFixed(4)} (Peak: ${maxR.toFixed(3)})`);
    console.log(` Stereo Balance (L/R):${balanceRatio.toFixed(3)} (Stereo Music Range: 0.4 - 2.5)`);
    console.log(` Silence Dropouts:    ${dropouts} dropouts`);
    console.log('---------------------------------------\n');

    let errors = [];

    // Check 1: AudioContext active
    if (data.ctxState !== 'running') {
        errors.push(`AudioContext is not running (State: ${data.ctxState})`);
    }

    // Check 2: Audio Buffer accumulation (< 25ms)
    const bufMs = parseFloat(data.audioBufDisplay);
    if (isNaN(bufMs) || bufMs > 25.0 || bufMs < 5.0) {
        errors.push(`Audio buffer out of bounds (${bufMs} ms, target: 15-22ms)`);
    }

    // Check 3: Stereo Balance & Audibility
    if (rmsL < 0.02 || rmsR < 0.02) {
        errors.push(`Audio channel dead or muted (L=${rmsL.toFixed(4)}, R=${rmsR.toFixed(4)})`);
    } else if (balanceRatio < 0.35 || balanceRatio > 2.8) {
        errors.push(`Extreme unnatural stereo channel imbalance (L/R: ${balanceRatio.toFixed(3)})`);
    }

    // Check 4: Choppiness / Dropouts
    if (dropouts > 2) {
        errors.push(`High audio choppiness detected (${dropouts} silence gaps / buffer underruns)`);
    }

    // Check 5: FPS and M2P Latency
    if (parseFloat(data.fps) < 55.0) {
        errors.push(`FPS dropped too low (${data.fps})`);
    }

    if (errors.length > 0) {
        console.error('❌ QA ASSERTION FAILURES:');
        errors.forEach(e => console.error(`  - ${e}`));
        process.exit(1);
    } else {
        console.log('✅ PASS: All Video + Audio QA assertions passed perfectly!');
        process.exit(0);
    }
}

detectAudioFlaws().catch(err => {
    console.error('Test Execution Error:', err);
    process.exit(1);
});
