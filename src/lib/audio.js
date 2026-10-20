// Word audio, from the sources the user lists in Configurações › Áudio, tried in order as Yomitan
// tries its audio sources (the same source types as Yomitan's audio-downloader, plus local-db):
//   local-db          a local audio database (local-audio-yomichan's entries.db or android.db, the
//                     file Yomitan's local audio and Android apps use), read by the backend (audio.rs)
//   jpod101           JapanesePod101's audiomp3.php (Japanese only)
//   language-pod-101  the LanguagePod101 / ClassPod101 dictionary of the word's language
//   jisho             the recording on the word's Jisho.org page (Japanese only)
//   lingua-libre      Lingua Libre recordings on Wikimedia Commons
//   wiktionary        Wiktionary recordings on Wikimedia Commons
//   custom            Yomitan's "Custom URL": a URL template that answers with the audio itself
//   custom-json       Yomitan's "Custom URL (JSON)": answers {"type": "audioSourceList",
//                     "audioSources": [{"name", "url"}]}
//   tts               the system voice (speechSynthesis), which always "finds" something
// Templates take {term}, {reading} and {language}, as in Yomitan. Every request goes through the app
// (no CORS), and the sources are asked at the same time; the results keep the list order.

export const AUDIO_SOURCE_TYPES = [
    {value: 'local-db', label: 'Banco de áudio local (.db)'},
    {value: 'jpod101', label: 'JapanesePod101'},
    {value: 'language-pod-101', label: 'LanguagePod101'},
    {value: 'jisho', label: 'Jisho.org'},
    {value: 'lingua-libre', label: 'Lingua Libre'},
    {value: 'wiktionary', label: 'Wiktionary'},
    {value: 'custom', label: 'URL personalizada'},
    {value: 'custom-json', label: 'URL personalizada (JSON)'},
    {value: 'tts', label: 'Voz do sistema'},
];

const TYPES = new Set(AUDIO_SOURCE_TYPES.map((t) => t.value));
const REQUEST_TIMEOUT_MS = 10000;
// Wikimedia asks API clients to name themselves.
const WIKIMEDIA_HEADERS = {'User-Agent': 'Lookitan (https://github.com/arrow-poppa/lookitan)'};
const KANA = /^[぀-ゟ゠-ヿ]+$/;
// JapanesePod101's "the audio for this clip is currently not available" recording (as in Yomitan).
const JPOD101_MISSING_SHA256 = 'ae6398b5a27bc8c0a771df6c907ade794be15518174773c58c7c7ddd17098906';

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

function escapeRegex(text) {
    return String(text).replace(/[.*+?^${}()|[\]\\/]/g, '\\$&');
}

function decodeEntities(text) {
    return text.replace(/&(#x[0-9a-f]+|#\d+|amp|lt|gt|quot|apos);/gi, (m, e) => {
        const named = {amp: '&', lt: '<', gt: '>', quot: '"', apos: "'"}[e.toLowerCase()];
        if (named) { return named; }
        const code = e[1] === 'x' || e[1] === 'X' ? parseInt(e.slice(2), 16) : parseInt(e.slice(1), 10);
        return Number.isFinite(code) ? String.fromCodePoint(code) : m;
    });
}

/** The text of the first element with class `name` in `html` (up to its first closing tag). */
function textOfClass(html, name) {
    const m = html.match(new RegExp(`class=["'](?:[^"']*\\s)?${escapeRegex(name)}(?:\\s[^"']*)?["'][^>]*>([\\s\\S]*?)</`, 'i'));
    return m ? decodeEntities(m[1].replace(/<[^>]*>/g, '')).trim() : null;
}

function sourceSrc(html) {
    const m = html.match(/<source\b[^>]*\bsrc=["']([^"']+)["']/i);
    return m ? decodeEntities(m[1]) : null;
}

/** JapanesePod101's recording URL: a word written in kana alone goes as the reading. */
export function jpod101Url(term, reading = '') {
    const params = new URLSearchParams();
    if (!(KANA.test(term) && (!reading || reading === term))) { params.set('kanji', term); }
    if (reading || KANA.test(term)) { params.set('kana', reading || term); }
    return `https://assets.languagepod101.com/dictionary/japanese/audiomp3.php?${params}`;
}

// Jisho.org answers 403 to clients that do not look like a browser.
const BROWSER_HEADERS = {'User-Agent': 'Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/130.0.0.0 Safari/537.36'};
const readings = new Map();
const READINGS_KEPT = 200;

/**
 * The kana reading of a Japanese word, from Jisho.org's search API (exact matches only). The
 * pop-up has no reading, and JapanesePod101 only finds a word written in kanji with its reading.
 */
export async function japaneseReading(io, term) {
    if (KANA.test(term)) { return term; }
    if (readings.has(term)) { return readings.get(term); }
    const response = await request(io, `https://jisho.org/api/v1/search/words?keyword=${encodeURIComponent(term)}`, {headers: BROWSER_HEADERS});
    const data = response.ok ? await response.json() : null;
    const forms = (data?.data || []).flatMap((d) => (Array.isArray(d?.japanese) ? d.japanese : []));
    const reading = forms.find((f) => f?.word === term && typeof f.reading === 'string')?.reading || '';
    if (readings.size >= READINGS_KEPT) { readings.delete(readings.keys().next().value); }
    readings.set(term, reading);
    return reading;
}

const POD101 = new Set(['Afrikaans', 'Arabic', 'Bulgarian', 'Dutch', 'Filipino', 'Finnish', 'French', 'German', 'Greek', 'Hebrew', 'Hindi', 'Hungarian',
    'Indonesian', 'Italian', 'Japanese', 'Persian', 'Polish', 'Portuguese', 'Romanian', 'Russian', 'Spanish', 'Swahili', 'Swedish', 'Thai', 'Urdu', 'Vietnamese']);
const CLASS101 = new Set(['Cantonese', 'Chinese', 'Czech', 'Danish', 'English', 'Korean', 'Norwegian', 'Turkish']);

function englishLanguageName(iso) {
    try { return new Intl.DisplayNames(['en'], {type: 'language'}).of(iso) || ''; } catch (_e) { return ''; }
}

/** The LanguagePod101 (or ClassPod101) dictionary for a language, or null when it has none. */
export function languagePod101Url(iso) {
    const name = {fil: 'Filipino', tl: 'Filipino', nb: 'Norwegian', nn: 'Norwegian', yue: 'Cantonese'}[iso] || englishLanguageName(iso);
    const kind = POD101.has(name) ? 'pod' : CLASS101.has(name) ? 'class' : null;
    return kind ? {name, url: `https://www.${name.toLowerCase()}${kind}101.com/learningcenter/reference/dictionary_post`} : null;
}

/** The recordings of a LanguagePod101 dictionary answer that are for this word. */
export function parseLanguagePod101(html, base, language, term, reading = '') {
    const urls = new Set();
    for (const row of String(html).split(/<[^>]+class=["'](?:[^"']*\s)?dc-result-row(?:\s[^"']*)?["'][^>]*>/i).slice(1)) {
        const audio = row.match(/<audio\b[\s\S]*?<\/audio>/i);
        const src = audio ? sourceSrc(audio[0]) : null;
        if (!src) { continue; }
        if (language === 'Japanese') {
            const kana = textOfClass(row, 'dc-vocab_kana');
            if (!kana || (reading && reading !== term && reading !== kana)) { continue; }
        } else if (textOfClass(row, 'dc-vocab') !== term) {
            continue;
        }
        try { urls.add(new URL(src, base).href); } catch (_e) { /* not a URL */ }
    }
    return [...urls];
}

/** The recording on a Jisho.org search page: <audio id="audio_<term>:<reading>"><source src>. */
export function parseJisho(html, base, term, reading = '') {
    const id = `audio_${escapeRegex(term)}:${reading ? escapeRegex(reading) : '[^"\']*'}`;
    const audio = String(html).match(new RegExp(`<audio\\b[^>]*\\bid=["']${id}["'][^>]*>([\\s\\S]*?)</audio>`, 'i'));
    const src = audio ? sourceSrc(audio[1]) : null;
    try { return src ? new URL(src, base).href : null; } catch (_e) { return null; }
}

// ISO 639-3 codes of Lingua Libre's categories ("Lingua_Libre_pronunciation-<code>").
const ISO639_3 = {ar: 'ara', bg: 'bul', bn: 'ben', ca: 'cat', cs: 'ces', cy: 'cym', da: 'dan', de: 'deu', el: 'ell', en: 'eng', eo: 'epo', es: 'spa', et: 'est',
    eu: 'eus', fa: 'fas', fi: 'fin', fr: 'fra', ga: 'gle', gl: 'glg', he: 'heb', hi: 'hin', hr: 'hrv', hu: 'hun', hy: 'hye', id: 'ind', is: 'isl', it: 'ita',
    ja: 'jpn', ka: 'kat', km: 'khm', ko: 'kor', la: 'lat', lt: 'lit', lv: 'lav', mn: 'mon', ms: 'msa', nb: 'nob', nl: 'nld', nn: 'nno', no: 'nor', pl: 'pol',
    pt: 'por', ro: 'ron', ru: 'rus', sk: 'slk', sl: 'slv', sq: 'sqi', sr: 'srp', sv: 'swe', sw: 'swa', ta: 'tam', th: 'tha', tl: 'tgl', tr: 'tur', uk: 'ukr',
    ur: 'urd', vi: 'vie', yue: 'yue', zh: 'zho'};

function regionName(code) {
    try { return new Intl.DisplayNames(['pt'], {type: 'region'}).of(code) || code; } catch (_e) { return code; }
}

const COMMONS_API = 'https://commons.wikimedia.org/w/api.php';

/** Files on Wikimedia Commons whose title matches `search`, checked with `valid(title, user)`. */
async function wikimediaCommons(io, search, valid, name) {
    const query = new URLSearchParams({action: 'query', format: 'json', list: 'search', srsearch: search, srnamespace: '6', origin: '*'});
    const found = await (await request(io, `${COMMONS_API}?${query}`, {headers: WIKIMEDIA_HEADERS})).json();
    const titles = (found?.query?.search || []).map((r) => r?.title).filter((t) => typeof t === 'string').slice(0, 10);
    const files = await Promise.all(titles.map(async (title) => {
        const info = new URLSearchParams({action: 'query', format: 'json', titles: title, prop: 'imageinfo', iiprop: 'user|url', origin: '*'});
        const pages = (await (await request(io, `${COMMONS_API}?${info}`, {headers: WIKIMEDIA_HEADERS})).json())?.query?.pages || {};
        return Object.values(pages).flatMap((page) => {
            const file = page?.imageinfo?.[0];
            const user = String(file?.user || '');
            return file && typeof file.url === 'string' && valid(title, user) ? [{kind: 'url', name: name(title, user), url: file.url}] : [];
        });
    }));
    return files.flat();
}

async function linguaLibre(io, term, iso) {
    const iso3 = ISO639_3[iso];
    if (!iso3) { return []; }
    const word = escapeRegex(term);
    return wikimediaCommons(io, `intitle:/-${word}\\.wav/i incategory:"Lingua_Libre_pronunciation-${iso3}"`,
        (title, user) => new RegExp(`^File:LL-Q\\d+\\s+\\(${iso3}\\)-${escapeRegex(user)}-${word}\\.wav$`, 'i').test(title),
        (_title, user) => `Lingua Libre · ${user}`);
}

async function wiktionary(io, term, iso) {
    if (!/^[a-z]{2,3}$/.test(iso)) { return []; }
    const word = escapeRegex(term);
    return wikimediaCommons(io, `intitle:/${iso}(-[a-zA-Z]{2})?-${word}[0123456789]*\\.ogg/i`,
        (title) => new RegExp(`^File:${iso}(-\\w\\w)?-${word}\\d*\\.ogg$`, 'i').test(title),
        (title, user) => {
            const region = title.match(new RegExp(`^File:${iso}-(\\w\\w)-`, 'i'));
            return `Wiktionary · ${region ? `(${regionName(region[1].toUpperCase())}) ` : ''}${user}`;
        });
}

/** The recordings one source has for a word. */
async function fromSource(source, word, io) {
    const {term} = word;
    const reading = word.reading || '';
    const language = word.language || '';
    switch (source.type) {
        case 'local-db': {
            // The file's own sources in the user's order; none chosen yet means all, default order.
            const order = source.subsources.length ? source.subsources.filter((s) => s.enabled).map((s) => s.name) : null;
            if (!source.path || (order && order.length === 0)) { return []; }
            const rows = await io.invoke('audio_lookup', {path: source.path, term, reading: reading || null, order});
            return (rows || []).map((row) => ({kind: 'db', name: row.name, path: source.path, source: row.source, file: row.file}));
        }
        case 'jpod101': {
            if (language !== 'ja') { return []; }
            const kana = reading || await japaneseReading(io, term).catch(() => '');
            return [{kind: 'url', name: 'JapanesePod101', url: jpod101Url(term, kana), check: 'jpod101'}];
        }
        case 'language-pod-101': {
            const dictionary = languagePod101Url(language);
            if (!dictionary) { return []; }
            const body = new URLSearchParams({post: 'dictionary_reference', match_type: 'exact', search_query: term, vulgar: 'true'}).toString();
            const response = await request(io, dictionary.url, {method: 'POST', headers: {'Content-Type': 'application/x-www-form-urlencoded'}, body});
            if (!response.ok) { return []; }
            return parseLanguagePod101(await response.text(), response.url || dictionary.url, dictionary.name, term, reading)
                .map((url) => ({kind: 'url', name: 'LanguagePod101', url}));
        }
        case 'jisho': {
            if (language !== 'ja') { return []; }
            const page = `https://jisho.org/search/${encodeURIComponent(term)}`;
            const response = await request(io, page, {headers: BROWSER_HEADERS});
            const url = response.ok ? parseJisho(await response.text(), response.url || page, term, reading) : null;
            return url ? [{kind: 'url', name: 'Jisho.org', url}] : [];
        }
        case 'lingua-libre':
            return linguaLibre(io, term, language);
        case 'wiktionary':
            return wiktionary(io, term, language);
        case 'custom': {
            if (!source.url) { return []; }
            const url = fillAudioUrl(source.url, word);
            return [{kind: 'url', name: hostOf(url), url}];
        }
        case 'custom-json': {
            if (!source.url) { return []; }
            const response = await request(io, fillAudioUrl(source.url, word));
            if (!response.ok) { return []; }
            return parseAudioList(await response.json()).map((item) => ({kind: 'url', name: item.name || hostOf(item.url), url: item.url}));
        }
        case 'tts':
            return [{kind: 'tts', name: 'Voz do sistema'}];
        default:
            return [];
    }
}

/**
 * Every recording found for a word, in the order of the sources (Configurações › Áudio). Each one
 * is {name, kind: 'db' | 'url' | 'tts', ...}; a source that fails is skipped, as in Yomitan. The
 * sources are asked at the same time, so a slow site does not hold up the ones below it.
 * @param {{invoke: Function, fetch?: Function}} io
 */
export async function findAudio(options, word, io) {
    const sources = normalizeAudioOptions(options).sources.filter((s) => s.enabled);
    const lists = await Promise.all(sources.map((source) => fromSource(source, word, io).catch(() => [])));
    return lists.flat();
}

async function request(io, url, init = {}) {
    const doFetch = io.fetch || (await import('@tauri-apps/plugin-http')).fetch;
    const timeout = new AbortController();
    const timer = setTimeout(() => timeout.abort(), REQUEST_TIMEOUT_MS);
    try { return await doFetch(url, {method: 'GET', ...init, signal: timeout.signal}); } finally { clearTimeout(timer); }
}

const EXTENSIONS = {'audio/mpeg': 'mp3', 'audio/mp3': 'mp3', 'audio/ogg': 'ogg', 'audio/opus': 'ogg', 'audio/mp4': 'm4a', 'audio/aac': 'aac', 'audio/x-m4a': 'm4a',
    'audio/wav': 'wav', 'audio/x-wav': 'wav', 'audio/wave': 'wav', 'audio/vnd.wave': 'wav', 'audio/flac': 'flac', 'audio/webm': 'webm', 'application/ogg': 'ogg'};
const MIME = {mp3: 'audio/mpeg', ogg: 'audio/ogg', m4a: 'audio/mp4', aac: 'audio/aac', wav: 'audio/wav', flac: 'audio/flac', webm: 'audio/webm'};

function toBase64(bytes) {
    let binary = '';
    for (let i = 0; i < bytes.length; i += 0x8000) { binary += String.fromCharCode.apply(null, bytes.subarray(i, i + 0x8000)); }
    return btoa(binary);
}

async function sha256(bytes) {
    const digest = new Uint8Array(await globalThis.crypto.subtle.digest('SHA-256', bytes));
    return Array.from(digest, (b) => b.toString(16).padStart(2, '0')).join('');
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
        if (!response.ok || (type && !type.startsWith('audio/') && type !== 'application/octet-stream' && type !== 'application/ogg')) { return null; }
        const bytes = new Uint8Array(await response.arrayBuffer());
        if (bytes.length === 0) { return null; }
        // JapanesePod101 answers a missing word with a "not available" recording instead of an error.
        if (entry.check === 'jpod101' && await sha256(bytes) === JPOD101_MISSING_SHA256) { return null; }
        const extension = EXTENSIONS[type] || (entry.url.match(/\.(mp3|ogg|oga|opus|m4a|aac|wav|flac)(?:[?#]|$)/i)?.[1].toLowerCase().replace(/^(oga|opus)$/, 'ogg')) || 'mp3';
        return {data: toBase64(bytes), extension, mime: MIME[extension] || type || 'audio/mpeg'};
    }
    return null;
}
