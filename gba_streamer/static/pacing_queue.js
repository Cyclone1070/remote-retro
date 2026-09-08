class PacingQueue {
    constructor(options = {}) {
        this.mode = options.mode || 'smooth';
        this.cushionMs = options.cushionMs ?? 16.666;
        this.maxBacklog = options.maxBacklog ?? 3;
        this.queue = [];
    }

    size() {
        return this.queue.length;
    }

    getMode() {
        return this.mode;
    }

    setMode(mode) {
        if (mode === 'smooth' || mode === 'direct') {
            this.mode = mode;
        }
    }

    push(bytes, arrivalTime = performance.now()) {
        const targetTime = this.mode === 'smooth' ? (arrivalTime + this.cushionMs) : arrivalTime;
        this.queue.push({
            bytes,
            targetTime
        });

        while (this.queue.length > this.maxBacklog) {
            this.queue.shift();
        }
    }

    pop(now = performance.now()) {
        if (this.queue.length === 0) {
            return null;
        }

        if (this.mode === 'direct') {
            const head = this.queue.shift();
            return {
                bytes: head.bytes,
                blit: true
            };
        }

        // Smooth mode: pop if target cushion reached OR if >= 2 frames backlogged (catchup)
        const head = this.queue[0];
        if (now >= head.targetTime || this.queue.length >= 2) {
            this.queue.shift();
            return {
                bytes: head.bytes,
                blit: true
            };
        }

        return null;
    }
}

if (typeof module !== 'undefined' && module.exports) {
    module.exports = { PacingQueue };
} else if (typeof self !== 'undefined') {
    self.PacingQueue = PacingQueue;
}
