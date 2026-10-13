// Bounded AI response cache shared by lookups and restored after restarting the app.
//  * completed answers are kept for 24 hours per request-content key;
//  * a request already in flight for the same key is joined, not restarted, so a redraw of the
//    same lookup lets it finish;
//  * looking up another word cancels only the requests that are no longer displayed;
//  * partial, cancelled or failed answers are never cached;
//  * regenerating drops the cached answer and forces a new request.

const CACHE_TIMEOUT_MS = 24 * 60 * 60 * 1000;

export class AiResponseCache {
    constructor(timeoutMs = CACHE_TIMEOUT_MS, {storage = null, maxEntries = 100, maxChars = 2_000_000, now = Date.now} = {}) {
        /** @type {Map<string, {text: string, timestamp: number}>} */
        this._cache = new Map();
        /** @type {Map<string, {controller: AbortController, cancelled: boolean, full: string, listeners: Set<Function>, promise: Promise<string>}>} */
        this._pending = new Map();
        this._timeout = timeoutMs;
        this._storage = storage;
        this._maxEntries = maxEntries;
        this._maxChars = maxChars;
        this._now = now;
        this._storageKey = 'lookitan.ai-cache.v1';
        try {
            const stored = JSON.parse(storage?.getItem(this._storageKey) || 'null');
            if (stored?.version === 1 && Array.isArray(stored.entries)) {
                for (const [key, hit] of stored.entries.slice(-maxEntries)) {
                    if (typeof key === 'string' && typeof hit?.text === 'string' && hit.text.trim() &&
                        Number.isFinite(hit.timestamp) && this._fresh(hit)) {
                        this._cache.set(key, {text: hit.text, timestamp: hit.timestamp});
                    }
                }
                this._prune();
            }
        } catch (_e) { /* Damaged or unavailable storage must not break lookup. */ }
    }

    /** @param {unknown[]} parts everything that changes the answer */
    key(parts) {
        return JSON.stringify(parts);
    }

    /** The cached answer for `key` while it is fresh. */
    peek(key) {
        const hit = this._cache.get(key);
        if (hit && this._fresh(hit)) {
            this._cache.delete(key);
            this._cache.set(key, hit);
            return hit.text;
        }
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
            this._persist();
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
                if (!pending.cancelled && this._pending.get(key) === pending && typeof text === 'string' && text.trim()) {
                    this._cache.set(key, {text, timestamp: this._now()});
                    this._prune();
                    this._persist();
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
        try { this._storage?.removeItem(this._storageKey); } catch (_e) { /* Keep working in memory. */ }
    }

    _fresh(hit) {
        const age = this._now() - hit.timestamp;
        return age >= 0 && age < this._timeout;
    }

    _prune() {
        for (const [key, hit] of this._cache) {
            if (!this._fresh(hit)) { this._cache.delete(key); }
        }
        let chars = [...this._cache].reduce((sum, [key, hit]) => sum + key.length + hit.text.length, 0);
        for (const [key, hit] of this._cache) {
            if (this._cache.size <= this._maxEntries && chars <= this._maxChars) { break; }
            chars -= key.length + hit.text.length;
            this._cache.delete(key);
        }
    }

    _persist() {
        try { this._storage?.setItem(this._storageKey, JSON.stringify({version: 1, entries: [...this._cache]})); }
        catch (_e) { /* Storage quota or permissions: retain the in-memory cache. */ }
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
