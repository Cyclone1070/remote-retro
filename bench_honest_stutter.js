const { chromium } = require("/tmp/node_modules/playwright");

async function run() {
    const targetUrl = process.env.STREAM_URL || "http://100.73.151.90:48500";
    console.log("===================================================================");
    console.log(" 🔬 AUDITED HONEST STUTTER & PRESENTATION BENCHMARK");
    console.log(" Target: " + targetUrl);
    console.log(" Mode:   Client Streamed Play (Measured from Canvas Render Loop)");
    console.log("===================================================================");

    const browser = await chromium.launch({
        headless: true,
        args: [
            "--autoplay-policy=no-user-gesture-required",
            "--disable-background-timer-throttling",
            "--disable-renderer-backgrounding",
            "--use-gl=angle",
            "--use-angle=metal"
        ]
    });

    const page = await browser.newPage();
    await page.goto(targetUrl, { waitUntil: "networkidle" });
    await page.click("#canvasWrapper");

    // Wait for connection
    await page.waitForFunction(() => {
        const el = document.getElementById("loadingOverlay");
        return el && window.getComputedStyle(el).visibility === "hidden";
    }, { timeout: 10000 });

    console.log("▶ Stream connected. Sending interactive gameplay inputs...");
    // Advance game
    await page.keyboard.press("Enter");
    await page.waitForTimeout(400);
    await page.keyboard.press("z");
    await page.waitForTimeout(400);
    await page.keyboard.down("d"); // Walk right to cause constant motion

    // Warm-up pipeline and let initial connection settle
    await page.waitForTimeout(1000);
    await page.evaluate(() => { window.__honestRenderHistory = []; });

    // Wait until at least 600 steady-state frames are captured
    console.log("▶ Recording 600 consecutive steady-state canvas render intervals...");
    await page.waitForFunction(() => {
        return window.__honestRenderHistory && window.__honestRenderHistory.length >= 600;
    }, { timeout: 25000 });

    await page.keyboard.up("d");

    const metrics = await page.evaluate(() => window.getHonestStutterMetrics(600));
    await browser.close();

    if (!metrics) {
        console.error("❌ Failed to collect honest stutter metrics");
        process.exit(1);
    }

    console.log(" Evaluated Frames:        " + metrics.evaluatedFrames);
    console.log(" Delivered FPS:           " + metrics.fps + " FPS");
    console.log(" Mean Frame Interval:     " + metrics.meanMs + " ms (Target: 16.667 ms)");
    console.log(" Pacing Jitter (σ):       " + metrics.sigmaMs + " ms");
    console.log(" Macro-Stutters (>=33ms): " + metrics.macroStutters + " (" + metrics.macroStutterPct + "%)");
    console.log(" Micro-Stutters (uneven): " + metrics.microStutters + " (" + metrics.microStutterPct + "%)");
    console.log(" 1% Low Framerate (P1):   " + metrics.p1LowFps + " FPS");
    console.log(" 0.1% Low Framerate:      " + metrics.p01LowFps + " FPS");
    console.log(" P50 / P95 / P99:         " + metrics.p50 + " ms / " + metrics.p95 + " ms / " + metrics.p99 + " ms");
    console.log(" Min / Max Frame Time:    " + metrics.minMs + " ms / " + metrics.maxMs + " ms");
    console.log("===================================================================");
}

run().catch(err => {
    console.error("Error:", err);
    process.exit(1);
});
