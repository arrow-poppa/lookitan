// Lookup popup. Sections follow the dictionary order from the Dictionaries tab: the AI word card,
// the context translator and the MDict dictionaries. Top-right: save the word and add it to Anki.

import {listen} from '@tauri-apps/api/event';
import {api, entryUrl, escapeHtml, loadAllSettings, renderMarkdown} from '../lib/api.js';
import {explain, isConfigured} from '../lib/ai/explain.js';
import {fillLanguages, parseCard} from '../lib/ai/word-card.js';
import {buildNote, createConnection, isFormatReady} from '../lib/anki/notes.js';
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

function section(title, {ai: isAi = false} = {}) {
    const sec = document.createElement('section');
    sec.className = 'sec';
    sec.innerHTML = `<div class="sec-head"><span>${escapeHtml(title)}</span>${isAi ? icon('sparkle', 'sm') : ''}<span class="line"></span></div><div class="sec-body"></div>`;
    $('sections').append(sec);
    return sec.querySelector('.sec-body');
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

function renderCard(body, card) {
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
        ${card.translation ? `<div class="tr${hide ? ' hidden-tr' : ''}" title="${hide ? 'Clique para mostrar' : ''}">${escapeHtml(card.translation)}</div>` : ''}
        ${card.definition ? `<p class="def">${escapeHtml(card.definition)}</p>` : ''}
        <button class="ghost explain-btn">${icon('sparkle', 'sm')}Explicar</button>
        <div class="explanation ai-text" hidden></div>
        ${p.sentence ? `<details class="ctx"><summary>Contexto</summary><div class="body">${markWord(p.sentence, p.word)}</div></details>` : ''}`;
    body.querySelector('.speak')?.addEventListener('click', () => speak(headword));
    body.querySelector('.hidden-tr')?.addEventListener('click', (ev) => ev.currentTarget.classList.remove('hidden-tr'));
    body.querySelector('.explain-btn').addEventListener('click', () => void runExplain(body.querySelector('.explanation')));
}

async function runExplain(target) {
    target.hidden = false;
    target.innerHTML = shimmer();
    const lookup = state;
    try {
        const text = await explain(ai, lookup.payload.word, lookup.payload.sentence || lookup.payload.word, {
            signal: lookup.abort.signal,
            prompt: fillLanguages(ai.customPrompt, settings),
            onChunk: (_d, full) => { if (state === lookup) { target.innerHTML = renderMarkdown(full); } },
        });
        if (state === lookup) {
            lookup.explanation = text;
            target.innerHTML = renderMarkdown(text);
        }
    } catch (e) {
        if (state === lookup && !lookup.abort.signal.aborted) { target.innerHTML = `<p class="error">${escapeHtml(e?.message || String(e))}</p>`; }
    }
}

async function runCard(body) {
    const lookup = state;
    body.innerHTML = shimmer();
    try {
        const text = await explain({...ai, streamResponse: false}, lookup.payload.word, lookup.payload.sentence || lookup.payload.word, {
            signal: lookup.abort.signal,
            prompt: fillLanguages(ai.cardPrompt, settings),
        });
        if (state !== lookup) { return; }
        const card = parseCard(text);
        if (card) {
            lookup.aiCard = card;
            renderCard(body, card);
        } else {
            lookup.explanation = text;
            body.innerHTML = `<div class="ai-text">${renderMarkdown(text)}</div>`;
        }
        void refreshAnkiButton();
        if (settings.saveWordsAutomatically && !lookup.savedId) { void saveWord(); }
    } catch (e) {
        if (state === lookup && !lookup.abort.signal.aborted) { body.innerHTML = `<p class="error">${escapeHtml(e?.message || String(e))}</p>`; }
    }
}

async function runContext(body) {
    const lookup = state;
    const p = lookup.payload;
    if (!p.sentence) { body.innerHTML = '<p class="hint">Sem frase de contexto nesta consulta.</p>'; return; }
    body.innerHTML = '<div class="shimmer w80"></div><div class="shimmer w60"></div>';
    try {
        const text = await explain(ai, p.word, p.sentence, {
            signal: lookup.abort.signal,
            prompt: fillLanguages(ai.contextPrompt, settings),
            onChunk: (_d, full) => { if (state === lookup) { body.innerHTML = `<div class="translation">${highlightStars(full)}</div>`; } },
        });
        if (state === lookup) {
            lookup.contextTranslation = text;
            body.innerHTML = `<div class="translation">${highlightStars(text)}</div>`;
        }
    } catch (e) {
        if (state === lookup && !lookup.abort.signal.aborted) { body.innerHTML = `<p class="error">${escapeHtml(e?.message || String(e))}</p>`; }
    }
}

function highlightStars(text) {
    return escapeHtml(text.trim()).replace(/\*\*([^*]+)\*\*/g, '<mark>$1</mark>');
}

// ------------------------------------------------------------------ dictionaries

function renderMdictSections() {
    const entries = state.payload.entries || [];
    for (const d of dicts.filter((x) => x.kind === 'mdict' && x.enabled)) {
        const mine = entries.filter((e) => e.dictId === d.id);
        if (mine.length === 0) { continue; }
        const body = section(d.name);
        body.classList.add('entry');
        for (const e of mine) {
            const frame = document.createElement('iframe');
            frame.src = entryUrl(e.dictId, e.entry);
            frame.title = `${d.name}: ${e.headword}`;
            body.append(frame);
        }
    }
}

async function glossaries() {
    if (state.glossaries) { return state.glossaries; }
    const out = [];
    const seen = new Set();
    for (const e of state.payload.entries || []) {
        if (seen.has(e.dictId) || out.length >= 3) { continue; }
        seen.add(e.dictId);
        try { out.push({name: e.dictName, html: await api.entryHtml(e.dictId, e.entry)}); } catch (_e) { /* skip */ }
    }
    state.glossaries = out;
    return out;
}

// ------------------------------------------------------------------ saving and Anki

async function saveWord() {
    const lookup = state;
    if (!lookup || !lookup.payload.word) { return; }
    await glossaries();
    const card = cardFromLookup(lookup, settings);
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

function noteContext() {
    return {
        word: state.payload.word,
        headword: state.aiCard?.headword || state.payload.entries?.[0]?.headword,
        sentence: state.payload.sentence,
        glossaries: state.glossaries || [],
        aiCard: state.aiCard || {},
        aiExplanation: state.explanation,
        contextTranslation: state.contextTranslation,
        title: state.payload.app?.title,
        query: state.payload.word,
        screenshot: async () => (state.payload.recognition ? api.captureImage(state.payload.recognition, anki.screenshotArea || 'sentence') : null),
    };
}

function setAnki(mode, title) {
    const b = $('btn-anki');
    b.hidden = mode === 'hidden';
    b.dataset.mode = mode;
    b.innerHTML = icon(mode === 'exists' || mode === 'added' ? 'check' : 'plus');
    b.classList.toggle('on', mode === 'exists' || mode === 'added');
    b.disabled = mode === 'busy';
    b.title = title || '';
}

async function refreshAnkiButton() {
    if (!anki.enable || !isFormatReady(anki) || !state?.payload.word) { setAnki('hidden'); return; }
    if (state.ankiNoteId) { setAnki('added', 'Adicionada ao Anki — clique para abrir'); return; }
    setAnki('add', 'Adicionar ao Anki');
    if (!anki.checkForDuplicates) { return; }
    try {
        const conn = createConnection(anki);
        const note = await buildNote(noteContext(), anki, null);
        note.options.allowDuplicate = false;
        const [canAdd] = await conn.canAddNotes([note]);
        if (!canAdd) {
            setAnki(anki.duplicateBehavior === 'prevent' ? 'exists' : 'add',
                anki.duplicateBehavior === 'prevent' ? 'Já está no Anki — clique para abrir' : 'Já existe uma nota parecida; clique para adicionar outra');
            state.duplicate = true;
        }
    } catch (e) {
        setAnki('add', `Anki: ${e?.message || e}`);
    }
}

async function onAnkiClick() {
    const mode = $('btn-anki').dataset.mode;
    const conn = createConnection(anki);
    if (mode === 'added' || mode === 'exists') {
        try {
            if (state.ankiNoteId) {
                await conn.guiBrowseNote(state.ankiNoteId);
            } else {
                const note = await buildNote(noteContext(), anki, null);
                const [ids] = await conn.findNoteIds([note]);
                if (ids && ids.length) { await conn.guiBrowseNotes(ids); }
            }
        } catch (_e) { /* Anki closed */ }
        return;
    }
    setAnki('busy', 'Adicionando…');
    try {
        await glossaries();
        const note = await buildNote(noteContext(), anki, conn);
        const id = await conn.addNote(note);
        if (id === null) { throw new Error('o Anki recusou a nota'); }
        if (anki.suspendNewCards) {
            const cards = await conn.findCardsForNote(id);
            await conn.suspendCards(cards);
        }
        state.ankiNoteId = id;
        setAnki('added', 'Adicionada ao Anki — clique para abrir');
        if (state.savedId) { void saveWord(); }
    } catch (e) {
        setAnki('add', `Erro: ${e?.message || e}`);
    }
}

// ------------------------------------------------------------------ render

async function render(payload) {
    if (state && ai?.cancelPendingRequests !== false) { state.abort.abort(); }
    const all = await loadAllSettings();
    settings = all.settings;
    ai = all.ai;
    anki = all.anki;
    dicts = await api.listDictionaries();
    state = {payload, abort: new AbortController()};
    $('sections').replaceChildren();
    $('sections').scrollTop = 0;
    $('query').value = payload.word || '';
    $('notice').hidden = !payload.notice;
    $('notice').textContent = payload.notice || '';
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
            const body = section('IA — palavra no contexto', {ai: true});
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
            const body = section('Tradutor de contexto', {ai: true});
            shown++;
            if (ai.autoGenerateOnLookup) {
                void runContext(body);
            } else {
                body.innerHTML = `<button class="ghost explain-btn">${icon('sparkle', 'sm')}Traduzir</button>`;
                body.querySelector('button').addEventListener('click', () => void runContext(body));
            }
        }
    }
    const before = $('sections').children.length;
    renderMdictSections();
    shown += $('sections').children.length - before;
    if (shown === 0) {
        $('sections').innerHTML = '<div class="empty"><strong>Nenhum resultado</strong>Adicione dicionários ou configure a IA na janela do Lookitan.</div>';
    }
    void refreshAnkiButton();
}

async function lookup(text) {
    const t = text.trim();
    if (!t) { return; }
    const payload = await api.lookupText(t, state?.payload.sentence || '');
    payload.recognition = state?.payload.recognition ?? null;
    payload.app = state?.payload.app ?? null;
    await render(payload);
}

window.addEventListener('message', (ev) => {
    const d = ev.data;
    if (!d || d.lookitan !== true) { return; }
    if (d.type === 'height') {
        for (const f of document.querySelectorAll('.entry iframe')) {
            if (new URL(f.src).pathname === d.path) { f.style.height = `${Math.min(Math.max(d.height + 4, 40), 4000)}px`; }
        }
    } else if (d.type === 'lookup' && typeof d.text === 'string') {
        void lookup(d.text);
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
$('btn-close').addEventListener('click', () => { void api.hidePopup(); });
$('btn-pin').addEventListener('click', () => {
    pinned = !pinned;
    $('btn-pin').classList.toggle('on', pinned);
    void api.setPopupPinned(pinned);
});
$('btn-save').addEventListener('click', () => { void saveWord(); });
$('btn-anki').addEventListener('click', () => { void onAnkiClick(); });
document.addEventListener('keydown', (ev) => {
    if (ev.key === 'Escape') { void api.hidePopup(); }
    if (ev.ctrlKey && (ev.key === '+' || ev.key === '=')) { zoom = Math.min(2, zoom + 0.1); applyZoom(); }
    if (ev.ctrlKey && ev.key === '-') { zoom = Math.max(0.6, zoom - 0.1); applyZoom(); }
    if (ev.ctrlKey && ev.key === '0') { zoom = 1; applyZoom(); }
});
applyZoom();
void listen('lookup', (event) => { void render(event.payload); });
