// Anki options (same shape as Yomitan's `anki` options) and note building with Yomitan-style
// field markers such as {expression}, {glossary}, {sentence}, {cloze-body} and {screenshot}.

import {AnkiConnect} from './anki-connect.js';
import {generateAnkiNoteMediaFileName, getFieldMarkers} from './anki-util.js';

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
    'expression', 'reading', 'glossary', 'glossary-brief', 'glossary-first', 'glossary-plain',
    'glossary-first-plain', 'sentence', 'cloze-prefix', 'cloze-body', 'cloze-suffix', 'dictionary',
    'screenshot', 'ai-definition', 'ai-explanation', 'translation', 'context-translation', 'cefr',
    'part-of-speech', 'tags', 'document-title', 'search-query',
];

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
    const glossaryWithAi = [];
    if (ai.definition) { glossaryWithAi.push(`<div><i>(AI)</i> ${escapeHtml(ai.definition)}</div>`); }
    for (const g of clean) { glossaryWithAi.push(`<div><i>(${escapeHtml(g.name)})</i> ${g.html}</div>`); }
    const c = cloze(ctx.sentence || '', ctx.word);
    const values = {
        'expression': escapeHtml(ctx.headword || ctx.word),
        'reading': escapeHtml(ai.ipa || ''),
        'glossary': glossaryWithAi.join(''),
        'glossary-brief': clean.map((g) => g.html).join('<br>') || escapeHtml(ai.definition || ''),
        'glossary-first': clean[0] ? clean[0].html : escapeHtml(ai.definition || ''),
        'glossary-plain': [ai.definition || '', ...clean.map((g) => htmlToText(g.html))].filter(Boolean).map(escapeHtml).join('<br>'),
        'glossary-first-plain': escapeHtml(clean[0] ? htmlToText(clean[0].html) : (ai.definition || '')),
        'sentence': escapeHtml(ctx.sentence || ''),
        'cloze-prefix': escapeHtml(c.prefix),
        'cloze-body': escapeHtml(c.body),
        'cloze-suffix': escapeHtml(c.suffix),
        'dictionary': escapeHtml(clean[0] ? clean[0].name : (ai.definition ? 'AI' : '')),
        'ai-definition': escapeHtml(ai.definition || ''),
        'ai-explanation': escapeHtml(ctx.aiExplanation || '').replace(/\n/g, '<br>'),
        'translation': escapeHtml(ai.translation || ''),
        'context-translation': escapeHtml((ctx.contextTranslation || '').replace(/\*\*/g, '')),
        'cefr': escapeHtml(ai.cefr || ''),
        'part-of-speech': escapeHtml((ai.partOfSpeech || []).join(', ')),
        'tags': escapeHtml([...(ai.labels || [])].join(', ')),
        'document-title': escapeHtml(ctx.title || ''),
        'search-query': escapeHtml(ctx.query || ctx.word),
        'screenshot': '',
    };

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
        fields[name] = fields[name].replace(/\{([\p{Letter}\p{Number}_-]+)\}/gu, (m, marker) => (marker in values ? values[marker] : m));
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

export function isFormatReady(anki) {
    const f = anki.cardFormats[0];
    return Boolean(anki.enable && f && f.deck && f.model && Object.keys(f.fields).length > 0);
}
