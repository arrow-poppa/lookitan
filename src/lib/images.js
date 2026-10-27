// Pictures of the looked-up word, found the way Migaku's pop-up finds them (behavior only, none of
// its code): the AI turns the word into a short English search query, and Bing Images is searched
// with it. Without AI, the word itself is searched, as Migaku does when its AI call fails.
//
// Bing has no free image API, so this reads the same results page Migaku reads, `/images/async`.
// Each result there has an `m` attribute holding JSON: the picture (`murl`), Bing's own thumbnail
// (`turl`), its title (`t`) and the page it is on (`purl`). Migaku keeps only `murl`; the pop-up
// shows `turl`, which is small and always served, and the viewer and Anki get `murl`.

import {fillLanguages, languageName, promptText} from './ai/word-card.js';

/** Pictures shown per lookup, as in Migaku's pop-up. */
export const IMAGE_COUNT = 10;

/** Longest side of a picture sent to Anki. Migaku scales card pictures to 800 px wide. */
export const ANKI_IMAGE_MAX_SIDE = 800;

const SEARCH_TIMEOUT_MS = 15000;
const DOWNLOAD_TIMEOUT_MS = 15000;

export const IMAGE_QUERY_SYSTEM_PROMPT =
    'You turn a word into a picture search query.\n\n' +
    'Translate the word into ENGLISH, with the meaning it has in the context.\n\n' +
    'Answer with the one expression people use most for that exact meaning.\n\n' +
    'ALWAYS answer with the query only: no quotes, no explanation.\n\n' +
    "ALWAYS keep the dictionary form, for example prepend 'to ' for verbs.";

export const IMAGE_QUERY_PROMPT = 'Word: {{target}} ({{wordLanguage}})\nContext: {{sentence}}';

/**
 * The image query prompts as sent. {{target}} and {{sentence}} are filled by the AI provider, in
 * both prompts. A prompt the user emptied is '' (see `promptText`): an empty system prompt sends
 * none, and an empty user prompt asks the AI nothing, so the word itself is searched.
 * @param {{imageQuerySystemPrompt?: string, imageQueryPrompt?: string}} ai
 * @param {{ocrLanguage?: string, definitionLanguage?: string, translationLanguage?: string}} settings
 */
export function imagePrompts(ai, settings) {
    const fill = (text, fallback) => fillLanguages(promptText(text, fallback), settings)
        .replaceAll('{{wordLanguage}}', languageName(settings.ocrLanguage || 'en-US'));
    return {
        system: fill(ai.imageQuerySystemPrompt, IMAGE_QUERY_SYSTEM_PROMPT),
        user: fill(ai.imageQueryPrompt, IMAGE_QUERY_PROMPT),
    };
}

/**
 * The word the pictures look for. Unless "Usar a palavra corrigida pelo card" (`imageUseCardWord`)
 * is off, in this order: the longer unit the AI word card described ("Socrates Awards" for
 * "Socrates"), by its headword, which also carries any OCR fix, when its pieces were found in the
 * context (`unitFound`); else the word the card corrected the OCR to (`ocrFix`, "read →
 * corrected"); else, and always with the switch off, the word as the OCR read it. A unit the AI
 * proposed but that is not in the text is never used. No second AI request is made for it.
 * @param {string} word
 * @param {{ocrFix?: string, headword?: string, selection?: object|null}|null|undefined} card
 * @param {{imageUseCardWord?: boolean}} ai
 * @param {boolean} [unitFound]
 */
export function imageWord(word, card, ai, unitFound = false) {
    if (ai.imageUseCardWord === false) { return word; }
    const unit = card?.selection && unitFound ? String(card.headword || '').trim() : '';
    const fix = String(card?.ocrFix || '');
    const corrected = fix.includes('→') ? fix.slice(fix.lastIndexOf('→') + 1).trim() : '';
    return unit || corrected || word;
}

/**
 * The search query in the AI's answer: its first line, without quotes or a "Query:" label. An empty
 * or rambling answer gives the word itself.
 * @param {string} text
 * @param {string} fallback
 */
export function cleanQuery(text, fallback) {
    const line = String(text || '').split('\n').map((l) => l.trim()).find(Boolean) || '';
    const query = line
        .replace(/^(?:search\s+)?(?:query|search|term)\s*:\s*/i, '')
        .replace(/^["'`“”‘’«»*]+|["'`“”‘’«»*.]+$/g, '')
        .trim();
    return query && query.length <= 120 ? query : String(fallback || '').trim();
}

/**
 * Migaku's meme mode: " meme" at the end of the search. SafeSearch stays strict, but the results
 * can still be crude, which Migaku warns about too.
 */
export function withMemeMode(query, on) {
    return on ? `${query} meme` : query;
}

/** Bing Images' results page for a query, with Migaku's parameters (SafeSearch strict). */
export function bingUrl(query) {
    return `https://www.bing.com/images/async?q=${encodeURIComponent(query)}&first=0&count=100&adlt=strict`;
}

function unescapeHtml(text) {
    return text.replace(/&(?:#(\d+)|#x([0-9a-f]+)|(quot|amp|lt|gt|apos));/gi, (m, dec, hex, name) => {
        if (dec) { return String.fromCodePoint(Number(dec)); }
        if (hex) { return String.fromCodePoint(parseInt(hex, 16)); }
        return {quot: '"', amp: '&', lt: '<', gt: '>', apos: "'"}[name.toLowerCase()] ?? m;
    });
}

const isWebUrl = (url) => /^https?:\/\//i.test(url);

/**
 * The pictures on a Bing Images results page, in Bing's order, each picture once.
 * @param {string} html
 * @returns {{src: string, thumb: string, title: string, page: string}[]}
 */
export function parseBingImages(html) {
    const out = [];
    const seen = new Set();
    const add = (item) => {
        if (!isWebUrl(item.src) || seen.has(item.src)) { return; }
        seen.add(item.src);
        out.push({...item, thumb: isWebUrl(item.thumb) ? item.thumb : item.src});
    };
    for (const m of String(html).matchAll(/\sm="(\{[^"]*\})"/g)) {
        try {
            const d = JSON.parse(unescapeHtml(m[1]));
            add({src: String(d.murl || ''), thumb: String(d.turl || ''), title: String(d.t || ''), page: String(d.purl || '')});
        } catch (_e) { /* an `m` attribute that is not a result */ }
    }
    if (out.length === 0) {
        // Only the picture's address, read the way Migaku reads it.
        for (const m of String(html).matchAll(/murl&quot;:&quot;(.*?)&quot;/g)) {
            add({src: unescapeHtml(m[1]), thumb: '', title: '', page: ''});
        }
    }
    return out;
}

/** Aborts with a TimeoutError after `ms`, or when `signal` aborts. */
function deadline(ms, signal) {
    const controller = new AbortController();
    const timer = setTimeout(() => controller.abort(new DOMException('Image request timed out', 'TimeoutError')), ms);
    const abort = () => controller.abort(signal.reason);
    signal?.addEventListener('abort', abort, {once: true});
    return {signal: controller.signal, done: () => { clearTimeout(timer); signal?.removeEventListener('abort', abort); }};
}

async function httpFetch(request) {
    // Through the app, like the AI requests: Bing sends no CORS headers.
    return request || (await import('@tauri-apps/plugin-http')).fetch;
}

/**
 * Searches Bing Images. `fetch` defaults to the Tauri HTTP plugin's.
 * @param {string} query
 * @param {{signal?: AbortSignal, fetch?: typeof globalThis.fetch}} [options]
 */
export async function searchImages(query, {signal, fetch: request} = {}) {
    const doFetch = await httpFetch(request);
    const timeout = deadline(SEARCH_TIMEOUT_MS, signal);
    try {
        // Bing now and then answers a page with no results; asking once more usually gives them.
        for (let attempt = 0; ; attempt++) {
            const response = await doFetch(bingUrl(query), {method: 'GET', headers: {'Cache-Control': 'no-cache', 'Pragma': 'no-cache'}, signal: timeout.signal});
            if (!response.ok) { throw new Error(`O Bing Imagens respondeu ${response.status}.`); }
            const items = parseBingImages(await response.text());
            if (items.length > 0 || attempt === 1) { return items.slice(0, IMAGE_COUNT); }
        }
    } finally {
        timeout.done();
    }
}

function toBase64(bytes) {
    let binary = '';
    for (let i = 0; i < bytes.length; i += 0x8000) {
        binary += String.fromCharCode.apply(null, bytes.subarray(i, i + 0x8000));
    }
    return btoa(binary);
}

const EXTENSIONS = {'image/jpeg': 'jpg', 'image/png': 'png', 'image/gif': 'gif', 'image/webp': 'webp', 'image/avif': 'avif', 'image/bmp': 'bmp'};

/** A picture no larger than ANKI_IMAGE_MAX_SIDE, as JPEG; GIFs and pictures the WebView cannot decode stay as they are. */
async function shrink(blob, type) {
    if (type === 'image/gif' || typeof createImageBitmap !== 'function') { return null; }
    let bitmap;
    try { bitmap = await createImageBitmap(blob); } catch (_e) { return null; }
    try {
        const scale = Math.min(1, ANKI_IMAGE_MAX_SIDE / Math.max(bitmap.width, bitmap.height));
        const canvas = document.createElement('canvas');
        canvas.width = Math.max(1, Math.round(bitmap.width * scale));
        canvas.height = Math.max(1, Math.round(bitmap.height * scale));
        const g = canvas.getContext('2d');
        // JPEG has no transparency: a transparent picture goes on white, as Anki shows it.
        g.fillStyle = '#fff';
        g.fillRect(0, 0, canvas.width, canvas.height);
        g.drawImage(bitmap, 0, 0, canvas.width, canvas.height);
        const out = await new Promise((done) => canvas.toBlob(done, 'image/jpeg', 0.88));
        return out ? {bytes: new Uint8Array(await out.arrayBuffer()), extension: 'jpg'} : null;
    } finally {
        bitmap.close?.();
    }
}

/**
 * Downloads a picture for an Anki note: the full picture, or Bing's thumbnail when the site does
 * not serve it. Returns null when neither can be read.
 * @param {{src: string, thumb: string}} item
 * @param {{signal?: AbortSignal, fetch?: typeof globalThis.fetch}} [options]
 * @returns {Promise<{data: string, extension: string}|null>}
 */
export async function downloadImage(item, {signal, fetch: request} = {}) {
    const doFetch = await httpFetch(request);
    for (const url of [...new Set([item.src, item.thumb].filter(isWebUrl))]) {
        const timeout = deadline(DOWNLOAD_TIMEOUT_MS, signal);
        try {
            const response = await doFetch(url, {method: 'GET', signal: timeout.signal});
            const type = (response.headers.get('content-type') || '').split(';')[0].trim().toLowerCase();
            if (!response.ok || !type.startsWith('image/')) { continue; }
            const blob = new Blob([await response.arrayBuffer()], {type});
            const small = await shrink(blob, type);
            if (small) { return {data: toBase64(small.bytes), extension: small.extension}; }
            if (!EXTENSIONS[type]) { continue; }
            return {data: toBase64(new Uint8Array(await blob.arrayBuffer())), extension: EXTENSIONS[type]};
        } catch (e) {
            if (signal?.aborted) { throw e; }
        } finally {
            timeout.done();
        }
    }
    return null;
}
