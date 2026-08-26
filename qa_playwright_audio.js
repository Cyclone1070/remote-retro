const { chromium } = require('/tmp/node_modules/playwright');

async function testPlaywrightAudio() {
    console.log('===================================================================');
    console.log(' 🎭 PLAYWRIGHT AUTONOMOUS CLIENT AUDIO QA');
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
    page.on('pageerror', err => console.error('BROWSER ERROR:', err));

    console.log('Navigating to http://100.73.151.90:48500 ...');
    await page.goto('http://100.73.151.90:48500', { waitUntil: 'networkidle' });

    console.log('Clicking canvas wrapper to unlock AudioContext...');
    await page.click('#canvasWrapper');

    // Wait 3 seconds for stream and audio chunks to play
    await page.waitForTimeout(3000);

    const telemetry = await page.evaluate(() => {
        return {
            fps: document.getElementById('fpsVal').innerText,
            totalLag: document.getElementById('totalLatency').innerText,
            hostLatency: document.getElementById('hostLatency').innerText,
            audioStatus: document.getElementById('audioStatus').innerText,
            hasAudioCtx: !!window.audioCtx,
            audioCtxState: window.audioCtx ? window.audioCtx.state : 'none',
        };
    });

    const screenshotPath = '/tmp/playwright_audio_qa.png';
    await page.screenshot({ path: screenshotPath });
    await browser.close();

    console.log('\n--- PLAYWRIGHT CLIENT TELEMETRY ---');
    console.log(` Delivered FPS:    ${telemetry.fps}`);
    console.log(` Audio Status:     ${telemetry.audioStatus}`);
    console.log(` AudioContext:     ${telemetry.hasAudioCtx ? 'Active' : 'Missing'} (State: ${telemetry.audioCtxState})`);
    console.log(` Total M2P Lag:    ${telemetry.totalLag}`);
    console.log(` Screenshot:       ${screenshotPath}`);
    console.log('-----------------------------------\n');

    let pass = true;
    if (!telemetry.hasAudioCtx || telemetry.audioCtxState !== 'running') {
        console.error('❌ FAIL: AudioContext is not running');
        pass = false;
    }
    if (parseFloat(telemetry.fps) < 55.0) {
        console.error('❌ FAIL: FPS too low');
        pass = false;
    }

    if (pass) {
        console.log('✅ PASS: Playwright Autonomous QA verified AudioContext is 100% active and streaming!');
        process.exit(0);
    } else {
        process.exit(1);
    }
}

testPlaywrightAudio().catch(e => {
    console.error('Playwright QA Error:', e);
    process.exit(1);
});
