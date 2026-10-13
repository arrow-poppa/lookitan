// Settings window.

import {getVersion} from '@tauri-apps/api/app';
import {listen} from '@tauri-apps/api/event';
import {open} from '@tauri-apps/plugin-dialog';
import {api, escapeHtml} from '../lib/api.js';
import {DEFAULT_PROMPT, normalizeAiOptions} from '../lib/ai/defaults.js';
import {DEFAULT_MODELS, explain} from '../lib/ai/explain.js';

const $ = (id) => document.getElementById(id);

let settings = null;
let ai = null;
let saveTimer = null;

// ---------------------------------------------------------------- saving

function setSaveState(text) {
    $('save-state').textContent = text;
}

function scheduleSave() {
    setSaveState('Salvando…');
    clearTimeout(saveTimer);
    saveTimer = setTimeout(async () => {
        settings.ai = ai;
        try {
            await api.saveSettings(settings);
            setSaveState('Salvo');
        } catch (e) {
            setSaveState(`Erro: ${e}`);
        }
    }, 400);
}

function bindValue(id, get, set, event = 'input') {
    const el = $(id);
    const read = () => (el.type === 'checkbox' ? el.checked : el.value);
    if (el.type === 'checkbox') { el.checked = Boolean(get()); } else { el.value = get() ?? ''; }
    el.addEventListener(event, () => { set(read()); scheduleSave(); });
}

// ---------------------------------------------------------------- tabs

for (const b of document.querySelectorAll('.sidebar button[data-tab]')) {
    b.addEventListener('click', () => {
        for (const x of document.querySelectorAll('.sidebar button[data-tab]')) { x.classList.toggle('active', x === b); }
        for (const t of document.querySelectorAll('.tab')) { t.hidden = t.id !== `tab-${b.dataset.tab}`; }
        if (b.dataset.tab === 'ocr') { void refreshEngine(); }
    });
}

// ---------------------------------------------------------------- general

function acceleratorFrom(ev) {
    const mods = [];
    if (ev.ctrlKey) { mods.push('CommandOrControl'); }
    if (ev.altKey) { mods.push('Alt'); }
    if (ev.shiftKey) { mods.push('Shift'); }
    if (ev.metaKey) { mods.push('Super'); }
    const code = ev.code;
    if (['ControlLeft', 'ControlRight', 'AltLeft', 'AltRight', 'ShiftLeft', 'ShiftRight', 'MetaLeft', 'MetaRight'].includes(code)) {
        return null;
    }
    let key = code;
    if (/^Key[A-Z]$/.test(code)) { key = code.slice(3); } else if (/^Digit\d$/.test(code)) { key = code.slice(5); }
    if (mods.length === 0 && !/^F\d{1,2}$/.test(key)) { return null; }
    return [...mods, key].join('+');
}

function setupGeneral() {
    const hk = $('hotkey');
    hk.value = settings.hotkey;
    hk.addEventListener('keydown', (ev) => {
        ev.preventDefault();
        const acc = acceleratorFrom(ev);
        if (acc) {
            hk.value = acc;
            settings.hotkey = acc;
            scheduleSave();
        }
    });
    $('hotkey-clear').addEventListener('click', () => {
        hk.value = '';
        settings.hotkey = '';
        scheduleSave();
    });
    bindValue('cap-w', () => settings.captureWidth, (v) => { settings.captureWidth = Math.max(200, Number(v) || 900); });
    bindValue('cap-h', () => settings.captureHeight, (v) => { settings.captureHeight = Math.max(60, Number(v) || 260); });
    $('copy-cmd').addEventListener('click', () => { void navigator.clipboard.writeText($('scan-cmd').value); });
}

// ---------------------------------------------------------------- OCR

async function setupOcr() {
    for (const r of document.querySelectorAll('input[name="engine"]')) {
        r.checked = r.value === settings.ocrEngine;
        r.addEventListener('change', () => {
            settings.ocrEngine = r.value;
            $('lens-key-field').hidden = r.value !== 'lens';
            scheduleSave();
        });
    }
    $('lens-key-field').hidden = settings.ocrEngine !== 'lens';
    bindValue('lens-key', () => settings.lensApiKey, (v) => { settings.lensApiKey = v.trim(); });
    const langs = await api.ocrLanguages();
    $('ocr-lang').innerHTML = langs.map((l) => `<option value="${l.tag}">${escapeHtml(l.name)} (${l.tag})</option>`).join('');
    bindValue('ocr-lang', () => settings.ocrLanguage, (v) => { settings.ocrLanguage = v; }, 'change');
    bindValue('use-gpu', () => settings.useGpu, (v) => { settings.useGpu = v; }, 'change');
}

async function refreshEngine() {
    const s = await api.engineStatus();
    $('gpu-field').hidden = s.session !== 'windows';
    $('wayland-hint').hidden = s.session !== 'kde-wayland' && s.session !== 'other-wayland';
    const parts = [];
    const sessionNames = {'windows': 'Windows', 'x11': 'Linux (X11)', 'kde-wayland': 'KDE Plasma (Wayland)', 'other-wayland': 'Linux (Wayland)'};
    parts.push(`Sessão: <strong>${sessionNames[s.session] || s.session}</strong>`);
    if (s.runtimeError) {
        parts.push(`<span class="error">ONNX Runtime: ${escapeHtml(s.runtimeError)}</span>`);
    } else if (!s.runtimeLoaded) {
        parts.push('ONNX Runtime: carregando…');
    }
    if (s.detModel) {
        parts.push(`Detecção: <strong>${s.detModel}</strong> na ${s.detOnGpu ? 'GPU (DirectML)' : 'CPU'}`);
        parts.push(`Reconhecimento: <strong>${s.recModel}</strong> na ${s.recOnGpu ? 'GPU (DirectML)' : 'CPU'}`);
    }
    if (s.error) { parts.push(`<span class="error">${escapeHtml(s.error)}</span>`); }
    $('engine-status').innerHTML = parts.join('<br>');
}

// ---------------------------------------------------------------- dictionaries

function renderDicts(list) {
    const ul = $('dict-list');
    if (!list.length) {
        ul.innerHTML = '<li class="muted">Nenhum dicionário instalado.</li>';
        return;
    }
    ul.innerHTML = list.map((d, i) => `
        <li data-id="${escapeHtml(d.id)}">
            <input type="checkbox" class="enable" ${d.enabled ? 'checked' : ''} title="Ativo">
            <div class="info">
                <div class="name">${escapeHtml(d.name || d.path)}</div>
                <div class="path" title="${escapeHtml(d.path)}">${escapeHtml(d.path)}</div>
                <div class="hint">${d.error ? `<span class="error">${escapeHtml(d.error)}</span>` :
                    d.loaded ? `${d.entries.toLocaleString()} verbetes${d.resourceFiles ? ` · ${d.resourceFiles} arquivo(s) de recursos` : ''}` :
                    d.enabled ? 'Carregando…' : 'Desativado'}</div>
            </div>
            <button class="icon up" ${i === 0 ? 'disabled' : ''} title="Subir">↑</button>
            <button class="icon down" ${i === list.length - 1 ? 'disabled' : ''} title="Descer">↓</button>
            <button class="icon remove" title="Remover">✕</button>
        </li>`).join('');
    for (const li of ul.querySelectorAll('li[data-id]')) {
        const id = li.dataset.id;
        li.querySelector('.enable').addEventListener('change', async (ev) => renderDicts(await api.updateDictionary(id, {enabled: ev.target.checked})));
        li.querySelector('.up').addEventListener('click', async () => renderDicts(await api.updateDictionary(id, {moveBy: -1})));
        li.querySelector('.down').addEventListener('click', async () => renderDicts(await api.updateDictionary(id, {moveBy: 1})));
        li.querySelector('.remove').addEventListener('click', async () => renderDicts(await api.removeDictionary(id)));
    }
}

function setupDicts() {
    $('add-dict').addEventListener('click', async () => {
        const path = await open({multiple: false, directory: false, filters: [{name: 'MDict', extensions: ['mdx']}]});
        if (!path) { return; }
        $('add-dict').disabled = true;
        $('add-dict').textContent = 'Carregando…';
        try {
            renderDicts(await api.addDictionary(path));
        } catch (e) {
            alertInline(String(e));
        } finally {
            $('add-dict').disabled = false;
            $('add-dict').textContent = 'Adicionar dicionário…';
        }
    });
}

function alertInline(text) {
    const li = document.createElement('li');
    li.innerHTML = `<span class="error">${escapeHtml(text)}</span>`;
    $('dict-list').prepend(li);
}

// ---------------------------------------------------------------- AI

function providerCfg() {
    return ai[ai.provider];
}

function refreshAiVisibility() {
    for (const el of document.querySelectorAll('#tab-ai [data-show]')) {
        el.hidden = !el.dataset.show.split(' ').includes(ai.provider);
    }
    for (const opt of document.querySelectorAll('#ai-thinking-intensity option[data-only]')) {
        opt.hidden = opt.dataset.only !== ai.provider;
    }
    const cfg = providerCfg();
    $('ai-key').value = cfg.apiKey || '';
    $('ai-model').value = cfg.model || '';
    $('ai-endpoint').value = ai.custom.endpoint || '';
    $('ai-gemini-thinking').value = ai.gemini.thinkingLevel || '';
    $('ai-thinking-mode').value = cfg.thinkingMode || '';
    $('ai-thinking-intensity').value = cfg.thinkingIntensity || '';
    $('ai-thinking-custom').value = ai.custom.customThinkingIntensity || '';
    $('ai-routing').value = ai.custom.providerRoutingMode || '';
    $('ai-slugs').value = ai.custom.providerSlugs || '';
    $('ai-fallbacks').checked = ai.custom.providerAllowFallbacks !== false;
    $('ai-body-json').value = ai.custom.requestBodyJson || '';
    $('ai-models').innerHTML = (DEFAULT_MODELS[ai.provider] || []).map((m) => `<option value="${escapeHtml(m)}">`).join('');
}

function setupAi() {
    $('ai-provider').value = ai.provider;
    $('ai-provider').addEventListener('change', (ev) => {
        ai.provider = ev.target.value;
        refreshAiVisibility();
        scheduleSave();
    });
    const onInput = (id, apply) => $(id).addEventListener('input', (ev) => { apply(ev.target.value); scheduleSave(); });
    onInput('ai-key', (v) => { providerCfg().apiKey = v.trim(); });
    onInput('ai-model', (v) => { providerCfg().model = v.trim(); });
    onInput('ai-endpoint', (v) => { ai.custom.endpoint = v.trim(); });
    onInput('ai-thinking-custom', (v) => { ai.custom.customThinkingIntensity = v; });
    onInput('ai-slugs', (v) => { ai.custom.providerSlugs = v; });
    onInput('ai-body-json', (v) => { ai.custom.requestBodyJson = v; });
    $('ai-gemini-thinking').addEventListener('change', (ev) => { ai.gemini.thinkingLevel = ev.target.value; scheduleSave(); });
    $('ai-thinking-mode').addEventListener('change', (ev) => { providerCfg().thinkingMode = ev.target.value; scheduleSave(); });
    $('ai-thinking-intensity').addEventListener('change', (ev) => { providerCfg().thinkingIntensity = ev.target.value; scheduleSave(); });
    $('ai-routing').addEventListener('change', (ev) => { ai.custom.providerRoutingMode = ev.target.value; scheduleSave(); });
    $('ai-fallbacks').addEventListener('change', (ev) => { ai.custom.providerAllowFallbacks = ev.target.checked; scheduleSave(); });

    bindValue('ai-prompt', () => ai.customPrompt, (v) => { ai.customPrompt = v; });
    bindValue('ai-system', () => ai.systemPrompt, (v) => { ai.systemPrompt = v; });
    $('ai-prompt-reset').addEventListener('click', () => {
        ai.customPrompt = DEFAULT_PROMPT;
        $('ai-prompt').value = DEFAULT_PROMPT;
        scheduleSave();
    });
    const temp = $('ai-temp');
    temp.value = String(ai.temperature);
    $('ai-temp-value').textContent = Number(ai.temperature).toFixed(2);
    temp.addEventListener('input', () => {
        ai.temperature = Number(temp.value);
        $('ai-temp-value').textContent = ai.temperature.toFixed(2);
        scheduleSave();
    });
    bindValue('ai-auto', () => ai.autoGenerateOnLookup, (v) => { ai.autoGenerateOnLookup = v; }, 'change');
    bindValue('ai-stream', () => ai.streamResponse, (v) => { ai.streamResponse = v; }, 'change');
    bindValue('ai-cancel', () => ai.cancelPendingRequests, (v) => { ai.cancelPendingRequests = v; }, 'change');
    bindValue('ai-fallback', () => ai.unknownWordFallback, (v) => { ai.unknownWordFallback = v; }, 'change');

    $('ai-test').addEventListener('click', async () => {
        const out = $('ai-test-result');
        out.className = 'muted';
        out.textContent = 'Testando…';
        try {
            const text = await explain(ai, 'serendipity', 'Finding that old book in the attic was pure serendipity.', {
                signal: new AbortController().signal,
                onChunk: (_d, full) => { out.textContent = full.slice(0, 160); },
            });
            out.className = 'ok';
            out.textContent = `Funcionou: ${text.slice(0, 160)}${text.length > 160 ? '…' : ''}`;
        } catch (e) {
            out.className = 'error';
            out.textContent = String(e?.message || e);
        }
    });
    refreshAiVisibility();
}

// ---------------------------------------------------------------- start

async function main() {
    settings = await api.getSettings();
    ai = normalizeAiOptions(settings.ai);
    $('version').textContent = `v${await getVersion()}`;
    setupGeneral();
    await setupOcr();
    setupDicts();
    setupAi();
    renderDicts(await api.listDictionaries());
    await refreshEngine();
    void listen('engine-status', () => { void refreshEngine(); });
    void listen('dictionaries-changed', async () => renderDicts(await api.listDictionaries()));
}

void main();
