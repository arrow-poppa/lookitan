// AI response cache with the behavior of yomitan-byok-ai's AIExplanationGenerator:
//  * answers are kept for one minute per (options, term, sentence) key;
//  * a request already in flight for the same key is joined, not restarted, so a redraw of the
//    same lookup lets it finish;
//  * looking up another word cancels only the requests that are no longer displayed;
//  * partial, cancelled or failed answers are never cached;
//  * regenerating drops the cached answer and forces a new request.

const CACHE_TIMEOUT_MS = 60000;

export class AiResponseCache {
    constructor(timeoutMs = CACHE_TIMEOUT_MS) {
        /** @type {Map<string, {text: string, timestamp: number}>} */
        this._cache = new Map();
        /** @type {Map<string, {controller: AbortController, cancelled: boolean, full: string, listeners: Set<Function>, promise: Promise<string>}>} */
        this._pending = new Map();
        this._timeout = timeoutMs;
    }

    /** @param {unknown[]} parts everything that changes the answer */
    key(parts) {
        return JSON.stringify(parts);
    }

    /** The cached answer for `key` while it is fresh. */
    peek(key) {
        const hit = this._cache.get(key);
        if (hit && Date.now() - hit.timestamp < this._timeout) { return hit.text; }
        if (hit) { this._cache.delete(key); }
        return null;
    }

    /**
     * Returns the cached answer, joins the request in flight, or starts one.
     * @param {string} key
     * @param {(signal: AbortSignal, onChunk: (delta: string, full: string) => void) => Promise<string>} start
     * @param {(delta: string, full: string) => void} [onChunk]
     * @param {{force?: boolean}} [options]
     */
    run(key, start, onChunk, {force = false} = {}) {
        if (force) {
            this._cache.delete(key);
            this._abort(key);
        }
        const cached = this.peek(key);
        if (cached !== null) {
            onChunk?.(cached, cached);
            return Promise.resolve(cached);
        }
        const existing = this._pending.get(key);
        if (existing) {
            if (onChunk) {
                existing.listeners.add(onChunk);
                if (existing.full) { onChunk(existing.full, existing.full); }
            }
            return existing.promise;
        }
        const pending = {controller: new AbortController(), cancelled: false, full: '', listeners: new Set(onChunk ? [onChunk] : []), promise: null};
        this._pending.set(key, pending);
        pending.promise = (async () => {
            try {
                const text = await start(pending.controller.signal, (delta, full) => {
                    if (pending.cancelled) { return; }
                    pending.full = full;
                    for (const l of pending.listeners) { l(delta, full); }
                });
                pending.controller.signal.throwIfAborted();
                if (!pending.cancelled && this._pending.get(key) === pending) {
                    this._cache.set(key, {text, timestamp: Date.now()});
                }
                return text;
            } finally {
                if (this._pending.get(key) === pending) { this._pending.delete(key); }
            }
        })();
        return pending.promise;
    }

    /** Stops listening without cancelling (the popup moved on but the answer may be reused). */
    detach(key, onChunk) {
        this._pending.get(key)?.listeners.delete(onChunk);
    }

    /** Aborts every request in flight except those for `keep`. */
    cancelPendingExcept(keep) {
        for (const key of [...this._pending.keys()]) {
            if (!keep.has(key)) { this._abort(key); }
        }
    }

    cancelPending() {
        for (const key of [...this._pending.keys()]) { this._abort(key); }
    }

    clear() {
        this._cache.clear();
    }

    _abort(key) {
        const p = this._pending.get(key);
        if (!p) { return; }
        p.cancelled = true;
        p.listeners.clear();
        try { p.controller.abort(); } catch (_e) { /* ignore */ }
        this._pending.delete(key);
    }
}
