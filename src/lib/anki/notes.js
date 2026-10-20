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
        // What {sentence} and the cloze markers hold: the sentence (as translated) or the whole
        // paragraph (as sent to the AI).
        contextMode: 'sentence',
        // What {ai-card} holds: the AI part alone ('card'), or the AI part followed by the
        // dictionaries' entries ('dictionaries'). The context is never in it: it goes in {sentence}.
        aiCardMode: 'card',
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
    if (out.contextMode !== 'paragraph') { out.contextMode = 'sentence'; }
    if (out.aiCardMode !== 'dictionaries') { out.aiCardMode = 'card'; }
    if (!Array.isArray(out.cardFormats) || out.cardFormats.length === 0) { out.cardFormats = base.cardFormats; }
    out.cardFormats = out.cardFormats.map((f) => ({...base.cardFormats[0], ...f, fields: {...(f.fields || {})}}));
    return out;
}

/** Markers Lookitan can fill, in the order shown in the marker menu. */
export const MARKERS = [
    'expression', 'reading', 'glossary', 'glossary-brief', 'glossary-no-dictionary', 'glossary-plain',
    'glossary-plain-no-dictionary', 'glossary-first', 'glossary-first-brief', 'glossary-first-no-dictionary',
    'glossary-first-plain', 'sentence', 'cloze-prefix', 'cloze-body', 'cloze-suffix', 'dictionary',
    'screenshot', 'image', 'audio', 'sentence-audio', 'popup-selection-text', 'ai-card', 'ai-card-plain', 'ai-definition', 'ai-explanation', 'ai-explanation-plain',
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
        [/(sentence|context|example).*(audio|sound)|(audio|sound).*(sentence|context|example)/, '{sentence-audio}'],
        [/(audio|sound)/, '{audio}'],
        [/(reading|ipa|pronunciation|transcription|phonetic)/, '{reading}'],
        [/(cloze)/, '{cloze-prefix}<b>{cloze-body}</b>{cloze-suffix}'],
        [/(sentence|context|example)/, '{sentence}'],
        [/(screenshot)/, '{screenshot}'],
        [/(image|picture|photo)/, '{image}'],
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

/**
 * The note with its fields in the order of its Anki note type, the model's first field first.
 * AnkiConnect's duplicate search (findNoteIds) queries the note's first field, as Anki does. The
 * settings come back from the app with their object keys sorted (serde_json without
 * preserve_order), so the saved field map's first key is the alphabetically first field ("Audio"),
 * not the note type's first one. A field the note type no longer has goes last.
 */
export function orderNoteFields(note, fieldNames) {
    if (!Array.isArray(fieldNames) || fieldNames.length === 0) { return note; }
    const fields = {};
    for (const name of fieldNames) { fields[name] = Object.hasOwn(note.fields, name) ? note.fields[name] : ''; }
    for (const [name, value] of Object.entries(note.fields)) { if (!Object.hasOwn(fields, name)) { fields[name] = value; } }
    return {...note, fields};
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

/**
 * The AI word card: headword, IPA, CEFR, part of speech and labels, translation, definition and
 * the explanation if it was generated. With `dictionaries` (aiCardMode 'dictionaries') the
 * dictionaries' entries follow it. The context is left out: it goes in {sentence}.
 */
function aiCardHtml(headword, ai, explanation, dictionaries = []) {
    const glossary = glossaryHtml(dictionaries);
    if (!ai.definition && !ai.translation && !explanation) { return glossary; }
    const head = [`<b>${escapeHtml(headword)}</b>`];
    if (ai.ipa) { head.push(`/${escapeHtml(ai.ipa)}/`); }
    const tags = [ai.cefr, ...(ai.partOfSpeech || []), ...(ai.labels || [])].filter(Boolean);
    const parts = [`<div>${head.join(' ')}</div>`];
    if (tags.length) { parts.push(`<div><i>${tags.map(escapeHtml).join(', ')}</i></div>`); }
    if (ai.translation) { parts.push(`<div>${escapeHtml(ai.translation)}</div>`); }
    if (ai.definition) { parts.push(`<div>${escapeHtml(ai.definition)}</div>`); }
    if (explanation) { parts.push(`<div>${renderMarkdown(explanation)}</div>`); }
    if (glossary) { parts.push(`<hr>${glossary}`); }
    return `<div style="text-align: left;">${parts.join('')}</div>`;
}

function aiCardPlain(headword, ai, explanation, dictionaries = []) {
    const glossary = glossaryPlain(dictionaries);
    if (!ai.definition && !ai.translation && !explanation) { return glossary; }
    const tags = [ai.cefr, ...(ai.partOfSpeech || []), ...(ai.labels || [])].filter(Boolean);
    const lines = [`${headword}${ai.ipa ? ` /${ai.ipa}/` : ''}`];
    if (tags.length) { lines.push(`(${tags.join(', ')})`); }
    if (ai.translation) { lines.push(ai.translation); }
    if (ai.definition) { lines.push(ai.definition); }
    if (explanation) { lines.push(markdownToText(explanation)); }
    const text = escapeHtml(lines.join('\n')).replace(/\n/g, '<br>');
    return glossary ? `${text}<br>${glossary}` : text;
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
 * @property {string} [context] The paragraph around the word, used when contextMode is 'paragraph'.
 * @property {{name: string, html: string}[]} [glossaries]
 * @property {{ipa?: string, cefr?: string, partOfSpeech?: string[], labels?: string[], definition?: string, translation?: string}} [aiCard]
 * @property {string} [aiExplanation]
 * @property {string} [contextTranslation]
 * @property {() => Promise<string|null>} [screenshot] Base64 PNG of the capture.
 * @property {() => Promise<{data: string, extension: string}[]>} [images] The pictures checked in the
 *   pop-up's "Imagens" section, as base64 files, for {image}.
 * @property {() => Promise<{data: string, extension: string}|null>} [audio] The word's audio from
 *   Configurações › Áudio (the recording last played, else the first found), for {audio}.
 * @property {() => Promise<{data: string, extension: string}|null>} [sentenceAudio] The sentence
 *   read aloud by an Edge neural voice, for {sentence-audio}.
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
    const sentence = (anki.contextMode === 'paragraph' && ctx.context) || ctx.sentence || '';
    const c = cloze(sentence, ctx.word);
    // The dictionaries' entries, for an AI card that carries them too.
    const cardDictionaries = anki.aiCardMode === 'dictionaries' ? defs.filter((d) => d.dictionary !== AI_GLOSSARY_NAME) : [];
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
        'sentence': escapeHtml(sentence),
        'cloze-prefix': escapeHtml(c.prefix),
        'cloze-body': escapeHtml(c.body),
        'cloze-suffix': escapeHtml(c.suffix),
        'dictionary': escapeHtml(first[0] ? first[0].dictionary : ''),
        'ai-card': aiCardHtml(ctx.headword || ctx.word, ai, ctx.aiExplanation || '', cardDictionaries),
        'ai-card-plain': aiCardPlain(ctx.headword || ctx.word, ai, ctx.aiExplanation || '', cardDictionaries),
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
        'image': '',
        'audio': '',
        'sentence-audio': '',
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
    const used = new Set();
    for (const [name, field] of Object.entries(format.fields)) {
        const template = fieldValue(field);
        for (const marker of getFieldMarkers(template)) { used.add(marker); }
        fields[name] = template;
    }
    if (used.has('screenshot') && conn && ctx.screenshot) {
        const data = await ctx.screenshot();
        if (data) {
            const fileName = generateAnkiNoteMediaFileName('lookitan_screenshot', '.png', Date.now());
            const stored = await conn.storeMediaFile(fileName, data);
            values.screenshot = `<img src="${escapeHtml(stored || fileName)}">`;
        }
    }
    if (used.has('image') && conn && ctx.images) {
        const files = await ctx.images();
        const time = Date.now();
        const tags = [];
        for (const [i, file] of files.entries()) {
            const fileName = generateAnkiNoteMediaFileName(`lookitan_image_${i + 1}`, `.${file.extension}`, time);
            const stored = await conn.storeMediaFile(fileName, file.data);
            tags.push(`<img src="${escapeHtml(stored || fileName)}">`);
        }
        values.image = tags.join('');
    }
    if (used.has('audio') && conn && ctx.audio) {
        const file = await ctx.audio();
        if (file) {
            const fileName = generateAnkiNoteMediaFileName('lookitan_audio', `.${file.extension}`, Date.now());
            const stored = await conn.storeMediaFile(fileName, file.data);
            // Anki's own sound tag, as Yomitan writes {audio}.
            values.audio = `[sound:${stored || fileName}]`;
        }
    }
    if (used.has('sentence-audio') && conn && ctx.sentenceAudio) {
        const file = await ctx.sentenceAudio();
        if (file) {
            const fileName = generateAnkiNoteMediaFileName('lookitan_sentence_audio', `.${file.extension}`, Date.now());
            const stored = await conn.storeMediaFile(fileName, file.data);
            values['sentence-audio'] = `[sound:${stored || fileName}]`;
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
