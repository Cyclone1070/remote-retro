const { chromium } = require('/tmp/node_modules/playwright');

async function benchmarkCloudRetroAudio() {
    console.log('--- Probing CloudRetro WebRTC Audio (http://100.73.151.90:8000) ---');
    const browser = await chromium.launch({
        headless: true,
        args: ['--no-sandbox', '--autoplay-policy=no-user-gesture-required', '--use-fake-ui-for-media-stream']
    });
    const page = await browser.newPage();

    await page.goto('http://100.73.151.90:8000', { waitUntil: 'networkidle' });
    
    // CloudRetro requires clicking game selection / start
    try {
        await page.waitForSelector('.game-item, #game-screen, canvas, button', { timeout: 5000 });
        const startBtn = await page.$('button, .game-item');
        if (startBtn) await startBtn.click();
    } catch(e) {}

    await page.waitForTimeout(4000);

    // Extract WebRTC Audio Stats via RTCPeerConnection getStats()
    const stats = await page.evaluate(async () => {
        let audioStats = null;
        if (window.pc) {
            const report = await window.pc.getStats();
            report.forEach(stat => {
                if (stat.type === 'inbound-rtp' && stat.kind === 'audio') {
                    audioStats = {
                        bytesReceived: stat.bytesReceived,
                        packetsReceived: stat.packetsReceived,
                        packetsLost: stat.packetsLost,
                        jitter: stat.jitter * 1000,
                        jitterBufferDelay: (stat.jitterBufferDelay / Math.max(1, stat.jitterBufferEmittedCount)) * 1000,
                        concealedSamples: stat.concealedSamples,
                        totalSamplesReceived: stat.totalSamplesReceived,
                        audioEnergy: stat.totalAudioEnergy,
                    };
                }
            });
        }
        return audioStats;
    });

    await browser.close();
    return stats;
}

async function benchmarkOurStreamerAudio() {
    console.log('--- Probing GBA Streamer Audio (http://100.73.151.90:48500) ---');
    const browser = await chromium.launch({
        headless: true,
        args: ['--no-sandbox', '--autoplay-policy=no-user-gesture-required']
    });
    const page = await browser.newPage();
    await page.goto('http://100.73.151.90:48500', { waitUntil: 'networkidle' });

    // Inject tap
    await page.evaluate(() => {
        window.__audioTapL = [];
        window.__audioTapR = [];
        const checkTap = setInterval(() => {
            if (window.audioCtx && window.scriptNode) {
                clearInterval(checkTap);
                const tap = window.audioCtx.createScriptProcessor(1024, 2, 2);
                tap.onaudioprocess = (e) => {
                    const l = e.inputBuffer.getChannelData(0);
                    const r = e.inputBuffer.getChannelData(1);
                    for (let i = 0; i < l.length; i++) {
                        window.__audioTapL.push(l[i]);
                        window.__audioTapR.push(r[i]);
                    }
                };
                window.scriptNode.connect(tap);
                tap.connect(window.audioCtx.destination);
            }
        }, 50);
    });

    await page.click('#canvasWrapper');
    await page.waitForTimeout(4000);

    const data = await page.evaluate(() => {
        const l = window.__audioTapL ? window.__audioTapL.slice(0, 44100 * 3) : [];
        const r = window.__audioTapR ? window.__audioTapR.slice(0, 44100 * 3) : [];
        return {
            audioBufMs: document.getElementById('audioStatus') ? document.getElementById('audioStatus').innerText : '0',
            samplesL: l,
            samplesR: r,
        };
    });

    await browser.close();
    return data;
}

async function run() {
    console.log('===================================================================');
    console.log(' 🔊 LIVE MEASURED AUDIO BENCHMARK: GBA STREAMER vs CLOUDRETRO');
    console.log('===================================================================');

    const [cloudretroStats, ourStats] = await Promise.all([
        benchmarkCloudRetroAudio().catch(e => { console.error('CloudRetro probe error:', e); return null; }),
        benchmarkOurStreamerAudio().catch(e => { console.error('Our streamer probe error:', e); return null; })
    ]);

    console.log('\n--- CLOUDRETRO LIVE AUDIO TELEMETRY ---');
    console.log(cloudretroStats || 'No direct WebRTC audio stats available');

    console.log('\n--- OUR STREAMER LIVE AUDIO TELEMETRY ---');
    const n = ourStats ? ourStats.samplesL.length : 0;
    let rmsL = 0;
    let rmsR = 0;
    if (ourStats) {
        for (let i = 0; i < n; i++) {
            rmsL += ourStats.samplesL[i] ** 2;
            rmsR += ourStats.samplesR[i] ** 2;
        }
        rmsL = Math.sqrt(rmsL / (n || 1));
        rmsR = Math.sqrt(rmsR / (n || 1));
    }
    console.log({
        audioBufferLatency: ourStats ? ourStats.audioBufMs : 'N/A',
        samplesCaptured: n,
        rmsLeft: rmsL.toFixed(4),
        rmsRight: rmsR.toFixed(4),
        format: 'Lossless 44.1kHz Stereo PCM'
    });
}

run();
