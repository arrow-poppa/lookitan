// Anki options (same shape as Yomitan's `anki` options) and note building with Yomitan-style
// field markers such as {expression}, {glossary}, {sentence}, {cloze-body} and {screenshot}.

import {AnkiConnect} from './anki-connect.js';
import {generateAnkiNoteMediaFileName, getFieldMarkers} from './anki-util.js';
import {markdownToText, renderMarkdown} from '../markdown.js';

export function defaultAnkiOptions() {
    return {
        enable: false,
        server: 'http://127.0.0.1:8765',
        apiKey: '',
        tags: ['lookitan'],
        screenshot: {format: 'png', quality: 92},
        screenshotArea: 'sentence',
        cardFormats: [{name: 'Expression', icon: 'big-circle', deck: '', model: '', type: 'term', fields: {}}],
        duplicateScope: 'collection',
        duplicateScopeCheckAllModels: false,
        checkForDuplicates: true,
        duplicateBehavior: 'new',
        suspendNewCards: false,
        noteGuiMode: 'browse',
    };
}

export function normalizeAnkiOptions(stored) {
    const base = defaultAnkiOptions();
    if (typeof stored !== 'object' || stored === null || Array.isArray(stored)) { return base; }
    const out = {...base, ...stored};
    out.screenshot = {...base.screenshot, ...(stored.screenshot || {})};
    if (!Array.isArray(out.cardFormats) || out.cardFormats.length === 0) { out.cardFormats = base.cardFormats; }
    out.cardFormats = out.cardFormats.map((f) => ({...base.cardFormats[0], ...f, fields: {...(f.fields || {})}}));
    return out;
}

/** Markers Lookitan can fill, in the order shown in the marker menu. */
export const MARKERS = [
    'expression', 'reading', 'glossary', 'glossary-brief', 'glossary-no-dictionary', 'glossary-plain',
    'glossary-plain-no-dictionary', 'glossary-first', 'glossary-first-brief', 'glossary-first-no-dictionary',
    'glossary-first-plain', 'sentence', 'cloze-prefix', 'cloze-body', 'cloze-suffix', 'dictionary',
    'screenshot', 'popup-selection-text', 'ai-definition', 'ai-explanation', 'ai-explanation-plain',
    'translation', 'context-translation', 'cefr', 'part-of-speech', 'tags', 'document-title', 'search-query',
];

/** Name of the AI word card when it is listed as a dictionary in glossary markers. */
export const AI_GLOSSARY_NAME = 'IA';

/** The variants of each per-dictionary marker, as in Yomitan's dynamic templates. */
const SINGLE_GLOSSARY_VARIANTS = ['', '-brief', '-no-dictionary', '-plain', '-plain-no-dictionary'];

/** Dictionary name as used in `{single-glossary-<name>}`; same rules as Yomitan's getKebabCase. */
export function getKebabCase(str) {
    return String(str)
        .replace(/[\s_\u3000]/g, '-')
        .replace(/[^\p{L}\p{N}-]/gu, '')
        .replace(/--+/g, '-')
        .replace(/^-|-$/g, '')
        .toLowerCase();
}

/**
 * `{single-glossary-…}` markers for these dictionary names (enabled MDict dictionaries and the AI
 * card, in the user's order).
 * @param {string[]} names
 */
export function dictionaryMarkers(names) {
    const out = [];
    for (const name of names) {
        const id = getKebabCase(name);
        if (!id) { continue; }
        for (const variant of SINGLE_GLOSSARY_VARIANTS) { out.push(`single-glossary-${id}${variant}`); }
    }
    return out;
}

/** Dictionary names for the glossary markers, from the settings' dictionary list. */
export function glossaryDictionaryNames(dictionaries) {
    return (dictionaries || [])
        .filter((d) => d.enabled && (d.kind === 'mdict' || d.kind === 'ai-word'))
        .map((d) => (d.kind === 'ai-word' ? AI_GLOSSARY_NAME : d.name));
}

/** Default marker for a note field, guessed from its name like Yomitan does. */
export function suggestMarker(fieldName, index) {
    const n = fieldName.toLowerCase();
    const rules = [
        [/^(front|word|expression|term|vocab|target|headword)/, '{expression}'],
        [/(reading|ipa|pronunciation|transcription|phonetic)/, '{reading}'],
        [/(cloze)/, '{cloze-prefix}<b>{cloze-body}</b>{cloze-suffix}'],
        [/(sentence|context|example)/, '{sentence}'],
        [/(screenshot|image|picture|photo)/, '{screenshot}'],
        [/(translation)/, '{translation}'],
        [/(meaning|definition|glossary|back|gloss)/, '{glossary}'],
        [/(explanation|notes?)$/, '{ai-explanation}'],
        [/(cefr|level)/, '{cefr}'],
        [/(part.?of.?speech|pos)$/, '{part-of-speech}'],
        [/(source|title)/, '{document-title}'],
    ];
    for (const [re, marker] of rules) {
        if (re.test(n)) { return marker; }
    }
    return index === 0 ? '{expression}' : (index === 1 ? '{glossary}' : '');
}

export function fieldValue(field) {
    if (typeof field === 'string') { return field; }
    return field && typeof field.value === 'string' ? field.value : '';
}

export function createConnection(anki) {
    const conn = new AnkiConnect();
    conn.server = anki.server;
    conn.enabled = Boolean(anki.enable);
    conn.apiKey = anki.apiKey ? anki.apiKey : null;
    return conn;
}

function escapeHtml(s) {
    return String(s).replace(/[&<>"]/g, (c) => ({'&': '&amp;', '<': '&lt;', '>': '&gt;', '"': '&quot;'})[c]);
}

/** Dictionary HTML made safe for a note: no scripts, styles, links or images that live in the .mdd. */
export function cleanGlossaryHtml(html) {
    const doc = new DOMParser().parseFromString(`<div>${html}</div>`, 'text/html');
    for (const el of doc.querySelectorAll('script,style,link,img,audio,video,iframe,object,embed')) { el.remove(); }
    for (const a of doc.querySelectorAll('a')) { a.replaceWith(...a.childNodes); }
    for (const el of doc.querySelectorAll('*')) {
        for (const attr of [...el.attributes]) {
            if (attr.name.startsWith('on') || attr.name === 'href' || attr.name === 'src') { el.removeAttribute(attr.name); }
        }
    }
    return doc.body.firstElementChild ? doc.body.firstElementChild.innerHTML.trim() : '';
}

function htmlToText(html) {
    const doc = new DOMParser().parseFromString(html, 'text/html');
    return (doc.body.textContent || '').replace(/\s+/g, ' ').trim();
}

/**
 * Glossary HTML in Yomitan's layout: one definition in a div, several in an ordered list. Each
 * definition starts with "(tags, dictionary)"; `noDictionary` leaves out the dictionary and
 * `brief` the whole parenthesis.
 */
function glossaryHtml(defs, {brief = false, noDictionary = false} = {}) {
    const items = defs.map((d) => {
        const tags = brief ? [] : [...d.tags, ...(noDictionary ? [] : [d.dictionary])];
        return `${tags.length ? `<i>(${tags.map(escapeHtml).join(', ')})</i> ` : ''}${d.html}`;
    });
    if (items.length === 0) { return ''; }
    if (items.length === 1) { return `<div style="text-align: left;">${items[0]}</div>`; }
    return `<div style="text-align: left;"><ol>${items.map((i) => `<li>${i}</li>`).join('')}</ol></div>`;
}

function glossaryPlain(defs, {noDictionary = false} = {}) {
    return defs.map((d) => escapeHtml(`${noDictionary ? '' : `(${d.dictionary}) `}${d.text}`)).join('<br>');
}

/**
 * The definitions of a lookup, one per dictionary, in the user's dictionary order. The AI card
 * counts as the dictionary AI_GLOSSARY_NAME, with its part of speech and labels as tags.
 */
function definitions(clean, ai, order) {
    const defs = clean.map((g) => ({dictionary: g.name, tags: [], html: g.html, text: htmlToText(g.html)}));
    if (ai.definition) {
        defs.push({
            dictionary: AI_GLOSSARY_NAME,
            tags: [...(ai.partOfSpeech || []), ...(ai.labels || [])],
            html: escapeHtml(ai.definition),
            text: ai.definition,
        });
    }
    const rank = (name) => {
        const i = (order || []).indexOf(name);
        if (i >= 0) { return i; }
        // Without an order (older saved words) the AI card comes first, as before.
        return name === AI_GLOSSARY_NAME ? -1 : Number.MAX_SAFE_INTEGER;
    };
    return defs.map((d, i) => ({d, i})).sort((a, b) => (rank(a.d.dictionary) - rank(b.d.dictionary)) || (a.i - b.i)).map(({d}) => d);
}

function cloze(sentence, word) {
    if (!sentence || !word) { return {prefix: '', body: word || '', suffix: ''}; }
    const i = sentence.toLowerCase().indexOf(word.toLowerCase());
    if (i < 0) { return {prefix: sentence, body: '', suffix: ''}; }
    return {prefix: sentence.slice(0, i), body: sentence.slice(i, i + word.length), suffix: sentence.slice(i + word.length)};
}

/**
 * @typedef {object} NoteContext
 * @property {string} word Word as read on screen.
 * @property {string} [headword] Dictionary form.
 * @property {string} [sentence]
 * @property {{name: string, html: string}[]} [glossaries]
 * @property {{ipa?: string, cefr?: string, partOfSpeech?: string[], labels?: string[], definition?: string, translation?: string}} [aiCard]
 * @property {string} [aiExplanation]
 * @property {string} [contextTranslation]
 * @property {() => Promise<string|null>} [screenshot] Base64 PNG of the capture.
 * @property {string} [title] Source window title.
 * @property {string} [query]
 * @property {string} [popupSelection] Text selected in the pop-up when the note was added.
 * @property {string[]} [dictionaryOrder] Dictionary names in the user's order (see glossaryDictionaryNames).
 */

/**
 * Builds the AnkiConnect note for the first card format. Media is uploaded only for markers used.
 * @param {NoteContext} ctx
 * @param {ReturnType<typeof defaultAnkiOptions>} anki
 * @param {AnkiConnect|null} conn Pass null to build without uploading media (duplicate checks).
 */
export async function buildNote(ctx, anki, conn) {
    const format = anki.cardFormats[0];
    const glossaries = ctx.glossaries || [];
    const clean = glossaries.map((g) => ({name: g.name, html: cleanGlossaryHtml(g.html)}));
    const ai = ctx.aiCard || {};
    const defs = definitions(clean, ai, ctx.dictionaryOrder);
    const first = defs.slice(0, 1);
    const c = cloze(ctx.sentence || '', ctx.word);
    const values = {
        'expression': escapeHtml(ctx.headword || ctx.word),
        'reading': escapeHtml(ai.ipa || ''),
        'glossary': glossaryHtml(defs),
        'glossary-brief': glossaryHtml(defs, {brief: true}),
        'glossary-no-dictionary': glossaryHtml(defs, {noDictionary: true}),
        'glossary-plain': glossaryPlain(defs),
        'glossary-plain-no-dictionary': glossaryPlain(defs, {noDictionary: true}),
        'glossary-first': glossaryHtml(first),
        'glossary-first-brief': glossaryHtml(first, {brief: true}),
        'glossary-first-no-dictionary': glossaryHtml(first, {noDictionary: true}),
        'glossary-first-plain': glossaryPlain(first, {noDictionary: true}),
        'popup-selection-text': escapeHtml(ctx.popupSelection || ''),
        'sentence': escapeHtml(ctx.sentence || ''),
        'cloze-prefix': escapeHtml(c.prefix),
        'cloze-body': escapeHtml(c.body),
        'cloze-suffix': escapeHtml(c.suffix),
        'dictionary': escapeHtml(first[0] ? first[0].dictionary : ''),
        'ai-definition': escapeHtml(ai.definition || ''),
        'ai-explanation': renderMarkdown(ctx.aiExplanation || ''),
        'ai-explanation-plain': escapeHtml(markdownToText(ctx.aiExplanation || '')).replace(/\n/g, '<br>'),
        'translation': escapeHtml(ai.translation || ''),
        'context-translation': escapeHtml((ctx.contextTranslation || '').replace(/\*\*/g, '')),
        'cefr': escapeHtml(ai.cefr || ''),
        'part-of-speech': escapeHtml((ai.partOfSpeech || []).join(', ')),
        'tags': escapeHtml([...(ai.labels || [])].join(', ')),
        'document-title': escapeHtml(ctx.title || ''),
        'search-query': escapeHtml(ctx.query || ctx.word),
        'screenshot': '',
    };
    for (const name of new Set(defs.map((d) => d.dictionary))) {
        const id = getKebabCase(name);
        if (!id) { continue; }
        const own = defs.filter((d) => d.dictionary === name);
        values[`single-glossary-${id}`] = glossaryHtml(own);
        values[`single-glossary-${id}-brief`] = glossaryHtml(own, {brief: true});
        values[`single-glossary-${id}-no-dictionary`] = glossaryHtml(own, {noDictionary: true});
        values[`single-glossary-${id}-plain`] = glossaryPlain(own);
        values[`single-glossary-${id}-plain-no-dictionary`] = glossaryPlain(own, {noDictionary: true});
    }

    const fields = {};
    let usesScreenshot = false;
    for (const [name, field] of Object.entries(format.fields)) {
        const template = fieldValue(field);
        if (getFieldMarkers(template).includes('screenshot')) { usesScreenshot = true; }
        fields[name] = template;
    }
    if (usesScreenshot && conn && ctx.screenshot) {
        const data = await ctx.screenshot();
        if (data) {
            const fileName = generateAnkiNoteMediaFileName('lookitan_screenshot', '.png', Date.now());
            const stored = await conn.storeMediaFile(fileName, data);
            values.screenshot = `<img src="${escapeHtml(stored || fileName)}">`;
        }
    }
    for (const name of Object.keys(fields)) {
        // A dictionary marker for a dictionary with no entry for this word becomes empty, as in Yomitan.
        fields[name] = fields[name].replace(/\{([\p{Letter}\p{Number}_-]+)\}/gu, (m, marker) =>
            (marker in values ? values[marker] : (marker.startsWith('single-glossary-') ? '' : m)));
    }

    return {
        deckName: format.deck,
        modelName: format.model,
        fields,
        tags: Array.isArray(anki.tags) ? anki.tags : [],
        options: {
            allowDuplicate: anki.duplicateBehavior !== 'prevent',
            duplicateScope: anki.duplicateScope === 'collection' ? 'collection' : 'deck',
            duplicateScopeOptions: {
                deckName: anki.duplicateScope === 'deck-root' ? format.deck.split('::')[0] : format.deck,
                checkChildren: anki.duplicateScope === 'deck-root',
                checkAllModels: Boolean(anki.duplicateScopeCheckAllModels),
            },
        },
    };
}

/** Whether a field of the note format uses one of these markers (prefix match). */
export function formatUsesMarker(anki, prefix) {
    const f = anki.cardFormats[0];
    return Boolean(f) && Object.values(f.fields).some((field) => getFieldMarkers(fieldValue(field)).some((m) => m.startsWith(prefix)));
}

export function isFormatReady(anki) {
    const f = anki.cardFormats[0];
    return Boolean(anki.enable && f && f.deck && f.model && Object.keys(f.fields).length > 0);
}
