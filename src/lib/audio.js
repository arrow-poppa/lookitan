// Word audio, from the sources the user lists in Configurações › Áudio, tried in order as Yomitan
// tries its audio sources:
//   local-db     a local audio database (local-audio-yomichan's entries.db or android.db, the file
//                Yomitan's local audio and Android apps use), read by the backend (audio.rs)
//   custom       Yomitan's "Custom URL": a URL template that answers with the audio itself
//   custom-json  Yomitan's "Custom URL (JSON)": answers {"type": "audioSourceList",
//                "audioSources": [{"name", "url"}]}
//   tts          the system voice (speechSynthesis), which always "finds" something
// Templates take {term}, {reading} and {language}, as in Yomitan.

export const AUDIO_SOURCE_TYPES = [
    {value: 'local-db', label: 'Banco de áudio local (.db)'},
    {value: 'custom', label: 'URL personalizada'},
    {value: 'custom-json', label: 'URL personalizada (JSON)'},
    {value: 'tts', label: 'Voz do sistema'},
];

const TYPES = new Set(AUDIO_SOURCE_TYPES.map((t) => t.value));
const REQUEST_TIMEOUT_MS = 10000;

export function defaultAudioOptions() {
    // others: how the pop-up shows the recordings after the first, 'menu' (right click on the
    // speaker) or 'buttons' (one button each).
    return {autoPlay: false, others: 'menu', sources: [{type: 'tts', enabled: true, path: '', url: '', subsources: []}]};
}

/**
 * A local database's own sources (nhk16, forvo…) in the user's order, merged with those the file
 * has: kept ones stay in place, new ones are added at the end (switched on), gone ones dropped.
 */
export function mergeSubsources(saved, inFile) {
    const names = new Set(inFile);
    const kept = (saved || []).filter((s) => names.has(s.name));
    const known = new Set(kept.map((s) => s.name));
    return [...kept, ...inFile.filter((n) => !known.has(n)).map((name) => ({name, enabled: true}))];
}

/** Stored audio options with defaults; unknown source types are dropped. */
export function normalizeAudioOptions(stored) {
    const base = defaultAudioOptions();
    if (typeof stored !== 'object' || stored === null || Array.isArray(stored)) { return base; }
    const sources = Array.isArray(stored.sources) ? stored.sources
        .filter((s) => s && TYPES.has(s.type))
        .map((s) => ({type: s.type, enabled: s.enabled !== false, path: String(s.path || ''), url: String(s.url || ''),
            subsources: Array.isArray(s.subsources) ? s.subsources.filter((x) => x && typeof x.name === 'string' && x.name)
                .map((x) => ({name: x.name, enabled: x.enabled !== false})) : []})) : base.sources;
    return {autoPlay: stored.autoPlay === true, others: stored.others === 'buttons' ? 'buttons' : 'menu', sources};
}

/** A Yomitan URL template with the word filled in (each value URL-encoded). */
export function fillAudioUrl(template, {term = '', reading = '', language = ''} = {}) {
    return String(template)
        .replaceAll('{term}', encodeURIComponent(term))
        .replaceAll('{reading}', encodeURIComponent(reading || term))
        .replaceAll('{language}', encodeURIComponent(language));
}

/** The recordings in a "Custom URL (JSON)" answer (Yomitan's customAudioList schema). */
export function parseAudioList(data) {
    if (!data || data.type !== 'audioSourceList' || !Array.isArray(data.audioSources)) { return []; }
    return data.audioSources
        .filter((s) => s && typeof s.url === 'string' && /^https?:\/\//i.test(s.url))
        .map((s) => ({name: typeof s.name === 'string' && s.name ? s.name : '', url: s.url}));
}

function hostOf(url) {
    try { return new URL(url).host; } catch (_e) { return url; }
}

/**
 * Every recording found for a word, in the order of the sources (Configurações › Áudio). Each one
 * is {name, kind: 'db' | 'url' | 'tts', ...}; a source that fails is skipped.
 * @param {{invoke: Function, fetch?: Function}} io
 */
export async function findAudio(options, word, io) {
    const out = [];
    for (const source of normalizeAudioOptions(options).sources) {
        if (!source.enabled) { continue; }
        try {
            if (source.type === 'local-db' && source.path) {
                // The file's own sources in the user's order; none chosen yet means all, default order.
                const order = source.subsources.length ? source.subsources.filter((s) => s.enabled).map((s) => s.name) : null;
                if (order && order.length === 0) { continue; }
                const rows = await io.invoke('audio_lookup', {path: source.path, term: word.term, reading: word.reading || null, order});
                for (const row of rows || []) { out.push({kind: 'db', name: row.name, path: source.path, source: row.source, file: row.file}); }
            } else if (source.type === 'custom' && source.url) {
                const url = fillAudioUrl(source.url, word);
                out.push({kind: 'url', name: hostOf(url), url});
            } else if (source.type === 'custom-json' && source.url) {
                const response = await request(io, fillAudioUrl(source.url, word));
                if (response.ok) {
                    for (const item of parseAudioList(await response.json())) { out.push({kind: 'url', name: item.name || hostOf(item.url), url: item.url}); }
                }
            } else if (source.type === 'tts') {
                out.push({kind: 'tts', name: 'Voz do sistema'});
            }
        } catch (_e) { /* an unreachable source is skipped, as in Yomitan */ }
    }
    return out;
}

async function request(io, url) {
    const doFetch = io.fetch || (await import('@tauri-apps/plugin-http')).fetch;
    const timeout = new AbortController();
    const timer = setTimeout(() => timeout.abort(), REQUEST_TIMEOUT_MS);
    try { return await doFetch(url, {method: 'GET', signal: timeout.signal}); } finally { clearTimeout(timer); }
}

const EXTENSIONS = {'audio/mpeg': 'mp3', 'audio/mp3': 'mp3', 'audio/ogg': 'ogg', 'audio/opus': 'ogg', 'audio/mp4': 'm4a', 'audio/aac': 'aac', 'audio/x-m4a': 'm4a', 'audio/wav': 'wav', 'audio/x-wav': 'wav', 'audio/flac': 'flac', 'audio/webm': 'webm'};
const MIME = {mp3: 'audio/mpeg', ogg: 'audio/ogg', m4a: 'audio/mp4', aac: 'audio/aac', wav: 'audio/wav', flac: 'audio/flac', webm: 'audio/webm'};

function toBase64(bytes) {
    let binary = '';
    for (let i = 0; i < bytes.length; i += 0x8000) { binary += String.fromCharCode.apply(null, bytes.subarray(i, i + 0x8000)); }
    return btoa(binary);
}

/**
 * A recording's file: {data (base64), extension, mime}, or null when it cannot be read or is not
 * audio. URLs go through the app (no CORS, and http works from both platforms' WebView origins).
 */
export async function audioFile(entry, io) {
    if (entry.kind === 'db') {
        const found = await io.invoke('audio_data', {path: entry.path, source: entry.source, file: entry.file});
        if (!found) { return null; }
        const [data, extension] = found;
        return {data, extension, mime: MIME[extension] || 'audio/mpeg'};
    }
    if (entry.kind === 'url') {
        const response = await request(io, entry.url);
        const type = (response.headers?.get?.('content-type') || '').split(';')[0].trim().toLowerCase();
        if (!response.ok || (type && !type.startsWith('audio/') && type !== 'application/octet-stream')) { return null; }
        const bytes = new Uint8Array(await response.arrayBuffer());
        if (bytes.length === 0) { return null; }
        const extension = EXTENSIONS[type] || (entry.url.match(/\.(mp3|ogg|oga|opus|m4a|aac|wav|flac)(?:[?#]|$)/i)?.[1].toLowerCase().replace(/^(oga|opus)$/, 'ogg')) || 'mp3';
        return {data: toBase64(bytes), extension, mime: MIME[extension] || type || 'audio/mpeg'};
    }
    return null;
}
