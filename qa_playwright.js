const { chromium } = require('/tmp/node_modules/playwright');

(async () => {
    console.log('=== Starting Playwright Autonomous QA ===');
    const browser = await chromium.launch({
        headless: true,
        args: ['--autoplay-policy=no-user-gesture-required', '--no-sandbox']
    });

    const context = await browser.newContext();
    const page = await context.newPage();

    page.on('console', msg => console.log('PAGE LOG:', msg.text()));
    page.on('pageerror', err => console.error('PAGE ERROR:', err));

    await page.goto('http://100.73.151.90:48500', { waitUntil: 'networkidle' });
    console.log('Page loaded, clicking overlay...');
    await page.click('#loadingOverlay');

    await page.waitForTimeout(3000);

    const audioState = await page.evaluate(() => {
        return {
            hasAudioCtx: !!window.audioCtx,
            audioCtxState: window.audioCtx ? window.audioCtx.state : 'none',
            audioNodeExists: !!window.audioNode,
            readPtr: window.audioReadPtr,
            writePtr: window.audioWritePtr,
            queueLen: typeof window.getAudioQueueLength === 'function' ? window.getAudioQueueLength() : -1,
        };
    });

    console.log('Audio State from Playwright:', audioState);
    await browser.close();
})();
