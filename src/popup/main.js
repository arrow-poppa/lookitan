// Lookup popup. Sections follow the dictionary order from the Dictionaries tab: the AI word card,
// the context translator and the MDict dictionaries. Top-right: save the word and add it to Anki.

import {emit, listen} from '@tauri-apps/api/event';
import {api, entryUrl, escapeHtml, loadAllSettings, renderMarkdown} from '../lib/api.js';
import {explain, isConfigured} from '../lib/ai/explain.js';
import {AiResponseCache} from '../lib/ai/response-cache.js';
import {cardPrompt, fillLanguages, parseCard, parseCardProgress} from '../lib/ai/word-card.js';
import {buildNote, createConnection, formatUsesMarker, glossaryDictionaryNames, isFormatReady} from '../lib/anki/notes.js';
import {cardFromLookup} from '../lib/cards.js';
import {hydrateIcons, icon} from '../lib/icons.js';

const $ = (id) => document.getElementById(id);

let settings = null;
let ai = null;
let anki = null;
let dicts = [];
/** Everything known about the current lookup. */
let state = null;
let pinned = false;
let zoom = Number(localStorage.getItem('zoom') || '1') || 1;
let renderSequence = 0;
/** Shared by every lookup while the popup lives, like the fork's generator cache. */
const responses = new AiResponseCache(undefined, {storage: localStorage});

/**
 * The text the AI reads with the word. As in Lookupper, the word card and the explanation get the
 * whole paragraph, and the context translation gets only the sentence.
 */
function aiText(kind, payload) {
    return (kind === 'context' ? payload.sentence : payload.context || payload.sentence) || payload.word;
}

/** Cache key: everything that changes the answer, as the fork's options context + term + sentence. */
async function aiKey(kind, prompt, lookup = state, options = ai) {
    // UI switches and credentials do not alter the answer. Never put API keys in persistent cache.
    const {apiKey: _credential, ...config} = options[options.provider] || {};
    const stable = Object.fromEntries(Object.entries(config).sort(([a], [b]) => a.localeCompare(b)));
    const content = responses.key([kind, options.provider, stable, options.temperature ?? 0.7,
        options.systemPrompt || '', prompt, lookup.payload.word, aiText(kind, lookup.payload)]);
    const digest = await crypto.subtle.digest('SHA-256', new TextEncoder().encode(content));
    return [...new Uint8Array(digest)].map((byte) => byte.toString(16).padStart(2, '0')).join('');
}

function applyZoom() {
    $('sections').style.zoom = String(zoom);
    localStorage.setItem('zoom', String(zoom));
}

function markWord(text, word) {
    const safe = escapeHtml(text);
    if (!word) { return safe; }
    const i = text.toLowerCase().indexOf(word.toLowerCase());
    if (i < 0) { return safe; }
    return `${escapeHtml(text.slice(0, i))}<mark>${escapeHtml(text.slice(i, i + word.length))}</mark>${escapeHtml(text.slice(i + word.length))}`;
}

/** The paragraph around the word, the word marked where the OCR found it (else at its first match). */
function markContext(p) {
    const text = p.context || p.sentence || '';
    const chars = Array.from(text);
    const at = p.context ? p.contextAt : null;
    const n = Array.from(p.word || '').length;
    if (Number.isInteger(at) && n > 0 && chars.slice(at, at + n).join('').toLowerCase() === p.word.toLowerCase()) {
        return `${escapeHtml(chars.slice(0, at).join(''))}<mark>${escapeHtml(chars.slice(at, at + n).join(''))}</mark>${escapeHtml(chars.slice(at + n).join(''))}`;
    }
    return markWord(text, p.word);
}

function section(title, {ai: isAi = false, onRegenerate = null} = {}) {
    const sec = document.createElement('section');
    sec.className = 'sec';
    sec.innerHTML = `<div class="sec-head"><span>${escapeHtml(title)}</span>${isAi ? icon('sparkle', 'sm') : ''}<span class="line"></span></div><div class="sec-body"></div>`;
    $('sections').append(sec);
    const body = sec.querySelector('.sec-body');
    if (onRegenerate) {
        const b = document.createElement('button');
        b.className = 'ghost icon';
        b.title = 'Gerar de novo';
        b.innerHTML = icon('refresh', 'sm');
        b.addEventListener('click', () => onRegenerate(body));
        sec.querySelector('.sec-head').append(b);
    }
    return body;
}

function shimmer() {
    return '<div class="shimmer w40"></div><div class="shimmer w80"></div><div class="shimmer w60"></div>';
}

function speak(text) {
    if (!('speechSynthesis' in window) || !text) { return; }
    const u = new SpeechSynthesisUtterance(text);
    u.lang = settings.ocrLanguage || 'en-US';
    window.speechSynthesis.cancel();
    window.speechSynthesis.speak(u);
}

// ------------------------------------------------------------------ AI word card

function renderCard(body, card, pending = false) {
    const p = state.payload;
    const headword = card.headword || p.word;
    const chips = [];
    if (card.cefr && /^[ABC][12]$/.test(card.cefr)) { chips.push(`<span class="cefr ${card.cefr.toLowerCase()}">${card.cefr}</span>`); }
    if (card.ipa) { chips.push(`<span class="ipa">/${escapeHtml(card.ipa)}/</span>`); }
    for (const pos of card.partOfSpeech) { chips.push(`<span class="chip">${escapeHtml(pos)}</span>`); }
    for (const l of card.labels) { chips.push(`<span class="chip">${escapeHtml(l)}</span>`); }
    const hide = settings.hideWordTranslation;
    body.innerHTML = `
        <div class="hw"><span class="word">${escapeHtml(headword)}</span>${'speechSynthesis' in window ? `<button class="ghost icon speak" title="Ouvir">${icon('speaker')}</button>` : ''}</div>
        ${chips.length ? `<div class="chips">${chips.join('')}</div>` : ''}
        ${card.translation && settings.showWordTranslation !== false ? `<div class="tr${hide ? ' hidden-tr' : ''}" title="${hide ? 'Clique para mostrar' : ''}">${escapeHtml(card.translation)}</div>` : ''}
        ${card.definition ? `<p class="def">${escapeHtml(card.definition)}</p>` : ''}
        ${pending ? '<div class="hint" role="status">Gerando…</div>' : `<button class="ghost explain-btn">${icon('sparkle', 'sm')}Explicar</button><button class="ghost icon explain-reset" title="Gerar explicação de novo" aria-label="Gerar explicação de novo">${icon('refresh', 'sm')}</button><div class="explanation ai-text" hidden></div>`}
        ${!pending && (p.context || p.sentence) ? `<details class="ctx"><summary>Contexto</summary><div class="body">${markContext(p)}</div></details>` : ''}`;
    body.querySelector('.speak')?.addEventListener('click', () => speak(headword));
    body.querySelector('.hidden-tr')?.addEventListener('click', (ev) => ev.currentTarget.classList.remove('hidden-tr'));
    body.querySelector('.explain-btn')?.addEventListener('click', () => void runExplain(body.querySelector('.explanation')));
    body.querySelector('.explain-reset')?.addEventListener('click', () => void runExplain(body.querySelector('.explanation'), true));
    if (!pending && ai.autoExplain === true) {
        void runExplain(body.querySelector('.explanation'));
    }
}

async function runExplain(target, force = false) {
    if (!target) { return; }
    target.hidden = false;
    target.innerHTML = shimmer();
    const lookup = state;
    const options = ai;
    const lookupSettings = settings;
    const request = {};
    lookup.explainRequest = request;
    lookup.explainPending = true;
    void refreshAnkiButton();
    const current = () => state === lookup && lookup.explainRequest === request;
    try {
        const prompt = fillLanguages(options.customPrompt, settings);
        const key = await aiKey('explain', prompt, lookup, options);
        if (!current()) { return; }
        lookup.keys.add(key);
        const text = await responses.run(key,
            (signal, onChunk) => explain(options, lookup.payload.word, aiText('explain', lookup.payload), {signal, prompt, onChunk}),
            (_d, full) => { if (current()) { target.innerHTML = renderMarkdown(full); } }, {force});
        if (current()) {
            lookup.explanation = text;
            target.innerHTML = renderMarkdown(text);
            if (lookup.savedId) { void saveWord(lookup, lookupSettings); }
        }
    } catch (e) {
        if (current() && e?.name !== 'AbortError') { target.innerHTML = `<p class="error">${escapeHtml(e?.message || String(e))}</p>`; }
    } finally {
        if (lookup.explainRequest === request) { lookup.explainPending = false; }
        if (current()) { void refreshAnkiButton(); }
    }
}

async function runCard(body, force = false) {
    const lookup = state;
    const options = ai;
    const request = {};
    lookup.cardRequest = request;
    const current = () => state === lookup && lookup.cardRequest === request;
    lookup.aiPending = true;
    void refreshAnkiButton();
    body.innerHTML = shimmer();
    try {
        const prompt = cardPrompt(options.cardPrompt, settings, options);
        const key = await aiKey('card', prompt, lookup, options);
        if (!current()) { return; }
        lookup.keys.add(key);
        const text = await responses.run(key,
            (signal, onChunk) => explain(options, lookup.payload.word, aiText('card', lookup.payload), {signal, prompt, onChunk}),
            (_delta, full) => {
                if (!current()) { return; }
                const partial = parseCardProgress(full);
                if (partial) { renderCard(body, partial, true); }
                else if (!full.includes('{')) { body.innerHTML = `<div class="ai-text">${renderMarkdown(full)}</div>`; }
            }, {force});
        if (!current()) { return; }
        const card = parseCard(text);
        if (card) {
            lookup.aiCard = card;
            renderCard(body, card);
        } else {
            lookup.explanation = text;
            body.innerHTML = `<div class="ai-text">${renderMarkdown(text)}</div>`;
        }
        if (settings.saveWordsAutomatically && !lookup.savedId) { void saveWord(); }
    } catch (e) {
        if (current() && e?.name !== 'AbortError') { body.innerHTML = `<p class="error">${escapeHtml(e?.message || String(e))}</p>`; }
    } finally {
        if (lookup.cardRequest === request) { lookup.aiPending = false; }
        if (current()) { void refreshAnkiButton(); }
    }
}

async function runContext(body, force = false) {
    const lookup = state;
    const options = ai;
    const p = lookup.payload;
    if (!p.sentence) { body.innerHTML = '<p class="hint">Sem frase de contexto nesta consulta.</p>'; return; }
    body.innerHTML = '<div class="shimmer w80"></div><div class="shimmer w60"></div>';
    try {
        const prompt = fillLanguages(options.contextPrompt, settings);
        const key = await aiKey('context', prompt, lookup, options);
        if (state !== lookup) { return; }
        lookup.keys.add(key);
        const text = await responses.run(key,
            (signal, onChunk) => explain(options, p.word, p.sentence, {signal, prompt, onChunk}),
            (_d, full) => { if (state === lookup) { body.innerHTML = `<div class="translation">${highlightStars(full)}</div>`; } },
            {force});
        if (state === lookup) {
            lookup.contextTranslation = text;
            body.innerHTML = `<div class="translation">${highlightStars(text)}</div>`;
        }
    } catch (e) {
        if (state === lookup && e?.name !== 'AbortError') { body.innerHTML = `<p class="error">${escapeHtml(e?.message || String(e))}</p>`; }
    }
}

function highlightStars(text) {
    return escapeHtml(text.trim()).replace(/\*\*([^*]+)\*\*/g, '<mark>$1</mark>');
}

// ------------------------------------------------------------------ dictionaries

function renderDictionary(d) {
    const entries = state.payload.entries || [];
    const mine = entries.filter((e) => e.dictId === d.id);
    if (mine.length === 0) { return false; }
    const body = section(d.name);
    body.classList.add('entry');
    for (const e of mine) {
        const frame = document.createElement('iframe');
        frame.src = entryUrl(e.dictId, e.entry) + (state.payload.fragment ? `#${encodeURIComponent(state.payload.fragment)}` : '');
        frame.title = `${d.name}: ${e.headword}`;
        frame.setAttribute('sandbox', 'allow-scripts allow-same-origin');
        body.append(frame);
    }
    return true;
}

function stopDictionaryAudio() {
    for (const frame of document.querySelectorAll?.('.entry iframe') || []) {
        frame.contentWindow?.postMessage({lookitan: true, type: 'stop-audio'}, '*');
    }
}

async function glossaries(lookup = state) {
    if (lookup.glossaries) { return lookup.glossaries; }
    const out = [];
    const grouped = new Map();
    const seen = new Set();
    for (const e of lookup.payload.entries || []) {
        const key = `${e.dictId}:${e.entry}`;
        if (seen.has(key)) { continue; }
        seen.add(key);
        try {
            const html = await api.entryHtml(e.dictId, e.entry);
            let group = grouped.get(e.dictId);
            if (!group) { group = {name: e.dictName, html: ''}; grouped.set(e.dictId, group); out.push(group); }
            group.html += (group.html ? '<hr>' : '') + html;
        } catch (_e) { /* skip unreadable articles */ }
    }
    lookup.glossaries = out;
    return out;
}

// ------------------------------------------------------------------ saving and Anki

async function saveWord(lookup = state, lookupSettings = settings) {
    if (!lookup || !lookup.payload.word) { return; }
    await glossaries(lookup);
    const card = cardFromLookup(lookup, lookupSettings);
    if (lookup.savedId) { card.id = lookup.savedId; }
    if (lookup.payload.recognition) {
        const img = await api.captureImage(lookup.payload.recognition, 'sentence').catch(() => null);
        if (img) { card.imageData = img; }
    }
    if (lookup.ankiNoteId) { card.ankiNoteId = lookup.ankiNoteId; }
    const saved = await api.saveWord(card);
    lookup.savedId = saved.id;
    if (state === lookup) { setSaveIcon(true); }
}

function setSaveIcon(saved) {
    $('btn-save').innerHTML = icon(saved ? 'bookmarkFilled' : 'bookmark');
    $('btn-save').classList.toggle('on', saved);
    $('btn-save').title = saved ? 'Palavra salva' : 'Salvar palavra';
}

function noteContext(lookup = state, options = anki) {
    return {
        word: lookup.payload.word,
        headword: lookup.aiCard?.headword || lookup.payload.entries?.[0]?.headword,
        sentence: lookup.payload.sentence,
        context: lookup.payload.context,
        glossaries: lookup.glossaries || [],
        aiCard: lookup.aiCard || {},
        aiExplanation: lookup.explanation,
        contextTranslation: lookup.contextTranslation,
        title: lookup.payload.app?.title,
        query: lookup.payload.word,
        popupSelection: popupSelection(),
        dictionaryOrder: glossaryDictionaryNames(dicts),
        screenshot: async () => (lookup.payload.recognition ? api.captureImage(lookup.payload.recognition, options.screenshotArea || 'sentence') : null),
    };
}

function setAnki(mode, title) {
    const b = $('btn-anki');
    b.hidden = mode === 'hidden';
    b.dataset.mode = mode;
    b.innerHTML = icon(mode === 'configure' ? 'settings' : mode === 'exists' || mode === 'added' ? 'check' : 'plus');
    b.classList.toggle('on', mode === 'exists' || mode === 'added');
    b.disabled = mode === 'busy' || mode === 'generating';
    b.title = title || '';
    b.setAttribute('aria-label', title || 'Anki');
}

function ankiStatus(message = '', error = false) {
    const target = $('anki-status');
    target.hidden = !message;
    target.textContent = message;
    target.classList.toggle('error', error);
}

async function refreshAnkiButton() {
    const lookup = state;
    const options = anki;
    if (!options.enable || !lookup?.payload.word) { setAnki('hidden'); ankiStatus(); return; }
    if (!isFormatReady(options)) {
        setAnki('configure', 'Configurar o Anki');
        ankiStatus('Escolha o baralho, o tipo de nota e os campos em Configurações › Anki.');
        return;
    }
    ankiStatus();
    if (lookup.ankiNoteId) { setAnki('added', 'Adicionada ao Anki — clique para abrir'); return; }
    if (lookup.aiPending || lookup.explainPending) { setAnki('generating', 'Aguarde a resposta da IA para adicionar ao Anki'); return; }
    setAnki('add', 'Adicionar ao Anki');
    if (!options.checkForDuplicates) { return; }
    try {
        const conn = createConnection(options);
        const note = await buildNote(noteContext(lookup, options), options, null);
        note.options.allowDuplicate = false;
        const [canAdd] = await conn.canAddNotes([note]);
        if (state !== lookup || lookup.ankiNoteId || lookup.aiPending || lookup.explainPending || $('btn-anki').dataset.mode === 'busy') { return; }
        if (!canAdd) {
            setAnki(options.duplicateBehavior === 'prevent' ? 'exists' : 'add',
                options.duplicateBehavior === 'prevent' ? 'Já está no Anki — clique para abrir' : 'Já existe uma nota parecida; clique para adicionar outra');
            lookup.duplicate = true;
        }
    } catch (e) {
        if (state !== lookup || lookup.ankiNoteId || lookup.aiPending || lookup.explainPending || $('btn-anki').dataset.mode === 'busy') { return; }
        setAnki('add', `Anki: ${e?.message || e}`);
        ankiStatus(`Anki: ${e?.message || e}`, true);
    }
}

async function onAnkiClick() {
    const mode = $('btn-anki').dataset.mode;
    if (mode === 'configure') {
        try {
            await api.openSettings();
            await emit('navigate-settings', {section: 'anki'});
        } catch (e) { ankiStatus(`Não foi possível abrir as configurações: ${e?.message || e}`, true); }
        return;
    }
    if (mode === 'busy' || mode === 'generating' || mode === 'hidden' || !state?.payload.word) { return; }
    const lookup = state;
    const options = anki;
    const lookupSettings = settings;
    const conn = createConnection(options);
    if (mode === 'added' || mode === 'exists') {
        try {
            if (lookup.ankiNoteId) {
                await conn.guiBrowseNote(lookup.ankiNoteId);
            } else {
                const note = await buildNote(noteContext(lookup, options), options, null);
                const [ids] = await conn.findNoteIds([note]);
                if (ids && ids.length) { await conn.guiBrowseNotes(ids); }
            }
        } catch (e) { if (state === lookup) { ankiStatus(`Anki: ${e?.message || e}`, true); } }
        return;
    }
    // Read the selection now: generating an explanation below can take a while.
    const selection = popupSelection();
    // A field asks for the AI explanation but "Explicar" was not used yet: generate it first.
    if (!lookup.explanation && formatUsesMarker(options, 'ai-explanation')) {
        const target = document.querySelector('.explanation');
        if (target) { await runExplain(target); }
        if (state !== lookup) { return; }
    }
    setAnki('busy', 'Adicionando…');
    ankiStatus();
    try {
        await glossaries(lookup);
        const note = await buildNote({...noteContext(lookup, options), popupSelection: selection}, options, conn);
        const id = await conn.addNote(note);
        if (id === null) { throw new Error('o Anki recusou a nota'); }
        if (options.suspendNewCards) {
            const cards = await conn.findCardsForNote(id);
            await conn.suspendCards(cards);
        }
        lookup.ankiNoteId = id;
        if (state === lookup) { setAnki('added', 'Adicionada ao Anki — clique para abrir'); }
        if (lookup.savedId) { void saveWord(lookup, lookupSettings); }
    } catch (e) {
        if (state === lookup) { setAnki('add', `Erro: ${e?.message || e}`); ankiStatus(`Anki: ${e?.message || e}`, true); }
    }
}

// ------------------------------------------------------------------ render

async function render(payload) {
    const sequence = ++renderSequence;
    const all = await loadAllSettings();
    const dictionaries = await api.listDictionaries();
    if (sequence !== renderSequence) { return; }
    settings = all.settings;
    ai = all.ai;
    anki = all.anki;
    dicts = dictionaries;
    state = {payload, keys: new Set()};
    if (ai.cancelPendingRequests !== false) {
        // Keep a request for this same word and sentence running; drop the others.
        const keep = new Set(await Promise.all([
            aiKey('card', cardPrompt(ai.cardPrompt, settings, ai)),
            aiKey('context', fillLanguages(ai.contextPrompt, settings)),
            aiKey('explain', fillLanguages(ai.customPrompt, settings)),
        ]));
        if (sequence !== renderSequence) { return; }
        responses.cancelPendingExcept(keep);
    }
    $('sections').replaceChildren();
    $('sections').scrollTop = 0;
    $('query').value = payload.word || '';
    $('notice').hidden = !payload.notice;
    $('notice').textContent = payload.notice || '';
    ankiStatus();
    const meta = [];
    if (payload.engine) { meta.push(payload.engine); }
    if (payload.ocrMs) { meta.push(`${payload.ocrMs} ms`); }
    $('meta').textContent = meta.join(' · ');
    setSaveIcon(false);
    setAnki('hidden');

    if (payload.error) {
        $('sections').innerHTML = `<div class="empty"><strong>${escapeHtml(payload.error)}</strong>Aponte para uma palavra e use o atalho de novo.</div>`;
        return;
    }

    const aiReady = isConfigured(ai);
    const noEntries = (payload.entries || []).length === 0;
    let shown = 0;
    for (const d of dicts.filter((x) => x.enabled)) {
        if (d.kind === 'ai-word') {
            const body = section('IA — palavra no contexto', {ai: true, onRegenerate: aiReady ? (b) => void runCard(b, true) : null});
            shown++;
            if (!aiReady) {
                body.innerHTML = '<p class="hint">Configure o seu provedor de IA em Dicionários › IA — palavra no contexto.</p>';
            } else if (ai.autoGenerateOnLookup || (noEntries && ai.unknownWordFallback)) {
                void runCard(body);
            } else {
                body.innerHTML = `<button class="ghost explain-btn">${icon('sparkle', 'sm')}Gerar</button>`;
                body.querySelector('button').addEventListener('click', () => void runCard(body));
            }
        } else if (d.kind === 'ai-context' && aiReady && payload.sentence && payload.sentence.trim() !== payload.word) {
            const body = section('Tradutor de contexto', {ai: true, onRegenerate: (b) => void runContext(b, true)});
            shown++;
            if (ai.autoGenerateOnLookup) {
                void runContext(body);
            } else {
                body.innerHTML = `<button class="ghost explain-btn">${icon('sparkle', 'sm')}Traduzir</button>`;
                body.querySelector('button').addEventListener('click', () => void runContext(body));
            }
        } else if (d.kind === 'mdict' && renderDictionary(d)) { shown++; }
    }
    if (shown === 0) {
        $('sections').innerHTML = '<div class="empty"><strong>Nenhum resultado</strong>Adicione dicionários ou configure a IA na janela do Lookitan.</div>';
    }
    void refreshAnkiButton();
}

async function lookup(text, fragment = '') {
    const t = text.trim();
    if (!t) { return; }
    const payload = await api.lookupText(t, state?.payload.sentence || '', state?.payload.context || '');
    payload.recognition = state?.payload.recognition ?? null;
    payload.app = state?.payload.app ?? null;
    payload.fragment = fragment;
    await render(payload);
}

// ------------------------------------------------------------------ popup selection ({popup-selection-text})

// Dictionary entries live in iframes on the lkt: origin, so they report their selection by message.
// The most recent non-empty selection wins; clearing it in the same place clears it here too.
let selectionText = '';
let selectionSource = null;

function noteSelection(source, text) {
    const value = String(text || '').trim();
    if (value) { selectionText = value; selectionSource = source; }
    else if (selectionSource === source) { selectionText = ''; selectionSource = null; }
}

function popupSelection() { return selectionText; }

document.addEventListener('selectionchange', () => noteSelection('popup', window.getSelection()?.toString()));

window.addEventListener('message', (ev) => {
    const d = ev.data;
    if (!d || d.lookitan !== true) { return; }
    const frame = [...document.querySelectorAll('.entry iframe')].find((f) => f.contentWindow === ev.source);
    if (!frame) { return; }
    if (d.type === 'selection') {
        noteSelection(ev.source, d.text);
    } else if (d.type === 'height') {
        if (Number.isFinite(d.height)) { frame.style.height = `${Math.min(Math.max(d.height + 4, 40), 10000)}px`; }
    } else if (d.type === 'lookup' && typeof d.text === 'string') {
        void lookup(d.text, typeof d.fragment === 'string' ? d.fragment : '');
    } else if (d.type === 'open' && typeof d.url === 'string') {
        void api.openExternal(d.url);
    }
});

hydrateIcons();
$('btn-search').addEventListener('click', () => {
    $('search').hidden = !$('search').hidden;
    if (!$('search').hidden) { $('query').focus(); $('query').select(); }
});
$('search').addEventListener('submit', (ev) => { ev.preventDefault(); void lookup($('query').value); });
// ------------------------------------------------------------------ open and close animation

function animateOpen(origin) {
    const card = $('card');
    card.style.setProperty('--tip-x', origin.relative ? `${origin.x * 100}%` : `${origin.x}px`);
    card.style.setProperty('--tip-y', `${origin.y}px`);
    card.classList.remove('tip-close', 'tip-open', 'tip-blank');
    void card.offsetWidth; // restart the animation for back-to-back lookups
    card.classList.add('tip-open');
}

let closeTimer = null;
/** Plays the close animation, then hides the window (the backend hides it at once for a new scan). */
function closePopup() {
    const card = $('card');
    if (closeTimer !== null) { return; }
    const reduced = window.matchMedia?.('(prefers-reduced-motion: reduce)').matches;
    if (reduced) { void api.hidePopup(); return; }
    card.classList.remove('tip-open');
    card.classList.add('tip-close');
    closeTimer = setTimeout(() => {
        closeTimer = null;
        void api.hidePopup();
        // Stay empty instead of growing back to full size before the window is hidden.
        card.classList.add('tip-blank');
        card.classList.remove('tip-close');
    }, 200);
}

// Before the backend hides the pop-up (Linux): show nothing, then say so once it is on screen,
// because a hidden WebKitGTK window shows its last painted frame again when it reappears.
void listen('popup-blank', (event) => {
    stopDictionaryAudio();
    if (closeTimer !== null) { clearTimeout(closeTimer); closeTimer = null; }
    const card = $('card');
    card.classList.remove('tip-open', 'tip-close');
    card.classList.add('tip-blank');
    const token = event?.payload?.token;
    if (token && typeof requestAnimationFrame === 'function') {
        requestAnimationFrame(() => requestAnimationFrame(() => { void api.webviewBlank(token); }));
    }
});

void listen('popup-open', (event) => {
    // A new lookup during the close animation keeps the popup open.
    if (closeTimer !== null) { clearTimeout(closeTimer); closeTimer = null; }
    animateOpen(event.payload);
});

$('btn-close').addEventListener('click', () => { closePopup(); });
$('btn-pin').addEventListener('click', () => {
    pinned = !pinned;
    $('btn-pin').classList.toggle('on', pinned);
    void api.setPopupPinned(pinned);
});
$('btn-save').addEventListener('click', () => { void saveWord(); });
$('btn-anki').addEventListener('click', () => { void onAnkiClick(); });
document.addEventListener('keydown', (ev) => {
    if (ev.key === 'Escape') { closePopup(); }
    if (ev.ctrlKey && (ev.key === '+' || ev.key === '=')) { zoom = Math.min(2, zoom + 0.1); applyZoom(); }
    if (ev.ctrlKey && ev.key === '-') { zoom = Math.max(0.6, zoom - 0.1); applyZoom(); }
    if (ev.ctrlKey && ev.key === '0') { zoom = 1; applyZoom(); }
});
applyZoom();
void listen('lookup', (event) => { void render(event.payload); });
// Closed like Yomitan's pop-up (Esc, X, a click elsewhere, a lookup that ends without a pop-up):
// drop the AI requests nobody will see.
void listen('popup-hidden', () => {
    stopDictionaryAudio();
    if (ai?.cancelPendingRequests !== false) { responses.cancelPending(); }
});
void listen('settings-changed', async () => {
    try {
        const all = await loadAllSettings();
        settings = all.settings;
        ai = all.ai;
        anki = all.anki;
        await refreshAnkiButton();
    } catch (e) { ankiStatus(`Configurações: ${e?.message || e}`, true); }
});
