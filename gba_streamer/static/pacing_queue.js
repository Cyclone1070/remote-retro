class PacingQueue {
    constructor(options = {}) {
        this.mode = options.mode || 'smooth';
        this.cushionMs = options.cushionMs ?? 16.666;
        this.maxBacklog = options.maxBacklog ?? 3;
        this.queue = [];
        this.primed = false;
        this.holdTicks = 0;
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
            this.primed = false;
            this.holdTicks = 0;
        }
    }

    push(bytes, arrivalTime = performance.now()) {
        const targetTime = arrivalTime + this.cushionMs;
        this.queue.push({
            bytes,
            targetTime,
            arrivalTime
        });

        while (this.queue.length > this.maxBacklog) {
            this.queue.shift();
        }
    }

    shiftDirect() {
        return this.queue.shift() || null;
    }

    pop(now = performance.now()) {
        if (this.queue.length === 0) {
            return null;
        }

        if (this.mode === 'direct') {
            const head = this.queue.shift();
            return {
                bytes: head.bytes,
                blit: true,
                waitMs: now - head.arrivalTime
            };
        }

        // Smooth mode: pop if target cushion reached OR catchup when backlog >= 2
        const head = this.queue[0];
        if (now >= head.targetTime || this.queue.length >= 2) {
            this.queue.shift();
            return {
                bytes: head.bytes,
                blit: true,
                waitMs: now - head.arrivalTime
            };
        }

        return null;
    }

    clear() {
        this.queue.length = 0;
    }
}

if (typeof module !== 'undefined' && module.exports) {
    module.exports = { PacingQueue };
} else if (typeof self !== 'undefined') {
    self.PacingQueue = PacingQueue;
}
