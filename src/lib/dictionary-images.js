// Picture dictionaries (an MDict whose entries are only pictures, such as PicDic): the pop-up shows
// their entries in the same strip as the "Imagens" section, instead of the entry's own page.
import {encodeImage} from './images.js';

const IMAGE_TYPES = {
    png: 'image/png', jpg: 'image/jpeg', jpeg: 'image/jpeg', gif: 'image/gif', webp: 'image/webp',
    avif: 'image/avif', bmp: 'image/bmp', svg: 'image/svg+xml',
};

const ENTITIES = {amp: '&', lt: '<', gt: '>', quot: '"', apos: "'", nbsp: ' '};

function unescapeHtml(text) {
    return String(text).replace(/&(#x[0-9a-f]+|#\d+|[a-z]+);/gi, (whole, name) => {
        if (name[0] === '#') {
            const code = name[1] === 'x' || name[1] === 'X' ? parseInt(name.slice(2), 16) : parseInt(name.slice(1), 10);
            return Number.isFinite(code) && code > 0 && code <= 0x10ffff ? String.fromCodePoint(code) : whole;
        }
        return ENTITIES[name.toLowerCase()] ?? whole;
    });
}

function attribute(tag, name) {
    const m = new RegExp(`\\s${name}\\s*=\\s*(?:"([^"]*)"|'([^']*)'|([^\\s>]+))`, 'i').exec(tag);
    return m ? unescapeHtml(m[1] ?? m[2] ?? m[3] ?? '').trim() : '';
}

/** Letters and digits only, lower case: what is left of a caption to compare with the headword. */
const letters = (text) => String(text).toLowerCase().replace(/[^\p{L}\p{N}]/gu, '');

/**
 * The pictures of an entry made only of pictures, in order, each once; null for any other entry.
 * An entry is made of pictures when, besides them, it has no letters or digits (PicDic puts a "-"
 * under each picture) or only its own headword. An entry with sound (a speaker icon) is not.
 * @param {string} html The entry's HTML, as `entry_html` gives it.
 * @param {string} [headword]
 * @returns {{src: string, title: string}[]|null}
 */
export function entryPictures(html, headword = '') {
    const body = String(html || '')
        .replace(/<!--[\s\S]*?-->/g, '')
        .replace(/<(script|style|head|title)\b[\s\S]*?<\/\1\s*>/gi, '');
    if (/sound:\/\/|<audio\b|<video\b/i.test(body)) { return null; }
    const pictures = [];
    const seen = new Set();
    for (const [tag] of body.matchAll(/<img\b[^>]*>/gi)) {
        const src = attribute(tag, 'src');
        if (!src || seen.has(src)) { continue; }
        seen.add(src);
        pictures.push({src, title: attribute(tag, 'alt') || attribute(tag, 'title')});
    }
    if (pictures.length === 0) { return null; }
    const text = letters(unescapeHtml(body.replace(/<[^>]*>/g, ' ')));
    return text === '' || (text === letters(headword) && text !== '') ? pictures : null;
}

/**
 * Where the pop-up loads a picture of an entry, the way the entry's own page would (its <base> is
 * the dictionary's resource route): `path` is the file inside the dictionary, or null for a picture
 * on the web or in a data: URL. Null for a picture the pop-up cannot load (file:, a path out of
 * the dictionary).
 * @param {string} base The `lkt` scheme's base URL (schemeBase()).
 * @param {string} dictId
 * @param {string} src
 * @returns {{url: string, path: string|null}|null}
 */
export function pictureSource(base, dictId, src) {
    const value = String(src || '').trim().replace(/\\/g, '/');
    if (/^data:image\//i.test(value) || /^https?:\/\//i.test(value)) { return {url: value, path: null}; }
    if (value.startsWith('//')) { return {url: `https:${value}`, path: null}; }
    if (/^[a-z][a-z0-9+.-]*:/i.test(value)) { return null; }
    let path = value.split(/[?#]/)[0].replace(/^(\.\/)+/, '').replace(/^\/+/, '');
    try { path = decodeURIComponent(path); } catch (_e) { /* a stray % stays as it is */ }
    if (!path || path.split('/').includes('..')) { return null; }
    return {url: `${base}/d/${encodeURIComponent(dictId)}/${path.split('/').map(encodeURIComponent).join('/')}`, path};
}

/** The picture's type from its file name. */
export function pictureType(path) {
    return IMAGE_TYPES[String(path).split('.').pop().toLowerCase()] || '';
}

/** The type of a picture whose file name does not say it, from its first bytes. */
export function sniffType(bytes) {
    const at = (i, ...values) => values.every((v, k) => bytes[i + k] === v);
    if (at(0, 0x89, 0x50, 0x4e, 0x47)) { return 'image/png'; }
    if (at(0, 0xff, 0xd8, 0xff)) { return 'image/jpeg'; }
    if (at(0, 0x47, 0x49, 0x46, 0x38)) { return 'image/gif'; }
    if (at(0, 0x52, 0x49, 0x46, 0x46) && at(8, 0x57, 0x45, 0x42, 0x50)) { return 'image/webp'; }
    if (at(0, 0x42, 0x4d)) { return 'image/bmp'; }
    return '';
}

/**
 * A dictionary picture as a file for {image}: read from the dictionary through `read`
 * (`api.dictionaryResource`), or fetched when it lives on the web or in a data: URL.
 * @param {{src: string, path: string|null, dictId: string}} item
 * @param {(dictId: string, path: string) => Promise<ArrayBuffer|number[]>} read
 * @param {(item: {src: string, thumb: string}) => Promise<{data: string, extension: string}|null>} download
 * @returns {Promise<{data: string, extension: string}|null>}
 */
export async function dictionaryPictureFile(item, read, download) {
    if (!item.path) {
        if (/^data:/i.test(item.src)) {
            const response = await fetch(item.src);
            const type = (response.headers.get('content-type') || '').split(';')[0].trim().toLowerCase();
            return encodeImage(await response.blob(), type);
        }
        return download({src: item.src, thumb: item.src});
    }
    const raw = await read(item.dictId, item.path);
    const bytes = raw instanceof ArrayBuffer ? new Uint8Array(raw) : new Uint8Array(raw);
    const type = pictureType(item.path) || sniffType(bytes);
    return type ? encodeImage(new Blob([bytes], {type}), type) : null;
}
