// "Imagem AI" pictures kept on disk, so a word looked up again after the app was closed shows the
// pictures already paid for. They live in the WebView's IndexedDB (in the app's data folder on
// Windows and Linux), newest first, up to STORED_PICTURES pictures; older sets are dropped.
// Without IndexedDB (tests, a private profile) nothing is stored and nothing breaks.

const DB_NAME = 'lookitan-image-gen';
const STORE = 'generations';

/** Pictures kept in all, about 1–2 MB each. */
export const STORED_PICTURES = 100;

let opening = null;

function open() {
    if (typeof indexedDB === 'undefined') { return Promise.resolve(null); }
    opening ||= new Promise((done) => {
        try {
            const request = indexedDB.open(DB_NAME, 1);
            request.onupgradeneeded = () => { request.result.createObjectStore(STORE, {keyPath: 'key'}); };
            request.onsuccess = () => done(request.result);
            request.onerror = () => done(null);
            request.onblocked = () => done(null);
        } catch (_e) {
            done(null);
        }
    });
    return opening;
}

/** Runs `work` on the store and resolves with what it sets as the result; null on any failure. */
async function withStore(mode, work) {
    const db = await open();
    if (!db) { return null; }
    return new Promise((done) => {
        try {
            const tx = db.transaction(STORE, mode);
            let result = null;
            work(tx.objectStore(STORE), (value) => { result = value; });
            tx.oncomplete = () => done(result);
            tx.onerror = () => done(null);
            tx.onabort = () => done(null);
        } catch (_e) {
            done(null);
        }
    });
}

/**
 * The pictures saved for a prompt, or null.
 * @returns {Promise<{blob: Blob, title: string}[]|null>}
 */
export async function loadGeneration(key) {
    const record = await withStore('readonly', (store, set) => {
        const request = store.get(key);
        request.onsuccess = () => set(request.result || null);
    });
    if (!record || !Array.isArray(record.pictures) || record.pictures.length === 0) { return null; }
    // Seen again: it is kept longer than the sets not looked at.
    void withStore('readwrite', (store) => { store.put({...record, usedAt: Date.now()}); });
    return record.pictures.filter((p) => p?.blob instanceof Blob);
}

/**
 * Saves the pictures of a prompt (replacing what it had), then drops the least recently seen sets
 * while more than STORED_PICTURES pictures are kept.
 * @param {{blob: Blob, title: string}[]} pictures
 */
export async function saveGeneration(key, pictures) {
    if (!await open()) { return; }
    const kept = pictures.filter((p) => p?.blob instanceof Blob).map((p) => ({blob: p.blob, title: String(p.title || '')}));
    if (kept.length === 0) { return forgetStoredGeneration(key); }
    await withStore('readwrite', (store) => {
        store.put({key, pictures: kept, usedAt: Date.now()});
        const all = store.getAll();
        all.onsuccess = () => {
            let total = 0;
            for (const record of all.result.sort((a, b) => b.usedAt - a.usedAt)) {
                total += record.pictures.length;
                if (total > STORED_PICTURES && record.key !== key) { store.delete(record.key); }
            }
        };
    });
}

export async function forgetStoredGeneration(key) {
    await withStore('readwrite', (store) => { store.delete(key); });
}
