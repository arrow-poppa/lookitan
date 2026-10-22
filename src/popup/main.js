// Lookup popup. Sections follow the dictionary order from the Dictionaries tab: the AI word card,
// the context translator, the pictures and the MDict dictionaries. Top-right: save the word and add
// it to Anki.

import {emit, listen} from '@tauri-apps/api/event';
import {api, entryUrl, escapeHtml, loadAllSettings, renderMarkdown} from '../lib/api.js';
import {explain, isConfigured} from '../lib/ai/explain.js';
import {aiOptionsFor} from '../lib/ai/defaults.js';
import {AiResponseCache} from '../lib/ai/response-cache.js';
import {cardPrompt, fillLanguages, parseCard, parseCardProgress} from '../lib/ai/word-card.js';
import {buildNote, cleanGlossaryHtml, createConnection, formatUsesMarker, glossaryDictionaryNames, isFormatReady, orderNoteFields} from '../lib/anki/notes.js';
import {cardFromLookup} from '../lib/cards.js';
import {invoke} from '@tauri-apps/api/core';
import {audioFile, edgeEntry, findAudio, normalizeAudioOptions, sentenceAudioFile} from '../lib/audio.js';
import {hydrateIcons, icon} from '../lib/icons.js';
import {cleanQuery, downloadImage, imagePrompts, searchImages, withMemeMode} from '../lib/images.js';

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
 * whole paragraph, and the context translation gets only the sentence. The image query also gets
 * the sentence: it only has to tell which meaning to picture.
 */
function aiText(kind, payload) {
    return (kind === 'context' || kind === 'images' ? payload.sentence : payload.context || payload.sentence) || payload.word;
}

/** The AI dictionary whose settings (provider, model, cancelling…) each kind of request follows. */
const KIND_DICTIONARY = {card: 'ai-word', explain: 'ai-word', context: 'ai-context', images: 'ai-images'};

/** The AI options of the dictionary a request kind belongs to: its own provider, the shared prompts. */
function optionsOf(kind) {
    return aiOptionsFor(ai, KIND_DICTIONARY[kind]);
}

/** The kind of each request key seen while the popup lives, to cancel by dictionary. */
const keyKinds = new Map();
const KEY_KINDS_KEPT = 400;

/** Cache key: everything that changes the answer, as the fork's options context + term + sentence. */
async function aiKey(kind, prompt, lookup = state, options = optionsOf(kind)) {
    // UI switches and credentials do not alter the answer. Never put API keys in persistent cache.
    const {apiKey: _credential, ...config} = options[options.provider] || {};
    const stable = Object.fromEntries(Object.entries(config).sort(([a], [b]) => a.localeCompare(b)));
    const content = responses.key([kind, options.provider, stable, options.temperature ?? 0.7,
        options.systemPrompt || '', prompt, lookup.payload.word, aiText(kind, lookup.payload)]);
    const digest = await crypto.subtle.digest('SHA-256', new TextEncoder().encode(content));
    const key = [...new Uint8Array(digest)].map((byte) => byte.toString(16).padStart(2, '0')).join('');
    keyKinds.delete(key);
    keyKinds.set(key, kind);
    while (keyKinds.size > KEY_KINDS_KEPT) { keyKinds.delete(keyKinds.keys().next().value); }
    return key;
}

const cancels = (kind) => optionsOf(kind).cancelPendingRequests !== false;

/** `keep` plus the requests of the AI dictionaries whose "Cancelar pedidos pendentes" is off. */
function keepRunning(keep) {
    for (const [key, kind] of keyKinds) {
        if (!cancels(kind)) { keep.add(key); }
    }
    return keep;
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

/**
 * The speaker beside a context: reads the original text aloud with the voice chosen in
 * Configurações › Áudio, the system voice or an Edge neural voice (the system voice if Edge fails).
 */
function sentenceButton() {
    const edge = normalizeAudioOptions(settings.audio).context.type === 'edge-tts';
    return `<button class="ghost icon speak-sentence" title="Ouvir o contexto (${edge ? 'voz neural do Edge' : 'voz do sistema'})" aria-label="Ouvir o contexto">${icon('speaker', 'sm')}</button>`;
}

function bindSentence(body, text) {
    body.querySelector('.speak-sentence')?.addEventListener('click', async () => {
        // A second click while it is still reading stops it.
        const speaking = Boolean(playing && !playing.paused) || ('speechSynthesis' in window && window.speechSynthesis.speaking);
        stopAudio();
        if (speaking || !text) { return; }
        const voice = normalizeAudioOptions(settings.audio).context;
        if (voice.type === 'edge-tts' && await playRecording(edgeEntry(text, voice.voice, settings.ocrLanguage || 'en-US'), text)) { return; }
        speak(text);
    });
}

// ------------------------------------------------------------------ word audio (Configurações › Áudio)

const audioIo = {invoke};
let playing = null;

function audioWord(term) {
    return {term, reading: '', language: String(settings.ocrLanguage || 'en').split('-')[0], locale: settings.ocrLanguage || 'en-US'};
}

/** The recordings of `term` from the configured sources, found once per lookup. */
function recordings(lookup, term) {
    lookup.audio ||= new Map();
    if (!lookup.audio.has(term)) { lookup.audio.set(term, findAudio(normalizeAudioOptions(settings.audio), audioWord(term), audioIo)); }
    return lookup.audio.get(term);
}

function stopAudio() {
    if (playing) { playing.pause(); URL.revokeObjectURL(playing.src); playing = null; }
    if ('speechSynthesis' in window) { window.speechSynthesis.cancel(); }
}

/** Plays one recording; false when it cannot be read, so the next one is tried. */
async function playRecording(entry, term) {
    stopAudio();
    if (entry.kind === 'tts') {
        if (!('speechSynthesis' in window)) { return false; }
        speak(term);
        return true;
    }
    const file = await audioFile(entry, audioIo).catch(() => null);
    if (!file) { return false; }
    const bytes = Uint8Array.from(atob(file.data), (c) => c.charCodeAt(0));
    const audio = new Audio(URL.createObjectURL(new Blob([bytes], {type: file.mime})));
    playing = audio;
    try { await audio.play(); return true; } catch (_e) { return false; }
}

/** The word's audio: the first recording that plays, in the order of the sources. */
async function playWord(lookup, term) {
    for (const entry of await recordings(lookup, term)) {
        if (state !== lookup) { return; }
        if (await playRecording(entry, term)) { lookup.audioPlayed = entry.kind === 'tts' ? lookup.audioPlayed : entry; return; }
    }
}

/** How the recordings after the first are shown: 'menu' (right click) or 'buttons'. */
function audioOthers() {
    return normalizeAudioOptions(settings.audio).others;
}

/** "Um botão para cada áudio": a small button per recording found, next to the speaker. */
async function audioButtons(target, lookup, term) {
    if (!target) { return; }
    const entries = (await recordings(lookup, term)).filter((e) => e.kind !== 'tts');
    if (state !== lookup || !target.isConnected) { return; }
    target.innerHTML = entries.map((e, i) => `<button class="ghost audio-chip" data-i="${i}" title="Ouvir: ${escapeHtml(e.name)}">${icon('speaker', 'sm')}${escapeHtml(e.name)}</button>`).join('');
    for (const b of target.querySelectorAll('button')) {
        b.addEventListener('click', async () => {
            const entry = entries[Number(b.dataset.i)];
            if (await playRecording(entry, term)) { lookup.audioPlayed = entry; }
        });
    }
}

/** Right click on the speaker: every recording found, to pick one (as Yomitan's audio menu). */
async function audioMenu(button, lookup, term) {
    closeAudioMenu();
    const entries = await recordings(lookup, term);
    if (state !== lookup) { return; }
    const menu = document.createElement('div');
    menu.className = 'audio-menu';
    menu.innerHTML = entries.length ? entries.map((e, i) => `<button class="ghost" data-i="${i}">${icon('speaker', 'sm')}${escapeHtml(e.name)}</button>`).join('')
        : '<p class="hint">Nenhum áudio encontrado. Configure as fontes em Configurações › Áudio.</p>';
    const r = button.getBoundingClientRect();
    const card = $('card').getBoundingClientRect();
    menu.style.left = `${r.left - card.left}px`;
    menu.style.top = `${r.bottom - card.top + 4}px`;
    $('card').append(menu);
    for (const b of menu.querySelectorAll('button')) {
        b.addEventListener('click', async () => {
            const entry = entries[Number(b.dataset.i)];
            closeAudioMenu();
            if (await playRecording(entry, term) && entry.kind !== 'tts') { lookup.audioPlayed = entry; }
        });
    }
    setTimeout(() => document.addEventListener('click', closeAudioMenu, {once: true}), 0);
}

function closeAudioMenu() {
    document.querySelector?.('.audio-menu')?.remove();
}

/** The file for {audio}: the recording last played, or the first that can be read (not the system voice). */
async function audioForAnki(lookup, term) {
    const entries = [lookup.audioPlayed, ...(await recordings(lookup, term))].filter((e) => e && e.kind !== 'tts');
    for (const entry of entries) {
        const file = await audioFile(entry, audioIo).catch(() => null);
        if (file) { return file; }
    }
    return null;
}

// ------------------------------------------------------------------ AI word card

/** The card's synonyms and components, as the pop-up and the saved cards show them. */
function cardExtrasHtml(card, components) {
    const synonyms = card.synonyms || [];
    const parts = components ? (card.components || []) : [];
    return `${synonyms.length ? `<div class="synonyms"><span class="muted">Sinônimos:</span> ${synonyms.map(escapeHtml).join(', ')}</div>` : ''}
        ${parts.length ? `<div class="components"><div class="components-title">Componentes</div>${parts.map((c) => `
            <div class="component"><span class="part">${escapeHtml(c.part)}</span><span class="part-text">${c.reading ? `<span class="part-reading">${escapeHtml(c.reading)}</span>` : ''}<span class="part-meaning">${escapeHtml(c.meaning)}</span></span></div>`).join('')}</div>` : ''}`;
}

function renderCard(body, card, pending = false) {
    const p = state.payload;
    const headword = card.headword || p.word;
    const chips = [];
    if (card.cefr && /^[ABC][12]$/.test(card.cefr)) { chips.push(`<span class="cefr ${card.cefr.toLowerCase()}">${card.cefr}</span>`); }
    if (card.reading) { chips.push(`<span class="reading">${escapeHtml(card.reading)}</span>`); }
    if (card.ipa) { chips.push(`<span class="ipa">/${escapeHtml(card.ipa)}/</span>`); }
    for (const pos of card.partOfSpeech) { chips.push(`<span class="chip">${escapeHtml(pos)}</span>`); }
    for (const l of card.labels) { chips.push(`<span class="chip">${escapeHtml(l)}</span>`); }
    const hide = settings.hideWordTranslation;
    body.innerHTML = `
        <div class="hw"><span class="word">${escapeHtml(headword)}</span><button class="ghost icon speak" title="${audioOthers() === 'menu' ? 'Ouvir (clique direito: escolher o áudio)' : 'Ouvir'}">${icon('speaker')}</button><span class="audio-buttons"></span></div>
        ${chips.length ? `<div class="chips">${chips.join('')}</div>` : ''}
        ${card.translation && settings.showWordTranslation !== false ? `<div class="tr${hide ? ' hidden-tr' : ''}" title="${hide ? 'Clique para mostrar' : ''}">${escapeHtml(card.translation)}</div>` : ''}
        ${card.definition ? `<p class="def">${escapeHtml(card.definition)}</p>` : ''}
        ${cardExtrasHtml(card, ai.cardComponents !== false)}
        ${pending ? '<div class="hint" role="status">Gerando…</div>' : `<button class="ghost explain-btn">${icon('sparkle', 'sm')}Explicar</button><button class="ghost icon explain-reset" title="Gerar explicação de novo" aria-label="Gerar explicação de novo">${icon('refresh', 'sm')}</button><div class="explanation ai-text" hidden></div>`}
        ${!pending && (p.context || p.sentence) ? `<details class="ctx"><summary>Contexto</summary><div class="body ctx-line">${sentenceButton()}<div>${markContext(p)}</div></div></details>` : ''}`;
    bindSentence(body, p.context || p.sentence);
    const speaker = body.querySelector('.speak');
    speaker?.addEventListener('click', () => void playWord(state, headword));
    if (audioOthers() === 'menu') {
        speaker?.addEventListener('contextmenu', (ev) => { ev.preventDefault?.(); void audioMenu(speaker, state, headword); });
    } else if (!pending) {
        void audioButtons(body.querySelector('.audio-buttons'), state, headword);
    }
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
    const options = optionsOf('context');
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
            (_d, full) => { if (state === lookup) { showTranslation(body, lookup, full); } },
            {force});
        if (state === lookup) {
            lookup.contextTranslation = text;
            showTranslation(body, lookup, text);
        }
    } catch (e) {
        if (state === lookup && e?.name !== 'AbortError') { body.innerHTML = `<p class="error">${escapeHtml(e?.message || String(e))}</p>`; }
    }
}

/**
 * The context translation, blurred until clicked when "Ocultar a tradução" is on, like the word
 * translation. Once shown, it stays shown for this lookup, also while it is still streaming.
 */
function showTranslation(body, lookup, text) {
    const hide = ai.hideContextTranslation === true && !lookup.contextRevealed;
    body.innerHTML = `<div class="ctx-line">${sentenceButton()}<div class="translation${hide ? ' hidden-tr' : ''}"${hide ? ' title="Clique para mostrar"' : ''}>${highlightStars(text)}</div></div>`;
    bindSentence(body, lookup.payload.sentence);
    body.querySelector('.hidden-tr')?.addEventListener('click', (ev) => {
        lookup.contextRevealed = true;
        ev.currentTarget.classList.remove('hidden-tr');
        ev.currentTarget.removeAttribute('title');
    });
}

function highlightStars(text) {
    return escapeHtml(text.trim()).replace(/\*\*([^*]+)\*\*/g, '<mark>$1</mark>');
}

// ------------------------------------------------------------------ pictures (the "Imagens" dictionary)

/** Bing results per query while the popup lives; a failed search is not kept. */
const imageSearches = new Map();
const IMAGE_SEARCHES_KEPT = 40;

function imagePromptKey(options) {
    const {system, user} = imagePrompts(options, settings);
    return `${system}\n\n${user}`;
}

/** The search query: the AI's (cached like the other AI answers), or the word itself. */
async function imageQuery(lookup, options, force) {
    const word = lookup.payload.word;
    if (options.imageQueryUseAi === false || !isConfigured(options)) { return word; }
    const {system, user} = imagePrompts(options, settings);
    const key = await aiKey('images', `${system}\n\n${user}`, lookup, options);
    lookup.keys.add(key);
    try {
        const text = await responses.run(key,
            (signal, onChunk) => explain(options, word, aiText('images', lookup.payload), {signal, prompt: user, system, onChunk}),
            undefined, {force});
        return cleanQuery(text, word);
    } catch (e) {
        if (e?.name === 'AbortError') { throw e; }
        // As in Migaku: when the AI fails, the word itself is searched.
        return word;
    }
}

function searchCached(query, force) {
    if (force) { imageSearches.delete(query); }
    let search = imageSearches.get(query);
    if (!search) {
        search = searchImages(query);
        search.catch(() => { if (imageSearches.get(query) === search) { imageSearches.delete(query); } });
        imageSearches.set(query, search);
        while (imageSearches.size > IMAGE_SEARCHES_KEPT) { imageSearches.delete(imageSearches.keys().next().value); }
    }
    return search;
}

async function runImages(body, force = false) {
    const lookup = state;
    const options = optionsOf('images');
    const request = {};
    lookup.imagesRequest = request;
    const current = () => state === lookup && lookup.imagesRequest === request;
    body.innerHTML = `<div class="img-strip">${'<div class="img-tile loading"></div>'.repeat(3)}</div>`;
    try {
        // Meme mode only changes the search, so switching it reuses the AI's query.
        const query = withMemeMode(await imageQuery(lookup, options, force), options.imageMemeMode === true);
        if (!current()) { return; }
        const items = await searchCached(query, force);
        if (!current()) { return; }
        lookup.images = {query, items, selected: new Set()};
        renderImages(body, lookup);
    } catch (e) {
        if (current() && e?.name !== 'AbortError') { body.innerHTML = `<p class="error">${escapeHtml(e?.message || String(e))}</p>`; }
    }
}

const checkLabel = (selected) => (selected ? 'Desmarcar (não enviar ao Anki)' : 'Marcar para enviar ao Anki');

function checkButton(selected) {
    const label = checkLabel(selected);
    return `<button class="img-check${selected ? ' on' : ''}" title="${label}" aria-label="${label}" aria-pressed="${selected}">${icon('check', 'sm')}</button>`;
}

function setChecked(button, selected) {
    button.classList.toggle('on', selected);
    button.title = checkLabel(selected);
    button.setAttribute('aria-label', checkLabel(selected));
    button.setAttribute('aria-pressed', String(selected));
}

/**
 * A horizontal strip, as in Migaku. The check mark shows on hover (and stays on a checked
 * picture); only a click on it checks the picture for Anki. A click anywhere else enlarges it.
 */
function renderImages(body, lookup) {
    const {query, items} = lookup.images;
    // Below the pictures, as in Migaku: the search that was made and the meme mode switch.
    const foot = `<div class="img-foot">
            <span class="img-query hint">Busca: ${escapeHtml(query)}</span>
            <label class="switch img-meme" title="Busca imagens de memes da palavra. Cuidado: os resultados podem ser inadequados ou ofensivos.">
                <span class="label">Modo meme</span><input type="checkbox"${ai.imageMemeMode === true ? ' checked' : ''}><span class="track"></span>
            </label>
        </div>`;
    const empty = (text) => {
        body.innerHTML = `<p class="hint">${text}</p>${foot}`;
        bindMemeMode(body);
    };
    if (items.length === 0) { empty(`Nenhuma imagem para “${escapeHtml(query)}”.`); return; }
    body.innerHTML = `<div class="img-strip" role="list">${items.map((item, i) => `
        <div class="img-tile${lookup.images.selected.has(i) ? ' sel' : ''}" role="listitem" data-i="${i}">
            <img src="${escapeHtml(item.thumb)}" alt="${escapeHtml(item.title || query)}" draggable="false" referrerpolicy="no-referrer">
            ${checkButton(lookup.images.selected.has(i))}
        </div>`).join('')}</div>${foot}`;
    bindMemeMode(body);
    const strip = body.querySelector('.img-strip');
    strip?.addEventListener('wheel', (ev) => {
        // The wheel scrolls the strip sideways while it can, then the pop-up again.
        if (Math.abs(ev.deltaY) <= Math.abs(ev.deltaX)) { return; }
        const max = strip.scrollWidth - strip.clientWidth;
        const next = Math.min(max, Math.max(0, strip.scrollLeft + ev.deltaY));
        if (max <= 0 || next === strip.scrollLeft) { return; }
        ev.preventDefault();
        strip.scrollLeft = next;
    }, {passive: false});
    for (const tile of body.querySelectorAll?.('.img-tile') || []) {
        const i = Number(tile.dataset.i);
        // A picture the site does not serve is left out, as Migaku leaves it out.
        tile.querySelector('img')?.addEventListener('error', () => {
            tile.remove();
            if (state === lookup && !strip?.querySelector('.img-tile')) { empty(`Nenhuma imagem pôde ser carregada para “${escapeHtml(query)}”.`); }
        }, {once: true});
        tile.addEventListener('click', (ev) => {
            if (state !== lookup) { return; }
            const check = ev.target.closest?.('.img-check');
            if (check) {
                ev.stopPropagation();
                toggleImage(lookup, i);
                setChecked(check, lookup.images.selected.has(i));
                tile.classList.toggle('sel', lookup.images.selected.has(i));
                return;
            }
            openViewer(lookup, i);
        });
    }
}

/** The meme mode switch searches again at once and is kept for the next lookups, as in Migaku. */
function bindMemeMode(body) {
    body.querySelector('.img-meme input')?.addEventListener('change', (ev) => {
        const on = Boolean(ev.target.checked);
        ai.imageMemeMode = on;
        void saveMemeMode(on);
        void runImages(body);
    });
}

async function saveMemeMode(on) {
    try {
        // Only this key changes; everything else is saved as the backend has it now.
        const fresh = await api.getSettings();
        fresh.ai = {...(fresh.ai && typeof fresh.ai === 'object' ? fresh.ai : {}), imageMemeMode: on};
        await api.saveSettings(fresh);
    } catch (e) { ankiStatus(`Configurações: ${e?.message || e}`, true); }
}

function toggleImage(lookup, i) {
    const selected = lookup.images.selected;
    if (selected.has(i)) { selected.delete(i); } else { selected.add(i); }
}

/** Shows a checked or unchecked picture the same way in the strip, after a change in the viewer. */
function syncTile(lookup, i) {
    const tile = document.querySelector?.(`.img-tile[data-i="${i}"]`);
    const check = tile?.querySelector('.img-check');
    if (!tile || !check) { return; }
    tile.classList.toggle('sel', lookup.images.selected.has(i));
    setChecked(check, lookup.images.selected.has(i));
}

// The enlarged picture covers the pop-up. The arrows at its sides, ← →, or dragging the picture
// sideways move to the neighbours; Esc, a click outside the picture or X closes it.
let viewer = null;

/** How far the picture must be dragged to move to the next one, in CSS pixels. */
const SWIPE_PX = 60;

function openViewer(lookup, i) {
    closeViewer();
    const box = document.createElement('div');
    box.className = 'img-viewer';
    box.setAttribute('role', 'dialog');
    box.setAttribute('aria-label', 'Imagem ampliada');
    $('card').append(box);
    viewer = {box, lookup, i, swiped: false};
    box.addEventListener('click', (ev) => {
        // The click that ends a drag is not a click outside the picture.
        if (viewer?.swiped) { viewer.swiped = false; return; }
        if (ev.target === box || ev.target.classList?.contains('img-frame')) { closeViewer(); }
    });
    drawViewer();
}

function drawViewer(from = '') {
    if (!viewer) { return; }
    const {box, lookup, i} = viewer;
    const items = lookup.images.items;
    const item = items[i];
    const selected = lookup.images.selected.has(i);
    const several = items.length > 1;
    let host = '';
    try { host = item.page ? new URL(item.page).hostname.replace(/^www\./, '') : ''; } catch (_e) { /* not a URL */ }
    box.innerHTML = `
        <div class="img-frame">
            <img class="full${from ? ` from-${from}` : ''}" src="${escapeHtml(item.thumb)}" alt="${escapeHtml(item.title || lookup.images.query)}" draggable="false" referrerpolicy="no-referrer">
            ${several ? `<button class="img-nav prev" title="Anterior (←)" aria-label="Anterior">${icon('chevronLeft')}</button>
            <button class="img-nav next" title="Próxima (→)" aria-label="Próxima">${icon('chevronRight')}</button>` : ''}
        </div>
        <div class="img-bar">
            ${several ? `<span class="img-count">${i + 1}/${items.length}</span>` : ''}
            <span class="img-caption">${escapeHtml(item.title || lookup.images.query)}</span>
            <button class="ghost icon img-zoom-out" title="Diminuir (−)" aria-label="Diminuir">${icon('zoomOut')}</button>
            <button class="ghost icon img-zoom-in" title="Ampliar (+, roda do mouse ou clique duplo)" aria-label="Ampliar">${icon('zoomIn')}</button>
            ${host ? `<button class="ghost img-page" title="${escapeHtml(item.page)}">${icon('open', 'sm')}${escapeHtml(host)}</button>` : ''}
            <button class="ghost img-pick${selected ? ' on' : ''}" aria-pressed="${selected}" title="${checkLabel(selected)}">${icon('check', 'sm')}${selected ? 'Marcada para o Anki' : 'Marcar para o Anki'}</button>
            <button class="ghost icon img-close" title="Fechar (Esc)">${icon('close')}</button>
        </div>`;
    const full = box.querySelector('.full');
    viewer.zoom = {scale: 1, x: 0, y: 0, full, frame: box.querySelector('.img-frame')};
    gestures(viewer.zoom.frame, full, several);
    box.querySelector('.img-zoom-in')?.addEventListener('click', (ev) => { ev.stopPropagation(); zoomBy(ZOOM_STEP); });
    box.querySelector('.img-zoom-out')?.addEventListener('click', (ev) => { ev.stopPropagation(); zoomBy(1 / ZOOM_STEP); });
    if (full && item.src !== item.thumb && typeof Image === 'function') {
        // Bing's thumbnail shows at once; the full picture replaces it once it arrives, and a site
        // that refuses it leaves the thumbnail.
        const whole = new Image();
        whole.referrerPolicy = 'no-referrer';
        whole.addEventListener('load', () => { if (viewer?.box === box && viewer.i === i) { full.src = item.src; } }, {once: true});
        whole.src = item.src;
    }
    box.querySelector('.img-pick')?.addEventListener('click', (ev) => {
        ev.stopPropagation();
        toggleImage(lookup, i);
        syncTile(lookup, i);
        drawViewer();
    });
    box.querySelector('.img-nav.prev')?.addEventListener('click', (ev) => { ev.stopPropagation(); stepViewer(-1); });
    box.querySelector('.img-nav.next')?.addEventListener('click', (ev) => { ev.stopPropagation(); stepViewer(1); });
    box.querySelector('.img-close')?.addEventListener('click', (ev) => { ev.stopPropagation(); closeViewer(); });
    box.querySelector('.img-page')?.addEventListener('click', (ev) => { ev.stopPropagation(); void api.openExternal(item.page); });
}

const ZOOM_STEP = 1.25;
const ZOOM_MAX = 8;
/** Scale a double click toggles to (and back to 1). */
const ZOOM_DOUBLE = 2.5;

function applyPictureZoom(z) {
    if (z.scale <= 1) { z.scale = 1; z.x = 0; z.y = 0; }
    z.full.style.transform = z.scale === 1 ? '' : `translate(${z.x}px, ${z.y}px) scale(${z.scale})`;
    z.full.classList.toggle('zoomed', z.scale > 1);
}

/**
 * Zooms the enlarged picture by `factor`, keeping the point under (clientX, clientY) in place;
 * without a point, around the picture's centre.
 */
function zoomBy(factor, clientX = null, clientY = null) {
    const z = viewer?.zoom;
    if (!z?.full) { return; }
    const next = Math.min(ZOOM_MAX, Math.max(1, z.scale * factor));
    if (clientX !== null && typeof z.full.getBoundingClientRect === 'function') {
        // The picture's centre without the zoom's translation, then the pointer relative to it.
        const r = z.full.getBoundingClientRect();
        const cx = r.left + r.width / 2 - z.x;
        const cy = r.top + r.height / 2 - z.y;
        const px = (clientX - cx - z.x) / z.scale;
        const py = (clientY - cy - z.y) / z.scale;
        z.x = clientX - cx - px * next;
        z.y = clientY - cy - py * next;
    } else {
        z.x *= next / z.scale;
        z.y *= next / z.scale;
    }
    z.scale = next;
    applyPictureZoom(z);
}

/**
 * The picture's gestures: the wheel and a double click zoom; a drag moves a zoomed picture
 * around, and otherwise, past SWIPE_PX sideways (mouse, pen or finger), goes to its neighbour.
 */
function gestures(frame, full, several) {
    if (!frame || !full) { return; }
    let drag = null;
    const reset = () => { full.classList.remove('dragging'); full.style.opacity = ''; applyPictureZoom(viewer.zoom); };
    frame.addEventListener('wheel', (ev) => {
        ev.preventDefault();
        zoomBy(ev.deltaY < 0 ? ZOOM_STEP : 1 / ZOOM_STEP, ev.clientX, ev.clientY);
    }, {passive: false});
    full.addEventListener('dblclick', (ev) => {
        ev.stopPropagation();
        const z = viewer?.zoom;
        if (z) { zoomBy(z.scale > 1 ? 1 / z.scale : ZOOM_DOUBLE, ev.clientX, ev.clientY); }
    });
    frame.addEventListener('pointerdown', (ev) => {
        if (ev.button !== 0 || ev.target.closest?.('button')) { return; }
        const z = viewer?.zoom;
        drag = {id: ev.pointerId, x: ev.clientX, y: ev.clientY, moved: false, panning: Boolean(z && z.scale > 1), startX: z?.x || 0, startY: z?.y || 0};
    });
    frame.addEventListener('pointermove', (ev) => {
        if (!drag || ev.pointerId !== drag.id) { return; }
        const dx = ev.clientX - drag.x;
        const dy = ev.clientY - drag.y;
        if (!drag.moved && Math.hypot(dx, dy) < 6) { return; }
        if (!drag.panning && !several) { return; }
        // Captured only once it is a drag: a plain click on the picture must stay on the picture.
        if (!drag.moved) { frame.setPointerCapture?.(ev.pointerId); }
        drag.moved = true;
        full.classList.add('dragging');
        if (drag.panning) {
            const z = viewer.zoom;
            z.x = drag.startX + dx;
            z.y = drag.startY + dy;
            applyPictureZoom(z);
        } else {
            full.style.transform = `translateX(${dx}px)`;
            full.style.opacity = String(Math.max(0.35, 1 - Math.abs(dx) / 400));
        }
    });
    const end = (ev) => {
        if (!drag || ev.pointerId !== drag.id) { return; }
        const {moved, x, panning} = drag;
        drag = null;
        if (!moved) { return; }
        const dx = ev.clientX - x;
        if (viewer) {
            viewer.swiped = true;
            setTimeout(() => { if (viewer) { viewer.swiped = false; } }, 0);
        }
        if (!panning && ev.type === 'pointerup' && Math.abs(dx) >= SWIPE_PX) { stepViewer(dx < 0 ? 1 : -1); } else { reset(); }
    };
    frame.addEventListener('pointerup', end);
    frame.addEventListener('pointercancel', end);
}

function stepViewer(delta) {
    if (!viewer) { return; }
    const n = viewer.lookup.images.items.length;
    viewer.i = (viewer.i + delta + n) % n;
    // The next picture slides in from the side it comes from.
    drawViewer(delta > 0 ? 'right' : 'left');
}

/** Closes the enlarged picture; false when none was open. */
function closeViewer() {
    if (!viewer) { return false; }
    viewer.box.remove();
    viewer = null;
    return true;
}

/** The checked pictures as files for {image}, in the strip's order; unreadable ones are skipped. */
async function checkedImageFiles(lookup) {
    const images = lookup.images;
    if (!images || images.selected.size === 0) { return []; }
    const order = [...images.selected].sort((a, b) => a - b);
    const files = await Promise.all(order.map((i) => downloadImage(images.items[i]).catch(() => null)));
    return files.filter(Boolean);
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
    await refreshSavedCount(lookup);
}

function setSaveIcon(saved) {
    $('btn-save').innerHTML = icon(saved ? 'bookmarkFilled' : 'bookmark');
    $('btn-save').classList.toggle('on', saved);
    $('btn-save').title = saved ? 'Palavra salva' : 'Salvar palavra';
}

// ------------------------------------------------------------------ trackers: saved cards and Anki notes

// Two counters beside the bookmark and Anki buttons, like Yomitan's "view added note" button and
// Lookupper's bookmark that changes colour: how many cards of this word were saved in the app (a
// click shows them in the pop-up), and how many Anki notes it has (a click opens Anki's browser).

/** The forms of this lookup's word: as found, the first entry's headword and the AI card's. */
function wordFormList(lookup) {
    const p = lookup.payload;
    const seen = new Set();
    return [p.word, p.entries?.[0]?.headword, lookup.aiCard?.headword].map((w) => String(w || '').trim())
        .filter((w) => w && !seen.has(w.toLowerCase()) && seen.add(w.toLowerCase()));
}

/** The forms a saved card may have used for this lookup's word, lower-cased. */
function wordForms(lookup) {
    return new Set(wordFormList(lookup).map((w) => w.toLowerCase()));
}

/** The saved cards of this lookup's word (by the word as found or its dictionary form), newest first. */
async function savedCardsOf(lookup) {
    const forms = wordForms(lookup);
    const all = await api.listWords().catch(() => []);
    return (all || [])
        .filter((c) => forms.has(String(c.word || '').trim().toLowerCase()) || forms.has(String(c.dictionaryForm || '').trim().toLowerCase()))
        .sort((a, b) => (b.createdAt || 0) - (a.createdAt || 0));
}

function setCount(button, n, iconName, title) {
    button.hidden = !n;
    if (!n) { return; }
    button.innerHTML = `${icon(iconName, 'sm')}<span class="n">${n > 99 ? '99+' : n}</span>`;
    button.title = title;
    button.setAttribute('aria-label', title);
}

async function refreshSavedCount(lookup = state) {
    if (!lookup?.payload.word) { setCount($('btn-saved'), 0); return; }
    const check = (lookup.savedCheck || 0) + 1;
    lookup.savedCheck = check;
    const cards = await savedCardsOf(lookup);
    if (state !== lookup || lookup.savedCheck !== check) { return; }
    lookup.savedCards = cards;
    const n = cards.length;
    setCount($('btn-saved'), n, 'bookmarkFilled',
        `Salva ${n === 1 ? '1 vez' : `${n} vezes${n > 1 ? ' em contextos diferentes' : ''}`} — clique para ver`);
}

// Field names of each Anki note type, asked once per pop-up session (again after a settings change).
let modelFieldNames = new Map();

async function noteTypeFields(conn, options) {
    const model = options.cardFormats?.[0]?.model || '';
    const key = `${options.server} ${model}`;
    if (!modelFieldNames.has(key)) {
        const asked = conn.getModelFieldNames(model);
        modelFieldNames.set(key, asked);
        asked.catch(() => modelFieldNames.delete(key));
    }
    return modelFieldNames.get(key);
}

/**
 * Notes for Anki's duplicate search: fields in the note type's order (it queries the first one),
 * and only those whose first field has something, since an empty one would match every note
 * without it, whatever the word.
 */
async function duplicateQueryNotes(conn, options, contexts) {
    const names = await noteTypeFields(conn, options);
    const notes = await Promise.all(contexts.map((ctx) => buildNote(ctx, options, null)));
    return notes.map((note) => orderNoteFields(note, names))
        .filter((note) => String(Object.values(note.fields)[0] ?? '').replace(/<[^>]*>/g, '').trim() !== '');
}

/**
 * The Anki notes of this word: one duplicate-check query per form of the word (as found, the
 * dictionary's headword, the AI card's once it is there), so the counter does not wait for the AI
 * and finds notes made from another form. Only the newest check of a lookup is shown.
 */
async function refreshAnkiNotes(lookup = state, options = anki) {
    const button = $('btn-anki-notes');
    if (!options.enable || !options.checkForDuplicates || !isFormatReady(options) || !lookup?.payload.word) { setCount(button, 0); return; }
    const check = (lookup.ankiNotesCheck || 0) + 1;
    lookup.ankiNotesCheck = check;
    const latest = () => state === lookup && lookup.ankiNotesCheck === check;
    try {
        const conn = createConnection(options);
        const base = noteContext(lookup, options);
        const notes = await duplicateQueryNotes(conn, options, wordFormList(lookup).map((form) => ({...base, word: form, headword: form})));
        const found = notes.length ? await conn.findNoteIds(notes) : [];
        if (!latest()) { return; }
        const ids = [...new Set([...(found || []).flat(), ...(lookup.ankiNoteId ? [lookup.ankiNoteId] : [])])];
        lookup.ankiNotes = ids;
        const n = ids.length;
        setCount(button, n, 'cards', `${n === 1 ? '1 nota' : `${n} notas`} desta palavra no Anki — clique para abrir no Anki`);
    } catch (_e) {
        // AnkiConnect not running: no counter, as with the "+" button.
        if (latest()) { setCount(button, 0); }
    }
}

async function openAnkiNotes() {
    const lookup = state;
    if (!lookup?.ankiNotes?.length) { return; }
    try { await createConnection(anki).guiBrowseNotes(lookup.ankiNotes); }
    catch (e) { if (state === lookup) { ankiStatus(`Anki: ${e?.message || e}`, true); } }
}

// The saved cards open over the pop-up, like the enlarged picture; Esc or X closes them.
let savedView = null;

async function openSavedCards() {
    const lookup = state;
    if (!lookup) { return; }
    closeViewer();
    closeSavedCards();
    const box = document.createElement('div');
    box.className = 'saved-view';
    box.setAttribute('role', 'dialog');
    box.setAttribute('aria-label', 'Cards salvos desta palavra');
    $('card').append(box);
    savedView = {box, lookup};
    await drawSavedCards();
}

async function drawSavedCards() {
    if (!savedView) { return; }
    const {box, lookup} = savedView;
    const cards = await savedCardsOf(lookup);
    if (savedView?.box !== box) { return; }
    lookup.savedCards = cards;
    const word = lookup.aiCard?.headword || lookup.payload.entries?.[0]?.headword || lookup.payload.word;
    const date = (ms) => (ms ? new Date(ms).toLocaleString('pt-BR', {dateStyle: 'short', timeStyle: 'short'}) : '');
    box.innerHTML = `
        <div class="saved-bar">
            <strong>${escapeHtml(word)}</strong><span class="muted">${cards.length === 1 ? '1 card salvo' : `${cards.length} cards salvos`}</span>
            <span class="spacer"></span>
            <button class="ghost icon saved-close" title="Fechar (Esc)" aria-label="Fechar">${icon('close')}</button>
        </div>
        <div class="saved-list">${cards.length ? cards.map((c, i) => `
            <article class="saved-card${c.id === lookup.savedId ? ' current' : ''}" data-i="${i}">
${savedCardHtml(c, c.id === lookup.savedId)}
                <div class="saved-foot">
                    <span class="muted">${escapeHtml([date(c.createdAt), c.windowTitle].filter(Boolean).join(' · '))}</span>
                    <span class="spacer"></span>
                    ${c.ankiNoteId ? `<button class="ghost saved-anki" title="Abrir esta nota no navegador do Anki">${icon('cards', 'sm')}Abrir no Anki</button>` : ''}
                    <button class="ghost icon saved-delete" title="Excluir este card salvo" aria-label="Excluir este card salvo">${icon('trash', 'sm')}</button>
                </div>
            </article>`).join('') : '<p class="hint">Nenhum card salvo desta palavra.</p>'}
        </div>`;
    box.querySelector('.saved-close').addEventListener('click', () => closeSavedCards());
    box.querySelectorAll('.saved-card').forEach((article) => {
        const card = cards[Number(article.dataset.i)];
        article.querySelector('.saved-anki')?.addEventListener('click', async () => {
            try { await createConnection(anki).guiBrowseNote(card.ankiNoteId); }
            catch (e) { ankiStatus(`Anki: ${e?.message || e}`, true); }
        });
        article.querySelector('.saved-delete')?.addEventListener('click', async () => {
            await api.deleteWord(card.id);
            if (card.id === lookup.savedId) {
                lookup.savedId = null;
                if (state === lookup) { setSaveIcon(false); }
            }
            await refreshSavedCount(lookup);
            if (!lookup.savedCards?.length) { closeSavedCards(); return; }
            await drawSavedCards();
        });
        if (card.image) {
            void api.wordImage(card.image).then((data) => {
                const shot = article.querySelector('.saved-shot');
                if (!data || !shot) { return; }
                shot.innerHTML = `<img src="data:image/png;base64,${data}" alt="Captura do contexto">`;
                shot.hidden = false;
            }, () => {});
        }
    });
}

/**
 * The whole saved card, as the Palavras tab shows it: the word with IPA, CEFR, part of speech and
 * labels, the translation, definition and explanation, the paragraph (or sentence) with its
 * translation, the screenshot and the dictionaries' entries, folded.
 */
function savedCardHtml(c, current) {
    const chips = [
        c.reading ? `<span class="reading">${escapeHtml(c.reading)}</span>` : '',
        c.ipa ? `<span class="ipa">/${escapeHtml(c.ipa)}/</span>` : '',
        c.cefr && /^[ABC][12]$/.test(c.cefr) ? `<span class="cefr ${c.cefr.toLowerCase()}">${c.cefr}</span>` : '',
        ...[...(c.partOfSpeech || []), ...(c.labels || [])].map((t) => `<span class="chip">${escapeHtml(t)}</span>`),
        current ? '<span class="chip">esta consulta</span>' : '',
    ].join('');
    const text = c.context && c.context.includes(c.sentence || '') ? c.context : c.sentence;
    const glossaries = (c.glossaries || []).filter((g) => g?.html).map((g) => `
        <details class="saved-gloss"><summary>${escapeHtml(g.name || 'Dicionário')}</summary><div class="gloss">${cleanGlossaryHtml(g.html)}</div></details>`).join('');
    return `
        <div class="saved-head"><span class="word">${escapeHtml(c.dictionaryForm || c.word)}</span>${chips}</div>
        ${c.dictionaryForm && c.word && c.dictionaryForm.toLowerCase() !== c.word.toLowerCase() ? `<div class="muted saved-form">Na tela: ${escapeHtml(c.word)}</div>` : ''}
        ${c.translation ? `<div class="tr">${escapeHtml(c.translation)}</div>` : ''}
        ${c.definition ? `<p class="def">${escapeHtml(c.definition)}</p>` : ''}
        ${cardExtrasHtml(c, true)}
        ${c.explanation ? `<div class="explanation ai-text">${renderMarkdown(c.explanation)}</div>` : ''}
        ${text ? `<blockquote>${markWord(text, c.word)}</blockquote>` : ''}
        ${c.contextTranslation ? `<p class="ctx-tr">${highlightStars(c.contextTranslation)}</p>` : ''}
        <div class="saved-shot" hidden></div>
        ${glossaries}`;
}

/** Closes the saved cards; false when they were not open. */
function closeSavedCards() {
    if (!savedView) { return false; }
    savedView.box.remove();
    savedView = null;
    return true;
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
        images: () => checkedImageFiles(lookup),
        audio: () => audioForAnki(lookup, lookup.aiCard?.headword || lookup.payload.entries?.[0]?.headword || lookup.payload.word),
        sentenceAudio: () => sentenceAudioFile(lookup.payload.sentence, settings.audio, settings.ocrLanguage, audioIo),
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
    // The counters follow the lookup: the AI card can change the word's dictionary form.
    void refreshSavedCount(lookup);
    void refreshAnkiNotes(lookup, options);
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
        // AnkiConnect not running: no notice in the pop-up, only the button's tooltip. Clicking
        // "+" still reports the error.
        if (state !== lookup || lookup.ankiNoteId || lookup.aiPending || lookup.explainPending || $('btn-anki').dataset.mode === 'busy') { return; }
        setAnki('add', `Anki: ${e?.message || e}`);
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
                const notes = await duplicateQueryNotes(conn, options, [noteContext(lookup, options)]);
                const [ids] = notes.length ? await conn.findNoteIds(notes) : [[]];
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
        const context = {...noteContext(lookup, options), popupSelection: selection};
        // {sentence-audio} is read aloud now, by the Edge voice; when it fails the field stays empty.
        let noSentenceAudio = false;
        const readSentence = context.sentenceAudio;
        context.sentenceAudio = async () => {
            const file = await readSentence();
            noSentenceAudio = !file && Boolean(String(lookup.payload.sentence || '').trim());
            return file;
        };
        const note = await buildNote(context, options, conn);
        const id = await conn.addNote(note);
        if (id === null) { throw new Error('o Anki recusou a nota'); }
        if (options.suspendNewCards) {
            const cards = await conn.findCardsForNote(id);
            await conn.suspendCards(cards);
        }
        lookup.ankiNoteId = id;
        if (state === lookup) {
            setAnki('added', 'Adicionada ao Anki — clique para abrir');
            if (noSentenceAudio) { ankiStatus('Adicionada sem o áudio da frase: a voz do Edge não respondeu. Tente de novo mais tarde.', true); }
        }
        void refreshAnkiNotes(lookup, options);
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
    if (Object.keys(KIND_DICTIONARY).some(cancels)) {
        // Keep a request for this same word and sentence running, and those of the dictionaries
        // that do not cancel; drop the others.
        const keep = new Set(await Promise.all([
            aiKey('card', cardPrompt(ai.cardPrompt, settings, ai)),
            aiKey('context', fillLanguages(ai.contextPrompt, settings)),
            aiKey('explain', fillLanguages(ai.customPrompt, settings)),
            aiKey('images', imagePromptKey(optionsOf('images'))),
        ]));
        if (sequence !== renderSequence) { return; }
        responses.cancelPendingExcept(keepRunning(keep));
    }
    closeViewer();
    closeSavedCards();
    closeAudioMenu();
    stopAudio();
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
    setCount($('btn-saved'), 0);
    setCount($('btn-anki-notes'), 0);

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
        } else if (d.kind === 'ai-context' && isConfigured(optionsOf('context')) && payload.sentence && payload.sentence.trim() !== payload.word) {
            const body = section('Tradutor de contexto', {ai: true, onRegenerate: (b) => void runContext(b, true)});
            shown++;
            if (ai.autoTranslateContext) {
                void runContext(body);
            } else {
                body.innerHTML = `<button class="ghost explain-btn">${icon('sparkle', 'sm')}Traduzir</button>`;
                body.querySelector('button').addEventListener('click', () => void runContext(body));
            }
        } else if (d.kind === 'ai-images' && payload.word) {
            const body = section('Imagens', {ai: ai.imageQueryUseAi !== false && isConfigured(optionsOf('images')), onRegenerate: (b) => void runImages(b, true)});
            shown++;
            // Like "Explicar": the pictures come on a click, or at once with the automatic option.
            if (ai.autoShowImages === true) {
                void runImages(body);
            } else {
                body.innerHTML = `<button class="ghost explain-btn">${icon('image', 'sm')}Mostrar imagens</button>`;
                body.querySelector('button').addEventListener('click', () => void runImages(body));
            }
        } else if (d.kind === 'mdict' && renderDictionary(d)) { shown++; }
    }
    if (shown === 0) {
        $('sections').innerHTML = '<div class="empty"><strong>Nenhum resultado</strong>Adicione dicionários ou configure a IA na janela do Lookitan.</div>';
    }
    void refreshAnkiButton();
    // "Tocar o áudio automaticamente": the word as found on screen or in the first entry.
    if (normalizeAudioOptions(settings.audio).autoPlay && payload.word) {
        void playWord(state, payload.entries?.[0]?.headword || payload.word);
    }
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
    closeViewer();
    closeSavedCards();
    closeAudioMenu();
    stopAudio();
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
$('btn-saved').addEventListener('click', () => { void openSavedCards(); });
$('btn-anki-notes').addEventListener('click', () => { void openAnkiNotes(); });
document.addEventListener('keydown', (ev) => {
    if (viewer && (ev.key === 'ArrowLeft' || ev.key === 'ArrowRight')) { stepViewer(ev.key === 'ArrowLeft' ? -1 : 1); return; }
    // With a picture enlarged, + − 0 zoom the picture instead of the pop-up.
    if (viewer && (ev.key === '+' || ev.key === '=')) { ev.preventDefault?.(); zoomBy(ZOOM_STEP); return; }
    if (viewer && ev.key === '-') { ev.preventDefault?.(); zoomBy(1 / ZOOM_STEP); return; }
    if (viewer && ev.key === '0') { ev.preventDefault?.(); zoomBy(1 / (viewer.zoom?.scale || 1)); return; }
    // Esc closes an enlarged picture or the saved cards first, then the pop-up.
    if (ev.key === 'Escape') { if (!closeViewer() && !closeSavedCards()) { closePopup(); } }
    if (ev.ctrlKey && (ev.key === '+' || ev.key === '=')) { zoom = Math.min(2, zoom + 0.1); applyZoom(); }
    if (ev.ctrlKey && ev.key === '-') { zoom = Math.max(0.6, zoom - 0.1); applyZoom(); }
    if (ev.ctrlKey && ev.key === '0') { zoom = 1; applyZoom(); }
});
applyZoom();
/**
 * The first Japanese, Chinese or Korean lookup waits while the word analyzer downloads or loads
 * (lib.rs `wait_for_segmenter`); its lookup replaces this when it is ready.
 * @param {{phase: 'downloading'|'loading', label: string, percent?: number, megabytes?: number}} wait
 */
function showWaiting(wait) {
    renderSequence++;
    let box = $('sections').querySelector('.waiting');
    if (!box) {
        closeViewer();
        closeSavedCards();
        closeAudioMenu();
        stopAudio();
        $('query').value = '';
        $('notice').hidden = true;
        $('meta').textContent = '';
        box = document.createElement('div');
        box.className = 'empty waiting';
        box.innerHTML = '<strong></strong><span class="waiting-detail"></span><div class="waiting-bar"><i></i></div>'
            + '<span class="hint">Só na primeira vez. A consulta continua sozinha quando terminar.</span>';
        $('sections').replaceChildren(box);
    }
    const downloading = wait.phase === 'downloading';
    box.querySelector('strong').textContent = downloading ? 'Baixando a separação de palavras' : 'Carregando a separação de palavras';
    box.querySelector('.waiting-detail').textContent = downloading ? `${wait.label}: ${wait.percent}% de ${wait.megabytes} MB` : wait.label;
    box.querySelector('.hint').hidden = !downloading;
    const bar = box.querySelector('.waiting-bar');
    bar.classList.toggle('busy', !downloading);
    bar.querySelector('i').style.width = downloading ? `${wait.percent}%` : '';
}

void listen('lookup', (event) => { void render(event.payload); });
void listen('segmenter-wait', (event) => { showWaiting(event.payload); });
// Closed like Yomitan's pop-up (Esc, X, a click elsewhere, a lookup that ends without a pop-up):
// drop the AI requests nobody will see.
void listen('popup-hidden', () => {
    stopDictionaryAudio();
    closeViewer();
    closeSavedCards();
    closeAudioMenu();
    stopAudio();
    if (ai && Object.keys(KIND_DICTIONARY).some(cancels)) { responses.cancelPendingExcept(keepRunning(new Set())); }
});
void listen('settings-changed', async () => {
    try {
        const all = await loadAllSettings();
        settings = all.settings;
        ai = all.ai;
        anki = all.anki;
        // The note type or its fields may have changed.
        modelFieldNames = new Map();
        await refreshAnkiButton();
    } catch (e) { ankiStatus(`Configurações: ${e?.message || e}`, true); }
});
