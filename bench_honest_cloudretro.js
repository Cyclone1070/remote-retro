const { chromium } = require("/tmp/node_modules/playwright");

async function run() {
    const targetUrl = process.env.STREAM_URL || "http://192.168.1.111:8000";
    console.log("===================================================================");
    console.log(" 🔬 AUDITED HONEST CLOUDRETRO (WEBRTC VP8) BENCHMARK");
    console.log(" Target: " + targetUrl);
    console.log(" Mode:   WebRTC Native <video> (Measured via requestVideoFrameCallback)");
    console.log("===================================================================");

    const browser = await chromium.launch({
        headless: true,
        args: [
            "--no-sandbox",
            "--autoplay-policy=no-user-gesture-required",
            "--use-fake-ui-for-media-stream",
            "--disable-background-timer-throttling",
            "--disable-renderer-backgrounding",
            "--use-gl=angle",
            "--use-angle=metal"
        ]
    });

    const page = await browser.newPage();
    await page.goto(targetUrl, { waitUntil: "networkidle" });
    await page.waitForTimeout(1500);

    // Click 'Sushi The Cat' game
    const selected = await page.evaluate(() => {
        const els = Array.from(document.querySelectorAll('*'));
        const target = els.find(e => e.innerText && e.innerText.trim() === 'Sushi The Cat');
        if (target) {
            target.click();
            return target.innerText.trim();
        }
        return 'none';
    });
    console.log(` Selected Game:   ${selected}`);

    // Start game
    await page.keyboard.press("Enter");
    await page.waitForTimeout(2000);

    // Verify video playback
    const isPlaying = await page.waitForFunction(() => {
        const v = document.getElementById("stream");
        return v && !v.paused && v.readyState >= 3 && v.videoWidth > 0;
    }, { timeout: 10000 });

    if (!isPlaying) {
        console.error("❌ Failed to start CloudRetro video stream");
        await browser.close();
        process.exit(1);
    }

    const targetFrames = parseInt(process.env.BENCH_FRAMES || "1800", 10);
    const targetDurationSec = (targetFrames / 60).toFixed(0);
    console.log(` Target Frames:   ${targetFrames} frames (~${targetDurationSec} seconds)`);
    console.log("===================================================================");

    // Advance title screen
    await page.keyboard.press("Enter");
    await page.waitForTimeout(500);
    await page.keyboard.press("z");
    await page.waitForTimeout(500);

    // Warm-up pipeline and let WebRTC stabilize
    await page.waitForTimeout(1500);

    // Install requestVideoFrameCallback probe
    await page.evaluate(() => {
        window.__honestRenderHistory = [];
        const v = document.getElementById("stream");
        let lastT = null;
        function onFrame(now, metadata) {
            if (lastT !== null) {
                const dt = now - lastT;
                window.__honestRenderHistory.push(dt);
            }
            lastT = now;
            v.requestVideoFrameCallback(onFrame);
        }
        v.requestVideoFrameCallback(onFrame);

        // Honest stutter metrics calculation (IDENTICAL mathematical formula)
        window.getHonestStutterMetrics = function(sampleCount) {
            const history = window.__honestRenderHistory;
            const slice = history.slice(-sampleCount);
            if (slice.length < 30) return null;
            const n = slice.length;
            const mean = slice.reduce((a, b) => a + b, 0) / n;
            const variance = slice.reduce((a, b) => a + Math.pow(b - mean, 2), 0) / (n - 1 || 1);
            const sigma = Math.sqrt(variance);
            const macros = slice.filter(d => d >= 33.33).length;
            const micros = slice.filter(d => (d > 20.0 || d < 13.3) && d < 33.33).length;
            const sorted = [...slice].sort((a, b) => a - b);
            const p1Idx = Math.floor(n * 0.01);
            const p01Idx = Math.floor(n * 0.001);
            const p1Delta = sorted[Math.min(n - 1, Math.max(0, n - 1 - p1Idx))];
            const p01Delta = sorted[Math.min(n - 1, Math.max(0, n - 1 - p01Idx))];
            return {
                evaluatedFrames: n,
                fps: (1000.0 / mean).toFixed(2),
                meanMs: mean.toFixed(3),
                sigmaMs: sigma.toFixed(3),
                macroStutters: macros,
                macroStutterPct: ((macros / n) * 100).toFixed(2),
                microStutters: micros,
                microStutterPct: ((micros / n) * 100).toFixed(2),
                p1LowFps: (1000.0 / p1Delta).toFixed(2),
                p01LowFps: (1000.0 / p01Delta).toFixed(2),
                p50: sorted[Math.floor(n * 0.50)].toFixed(2),
                p95: sorted[Math.floor(n * 0.95)].toFixed(2),
                p99: sorted[Math.floor(n * 0.99)].toFixed(2),
                minMs: sorted[0].toFixed(2),
                maxMs: sorted[sorted.length - 1].toFixed(2),
            };
        };
    });

    console.log(`▶ Running choreographed human gameplay bot over ${targetFrames} frames...`);
    let botRunning = true;
    const botPromise = (async () => {
        let cycle = 0;
        while (botRunning) {
            try {
                // Phase 1: Sprint Right (2.5s) with mid-run hop
                await page.keyboard.down("ArrowRight");
                await page.waitForTimeout(1000);
                if (!botRunning) break;
                await page.keyboard.press("z");
                await page.waitForTimeout(1500);
                await page.keyboard.up("ArrowRight");
                if (!botRunning) break;

                // Phase 2: Sprint Left (2.0s) with high leap
                await page.keyboard.down("ArrowLeft");
                await page.waitForTimeout(1000);
                if (!botRunning) break;
                await page.keyboard.down("z");
                await page.waitForTimeout(300);
                await page.keyboard.up("z");
                await page.waitForTimeout(700);
                await page.keyboard.up("ArrowLeft");
                if (!botRunning) break;

                // Phase 3: Crouch (0.8s)
                await page.keyboard.down("ArrowDown");
                await page.waitForTimeout(800);
                await page.keyboard.up("ArrowDown");
                if (!botRunning) break;

                // Phase 4: Look Up (0.8s)
                await page.keyboard.down("ArrowUp");
                await page.waitForTimeout(800);
                await page.keyboard.up("ArrowUp");
                if (!botRunning) break;

                cycle++;
                // Every 3 cycles (~25s), toggle Pause menu
                if (cycle % 3 === 0) {
                    await page.keyboard.press("Enter");
                    await page.waitForTimeout(1500);
                    await page.keyboard.press("Enter");
                    await page.waitForTimeout(500);
                }
            } catch(e) {
                break;
            }
        }
    })();

    // Monitor progress periodically
    const checkInterval = setInterval(async () => {
        try {
            const count = await page.evaluate(() => window.__honestRenderHistory ? window.__honestRenderHistory.length : 0);
            const elapsed = (count / 60).toFixed(0);
            console.log(`  ⏳ [${count}/${targetFrames} frames | ${elapsed}s / ${targetDurationSec}s]`);
        } catch(e) {}
    }, 15000);

    // Wait until targetFrames steady-state frames are captured
    await page.waitForFunction((needed) => {
        return window.__honestRenderHistory && window.__honestRenderHistory.length >= needed;
    }, targetFrames, { timeout: (targetFrames / 60 * 1000) + 60000 });

    clearInterval(checkInterval);
    botRunning = false;
    await page.keyboard.up("ArrowRight").catch(()=>{});
    await page.keyboard.up("ArrowLeft").catch(()=>{});
    await page.keyboard.up("ArrowUp").catch(()=>{});
    await page.keyboard.up("ArrowDown").catch(()=>{});
    await botPromise.catch(()=>{});

    const metrics = await page.evaluate((n) => window.getHonestStutterMetrics(n), targetFrames);

    // Extract true WebRTC getStats()
    const rtcStats = await page.evaluate(async () => {
        if (!window.pc) return null;
        const reports = await window.pc.getStats();
        let videoIn = null;
        let audioIn = null;
        let candidatePair = null;

        reports.forEach(s => {
            if (s.type === "inbound-rtp" && s.kind === "video") videoIn = s;
            if (s.type === "inbound-rtp" && s.kind === "audio") audioIn = s;
            if (s.type === "candidate-pair" && s.state === "succeeded" && s.currentRoundTripTime !== undefined) candidatePair = s;
        });

        return {
            rttMs: candidatePair ? (candidatePair.currentRoundTripTime * 1000).toFixed(2) : "N/A",
            videoBytes: videoIn ? videoIn.bytesReceived : 0,
            framesDecoded: videoIn ? videoIn.framesDecoded : 0,
            framesDropped: videoIn ? videoIn.framesDropped : 0,
            avgDecodeMs: (videoIn && videoIn.framesDecoded > 0) ? ((videoIn.totalDecodeTime * 1000) / videoIn.framesDecoded).toFixed(2) : "N/A",
            jitterBufferMs: (videoIn && videoIn.jitterBufferEmittedCount > 0) ? ((videoIn.jitterBufferDelay * 1000) / videoIn.jitterBufferEmittedCount).toFixed(2) : "N/A",
            videoJitterMs: videoIn ? (videoIn.jitter * 1000).toFixed(2) : "N/A",
            audioBytes: audioIn ? audioIn.bytesReceived : 0,
            audioPacketsLost: audioIn ? audioIn.packetsLost : 0,
            audioConcealedPct: (audioIn && audioIn.totalSamplesReceived > 0) ? ((audioIn.concealedSamples / audioIn.totalSamplesReceived) * 100).toFixed(2) : "0.00",
            audioJitterMs: audioIn ? (audioIn.jitter * 1000).toFixed(2) : "N/A",
        };
    });

    await browser.close();

    if (!metrics) {
        console.error("❌ Failed to collect honest stutter metrics");
        process.exit(1);
    }

    console.log("\n===================================================================");
    console.log(` 🏆 CLOUDRETRO IN-BROWSER CLIENT PRESENTATION (${metrics.evaluatedFrames} FRAMES)`);
    console.log("===================================================================");
    console.log(" Evaluated Frames:        " + metrics.evaluatedFrames);
    console.log(" Delivered Display FPS:   " + metrics.fps + " FPS");
    console.log(" Mean Frame Interval:     " + metrics.meanMs + " ms (Target: 16.667 ms)");
    console.log(" Pacing Jitter (σ):       " + metrics.sigmaMs + " ms");
    console.log(" Macro-Stutters (>=33ms): " + metrics.macroStutters + " (" + metrics.macroStutterPct + "%)");
    console.log(" Micro-Stutters (uneven): " + metrics.microStutters + " (" + metrics.microStutterPct + "%)");
    console.log(" 1% Low Framerate (P1):   " + metrics.p1LowFps + " FPS");
    console.log(" 0.1% Low Framerate:      " + metrics.p01LowFps + " FPS");
    console.log(" P50 / P95 / P99:         " + metrics.p50 + " ms / " + metrics.p95 + " ms / " + metrics.p99 + " ms");
    console.log(" Min / Max Frame Time:    " + metrics.minMs + " ms / " + metrics.maxMs + " ms");
    if (rtcStats) {
        console.log("-------------------------------------------------------------------");
        console.log(" WebRTC True RTT:         " + rtcStats.rttMs + " ms");
        console.log(" WebRTC Video Decode:     " + rtcStats.avgDecodeMs + " ms");
        console.log(" WebRTC Jitter Buffer:    " + rtcStats.jitterBufferMs + " ms");
        console.log(" WebRTC Frames Dropped:   " + rtcStats.framesDropped);
        console.log(" WebRTC Audio Glitch/Loss:" + rtcStats.audioConcealedPct + "% concealed, " + rtcStats.audioPacketsLost + " lost");
        const totalKb = ((rtcStats.videoBytes + rtcStats.audioBytes) / 1024).toFixed(1);
        const duration = (metrics.evaluatedFrames / parseFloat(metrics.fps));
        const mbps = (((rtcStats.videoBytes + rtcStats.audioBytes) * 8) / (duration * 1e6)).toFixed(2);
        console.log(" WebRTC Total Bandwidth:  " + mbps + " Mbps (" + totalKb + " KB over " + duration.toFixed(1) + "s)");
    }
    console.log("===================================================================\n");
}

run().catch(err => {
    console.error("Error:", err);
    process.exit(1);
});
