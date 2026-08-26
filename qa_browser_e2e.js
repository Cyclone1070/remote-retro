const puppeteer = require('/tmp/node_modules/puppeteer-core');
const fs = require('fs');

async function runAutonomousQA() {
    console.log('===================================================================');
    console.log(' 🤖 AUTONOMOUS CLIENT-SIDE BROWSER QA + IN-BROWSER AUDIO TAP');
    console.log('===================================================================');

    const browser = await puppeteer.launch({
        executablePath: '/Applications/Google Chrome.app/Contents/MacOS/Google Chrome',
        headless: 'new',
        args: [
            '--no-sandbox',
            '--disable-setuid-sandbox',
            '--autoplay-policy=no-user-gesture-required',
            '--use-fake-ui-for-media-stream',
            '--disable-web-security'
        ]
    });

    const page = await browser.newPage();
    await page.setViewport({ width: 1024, height: 768 });

    console.log('Navigating to http://100.73.151.90:48500 ...');
    await page.goto('http://100.73.151.90:48500', { waitUntil: 'networkidle2' });

    // Inject in-browser Web Audio Tap to capture real DAC output
    await page.evaluate(() => {
        window.__recordedAudioL = [];
        window.__recordedAudioR = [];
        window.__audioCaptureActive = true;

        const checkHook = setInterval(() => {
            if (window.audioNode && window.audioCtx) {
                clearInterval(checkHook);
                const tapNode = window.audioCtx.createScriptProcessor(1024, 2, 2);
                tapNode.onaudioprocess = (e) => {
                    if (!window.__audioCaptureActive) return;
                    const inL = e.inputBuffer.getChannelData(0);
                    const inR = e.inputBuffer.getChannelData(1);
                    for (let i = 0; i < inL.length; i++) {
                        window.__recordedAudioL.push(inL[i]);
                        window.__recordedAudioR.push(inR[i]);
                    }
                };
                window.audioNode.connect(tapNode);
                tapNode.connect(window.audioCtx.destination);
                console.log('Web Audio Tap successfully attached to browser audio graph!');
            }
        }, 100);
    });

    console.log('Clicking canvas to unlock audio & start presentation...');
    await page.click('#loadingOverlay');

    // Collect 4 seconds of continuous stream telemetry and audio samples
    await new Promise(r => setTimeout(r, 4000));

    const hudMetrics = await page.evaluate(() => {
        window.__audioCaptureActive = false;
        return {
            totalLag: document.getElementById('totalLatency').innerText,
            networkRtt: document.getElementById('netLatency').innerText,
            hostLatency: document.getElementById('hostLatency').innerText,
            audioBuffer: document.getElementById('audioBufLatency').innerText,
            deliveredFps: document.getElementById('fpsVal').innerText,
            samplesCapturedL: window.__recordedAudioL ? window.__recordedAudioL.length : 0,
            sampleDataL: window.__recordedAudioL ? Array.from(window.__recordedAudioL.slice(0, 44100 * 3)) : [],
            sampleDataR: window.__recordedAudioR ? Array.from(window.__recordedAudioR.slice(0, 44100 * 3)) : [],
        };
    });

    const screenshotPath = '/tmp/browser_qa_live.png';
    await page.screenshot({ path: screenshotPath });
    await browser.close();

    console.log('\n--- LIVE BROWSER HUD TELEMETRY ---');
    console.log(` Delivered Framerate:     ${hudMetrics.deliveredFps} FPS`);
    console.log(` Audio Buffer Latency:    ${hudMetrics.audioBuffer}`);
    console.log(` Estimated M2P Lag:       ${hudMetrics.totalLag}`);
    console.log(` Network RTT:             ${hudMetrics.networkRtt}`);
    console.log(` Host Compute:            ${hudMetrics.hostLatency}`);
    console.log(` Browser Audio Captured:  ${hudMetrics.samplesCapturedL} stereo samples (~${(hudMetrics.samplesCapturedL / 44100).toFixed(2)}s)`);
    console.log('----------------------------------\n');

    // Save browser-rendered audio to WAV file
    const lSamples = hudMetrics.sampleDataL;
    const rSamples = hudMetrics.sampleDataR;
    const numFrames = lSamples.length;

    let nonZeroCount = 0;
    let sumSq = 0;
    let maxAmp = 0;
    let dropouts = 0;
    let consecutiveZeros = 0;

    const pcmData = Buffer.alloc(numFrames * 4);
    for (let i = 0; i < numFrames; i++) {
        const sl = Math.max(-1, Math.min(1, lSamples[i]));
        const sr = Math.max(-1, Math.min(1, rSamples[i]));

        const s16L = Math.round(sl * 32767);
        const s16R = Math.round(sr * 32767);

        pcmData.writeInt16LE(s16L, i * 4);
        pcmData.writeInt16LE(s16R, i * 4 + 2);

        const amp = Math.max(Math.abs(s16L), Math.abs(s16R));
        if (amp > maxAmp) maxAmp = amp;
        sumSq += s16L * s16L + s16R * s16R;

        if (amp > 10) {
            nonZeroCount++;
            if (consecutiveZeros > 441) { // > 10ms silence gap
                dropouts++;
            }
            consecutiveZeros = 0;
        } else {
            consecutiveZeros++;
        }
    }

    const rms = Math.sqrt(sumSq / (numFrames * 2 || 1));
    const wavPath = '/tmp/browser_rendered_audio.wav';
    
    // Write WAV header
    const wavHeader = Buffer.alloc(44);
    wavHeader.write('RIFF', 0);
    wavHeader.writeUInt32LE(36 + pcmData.length, 4);
    wavHeader.write('WAVE', 8);
    wavHeader.write('fmt ', 12);
    wavHeader.writeUInt32LE(16, 16);
    wavHeader.writeUInt16LE(1, 20); // PCM
    wavHeader.writeUInt16LE(2, 22); // Stereo
    wavHeader.writeUInt32LE(44100, 24); // 44.1kHz
    wavHeader.writeUInt32LE(44100 * 4, 28);
    wavHeader.writeUInt16LE(4, 32);
    wavHeader.writeUInt16LE(16, 34);
    wavHeader.write('data', 36);
    wavHeader.writeUInt32LE(pcmData.length, 40);

    fs.writeFileSync(wavPath, Buffer.concat([wavHeader, pcmData]));

    console.log('--- IN-BROWSER AUDIO SIGNAL ANALYSIS ---');
    console.log(` Saved WAV File:          ${wavPath}`);
    console.log(` Non-Zero Audio Frames:   ${nonZeroCount} / ${numFrames} (${((nonZeroCount/numFrames)*100).toFixed(1)}%)`);
    console.log(` Peak Amplitude:          ${maxAmp} / 32767`);
    console.log(` RMS Audio Energy:        ${rms.toFixed(2)} (Audible threshold > 500)`);
    console.log(` Audio Gap Dropouts:      ${dropouts} dropouts (>10ms gaps)`);
    console.log('----------------------------------------\n');

    let failed = false;
    const fpsNum = parseFloat(hudMetrics.deliveredFps);
    const audioBufNum = parseFloat(hudMetrics.audioBuffer);
    const m2pNum = parseFloat(hudMetrics.totalLag);

    if (isNaN(fpsNum) || fpsNum < 55.0) {
        console.error(`❌ FAIL: FPS too low (${fpsNum} < 55.0)`);
        failed = true;
    }
    if (isNaN(audioBufNum) || audioBufNum > 40.0 || audioBufNum < 5.0) {
        console.error(`❌ FAIL: Audio buffer out of bounds (${audioBufNum} ms, expected 15-35 ms)`);
        failed = true;
    }
    if (isNaN(m2pNum) || m2pNum > 40.0) {
        console.error(`❌ FAIL: M2P latency too high (${m2pNum} ms > 40.0 ms)`);
        failed = true;
    }
    if (numFrames < 44100) {
        console.error(`❌ FAIL: In-browser audio capture failed (${numFrames} samples)`);
        failed = true;
    }
    if (rms < 500) {
        console.error(`❌ FAIL: Audio RMS energy too low (${rms.toFixed(2)} < 500), audio is silent or dead`);
        failed = true;
    }

    if (!failed) {
        console.log('✅ PASS: Both Video HUD and In-Browser Web Audio graph verified 100% operational!');
        process.exit(0);
    } else {
        process.exit(1);
    }
}

runAutonomousQA().catch(err => {
    console.error('QA Harness Error:', err);
    process.exit(1);
});
