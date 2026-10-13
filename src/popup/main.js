// Lookup popup: dictionary entries for the word under the pointer plus the BYOK AI explanation.

import {listen} from '@tauri-apps/api/event';
import {api, entryUrl, escapeHtml, renderMarkdown} from '../lib/api.js';
import {normalizeAiOptions} from '../lib/ai/defaults.js';
import {explain, isConfigured} from '../lib/ai/explain.js';

const $ = (id) => document.getElementById(id);

const EXPLAIN_PROMPT =
    "Explain '{{target}}' in detail as it is used in: '{{sentence}}'. " +
    'Cover its meaning here, nuance and register, and give two short example sentences.';

let ai = normalizeAiOptions(null);
let current = null;
let aiAbort = null;
let aiText = '';
let pinned = false;
let zoom = Number(localStorage.getItem('zoom') || '1') || 1;

function applyZoom() {
    $('content').style.zoom = String(zoom);
    localStorage.setItem('zoom', String(zoom));
}

async function refreshSettings() {
    try {
        const settings = await api.getSettings();
        ai = normalizeAiOptions(settings.ai);
    } catch (e) {
        console.error(e);
    }
}

function setNotice(text) {
    $('notice').hidden = !text;
    $('notice').textContent = text || '';
}

function renderContext(sentence, word) {
    const el = $('context');
    if (!sentence) {
        el.hidden = true;
        return;
    }
    el.hidden = false;
    const i = word ? sentence.indexOf(word) : -1;
    el.innerHTML = i < 0 ?
        escapeHtml(sentence) :
        `${escapeHtml(sentence.slice(0, i))}<mark>${escapeHtml(word)}</mark>${escapeHtml(sentence.slice(i + word.length))}`;
}

function renderEntries(entries) {
    const root = $('entries');
    root.replaceChildren();
    let card = null;
    let lastDict = null;
    for (const e of entries) {
        if (e.dictId !== lastDict) {
            card = document.createElement('article');
            card.className = 'card entry';
            const head = document.createElement('div');
            head.className = 'card-head';
            head.innerHTML = `<span class="card-title">${escapeHtml(e.dictName)}</span>` +
                (e.matched && e.matched !== current?.word ? `<span class="muted">${escapeHtml(e.matched)}</span>` : '');
            card.appendChild(head);
            root.appendChild(card);
            lastDict = e.dictId;
        }
        const frame = document.createElement('iframe');
        frame.loading = 'eager';
        frame.src = entryUrl(e.dictId, e.entry);
        frame.title = `${e.dictName}: ${e.headword}`;
        card.appendChild(frame);
    }
}

function cancelAi() {
    if (aiAbort) {
        aiAbort.abort();
        aiAbort = null;
    }
}

async function runAi(prompt) {
    if (!current?.word) { return; }
    cancelAi();
    const controller = new AbortController();
    aiAbort = controller;
    const body = $('ai-body');
    aiText = '';
    body.classList.add('loading');
    body.innerHTML = '';
    try {
        const text = await explain(ai, current.word, current.sentence || current.word, {
            signal: controller.signal,
            prompt,
            onChunk: (_delta, full) => {
                if (controller.signal.aborted) { return; }
                aiText = full;
                body.innerHTML = renderMarkdown(full);
            },
        });
        if (!controller.signal.aborted) {
            aiText = text;
            body.innerHTML = renderMarkdown(text);
        }
    } catch (e) {
        if (!controller.signal.aborted) {
            body.innerHTML = `<p class="error">${escapeHtml(e?.message || String(e))}</p>`;
        }
    } finally {
        body.classList.remove('loading');
    }
}

function renderAi(entries) {
    const card = $('ai');
    if (!isConfigured(ai)) {
        card.hidden = true;
        return;
    }
    card.hidden = false;
    const model = ai[ai.provider]?.model || '';
    $('ai-model').textContent = `${ai.provider}${model ? ' · ' + model : ''}`;
    const auto = ai.autoGenerateOnLookup || (entries.length === 0 && ai.unknownWordFallback);
    if (auto) {
        void runAi();
    } else {
        $('ai-body').innerHTML = '<p class="muted">Clique em ↻ para gerar a explicação.</p>';
    }
}

async function render(payload) {
    if (ai.cancelPendingRequests !== false) { cancelAi(); }
    await refreshSettings();
    current = payload;
    $('query').value = payload.word || '';
    const meta = [];
    if (payload.engine) { meta.push(payload.engine); }
    if (payload.ocrMs) { meta.push(`${payload.ocrMs} ms`); }
    $('meta').textContent = meta.join(' · ');
    setNotice(payload.notice);

    const empty = $('empty');
    if (payload.error) {
        renderContext('', '');
        $('entries').replaceChildren();
        $('ai').hidden = true;
        empty.hidden = false;
        empty.innerHTML = `<strong>${escapeHtml(payload.error)}</strong>Aponte para uma palavra e use o atalho de novo.`;
        return;
    }
    renderContext(payload.sentence, payload.word);
    renderEntries(payload.entries || []);
    renderAi(payload.entries || []);
    const nothing = (payload.entries || []).length === 0 && $('ai').hidden;
    empty.hidden = !nothing;
    if (nothing) {
        empty.innerHTML = '<strong>Nenhum resultado</strong>Adicione dicionários ou configure a IA em Configurações.';
    }
    $('content').scrollTop = 0;
}

async function lookup(text) {
    const t = text.trim();
    if (!t) { return; }
    const payload = await api.lookupText(t, current?.sentence || '');
    await render(payload);
}

window.addEventListener('message', (ev) => {
    const d = ev.data;
    if (!d || d.lookitan !== true) { return; }
    if (d.type === 'height') {
        for (const f of document.querySelectorAll('.entry iframe')) {
            if (new URL(f.src).pathname === d.path) {
                f.style.height = `${Math.min(Math.max(d.height + 4, 40), 4000)}px`;
            }
        }
    } else if (d.type === 'lookup' && typeof d.text === 'string') {
        void lookup(d.text);
    } else if (d.type === 'open' && typeof d.url === 'string') {
        void api.openExternal(d.url);
    }
});

$('search').addEventListener('submit', (ev) => {
    ev.preventDefault();
    void lookup($('query').value);
});
$('close').addEventListener('click', () => { void api.hidePopup(); });
$('settings').addEventListener('click', () => { void api.openSettings(); });
$('pin').addEventListener('click', () => {
    pinned = !pinned;
    $('pin').classList.toggle('on', pinned);
    void api.setPopupPinned(pinned);
});
$('zoom-in').addEventListener('click', () => { zoom = Math.min(2, zoom + 0.1); applyZoom(); });
$('zoom-out').addEventListener('click', () => { zoom = Math.max(0.6, zoom - 0.1); applyZoom(); });
$('ai-regen').addEventListener('click', () => { void runAi(); });
$('ai-explain').addEventListener('click', () => { void runAi(EXPLAIN_PROMPT); });
$('ai-copy').addEventListener('click', () => { if (aiText) { void navigator.clipboard.writeText(aiText); } });
document.addEventListener('keydown', (ev) => {
    if (ev.key === 'Escape') { void api.hidePopup(); }
});

applyZoom();
void refreshSettings();
void listen('lookup', (event) => { void render(event.payload); });
