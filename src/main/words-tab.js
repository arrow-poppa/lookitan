// Words tab: saved words with search and sort, details, Anki sync and export.

import {save as saveDialog} from '@tauri-apps/plugin-dialog';
import {invoke} from '@tauri-apps/api/core';
import {api, escapeHtml} from '../lib/api.js';
import {createConnection, buildNote, glossaryDictionaryNames, isFormatReady} from '../lib/anki/notes.js';
import {noteContextFromCard, toCsv, toQuizlet} from '../lib/cards.js';
import {translationText} from '../lib/ai/word-card.js';
import {icon} from '../lib/icons.js';
import {store} from './store.js';
import {el} from './ui.js';

let words = [];
let selectedId = null;
// The card the detail shows and its `updatedAt`: an edit saved from the detail comes back as
// "words-changed" and must not redraw the form being typed in.
let shown = {id: null, updatedAt: null};

/**
 * The fields of a saved word that can be edited, as Lookupper's card editor has them: key, label
 * and kind (`text`, `area`, `list` for comma-separated values, or `cefr`).
 */
const FIELDS = [
    ['dictionaryForm', 'Forma de dicionário', 'text'], ['translation', 'Tradução', 'text'],
    ['translationAlternatives', 'Outras traduções', 'list'],
    ['definition', 'Definição', 'area'], ['sentence', 'Frase', 'area'],
    ['contextTranslation', 'Tradução da frase', 'area'], ['reading', 'Leitura', 'text'],
    ['transliteration', 'Transliteração', 'text'], ['ipa', 'IPA', 'text'], ['cefr', 'CEFR', 'cefr'],
    ['form', 'Forma na tela', 'list'], ['partOfSpeech', 'Classe gramatical', 'list'],
    ['synonyms', 'Sinônimos', 'list'], ['explanation', 'Explicação', 'area'],
];
const CEFR_LEVELS = ['', 'A1', 'A2', 'B1', 'B2', 'C1', 'C2'];

/** A field's text in the editor. The context translation's ** marks are left out. */
function fieldText(w, key, kind) {
    const value = w[key];
    if (kind === 'list') { return (Array.isArray(value) ? value : []).join(', '); }
    if (key === 'contextTranslation') { return String(value || '').replace(/\*\*/g, ''); }
    return String(value || '');
}

function fieldValue(text, kind) {
    if (kind === 'list') { return text.split(',').map((s) => s.trim()).filter(Boolean); }
    return text.trim();
}

export async function initWordsTab() {
    document.getElementById('words-search').addEventListener('input', renderList);
    document.getElementById('words-sort').addEventListener('change', renderList);
    document.getElementById('anki-sync').addEventListener('click', syncAll);
    document.getElementById('words-export').addEventListener('click', exportWords);
    await refreshWords();
}

export async function refreshWords() {
    words = await api.listWords();
    renderList();
}

function filtered() {
    const q = document.getElementById('words-search').value.trim().toLowerCase();
    const sort = document.getElementById('words-sort').value;
    let out = words.filter((w) => !q || [w.word, w.translation, w.definition, w.sentence, w.contextTranslation].some((v) => String(v || '').toLowerCase().includes(q)));
    if (sort === 'oldest') { out = [...out].sort((a, b) => a.createdAt - b.createdAt); } else if (sort === 'az') { out = [...out].sort((a, b) => a.word.localeCompare(b.word)); } else { out = [...out].sort((a, b) => b.createdAt - a.createdAt); }
    return out;
}

function renderList() {
    const list = document.getElementById('words-list');
    const items = filtered();
    list.replaceChildren();
    if (words.length === 0) {
        list.append(el('div', {class: 'empty', html: `${icon('bookmark', 'lg')}<strong>Nenhuma palavra salva ainda</strong>Consulte uma palavra e clique no marcador no canto superior direito do popup.`}));
        document.getElementById('words-detail').replaceChildren();
        return;
    }
    if (!items.some((w) => w.id === selectedId)) { selectedId = items[0]?.id || null; }
    for (const w of items) {
        const item = el('div', {class: `word-item${w.id === selectedId ? ' sel' : ''}`},
            el('div', {class: 'w'}, el('bdi', {}, w.word),
                w.cefr ? el('span', {class: 'cefr-badge'}, w.cefr) : null,
                w.ankiNoteId ? el('span', {class: 'faint', style: 'margin-left:8px', html: icon('cards', 'sm'), title: 'No Anki'}) : null),
            el('div', {class: 'tr'}, translationText(w) || w.definition || ''));
        item.addEventListener('click', () => { selectedId = w.id; renderList(); });
        list.append(item);
    }
    const w = words.find((x) => x.id === selectedId);
    if (!w || shown.id !== w.id || shown.updatedAt !== w.updatedAt) { renderDetail(); }
}

/** Saves an edit of the card shown, keeping the form as it is. */
async function commit(w, changes, status) {
    Object.assign(w, changes);
    try {
        const saved = await api.saveWord(w);
        Object.assign(w, saved);
        if (shown.id === w.id) { shown.updatedAt = w.updatedAt; }
        status.textContent = w.ankiNoteId ? 'Salvo. A nota que já está no Anki não muda.' : 'Salvo.';
        renderList();
    } catch (e) {
        status.textContent = `Não foi possível salvar: ${e?.message || e}`;
    }
}

function fieldControl(w, key, kind, label, status) {
    let control;
    if (kind === 'cefr') {
        control = el('select', {'aria-label': label}, ...CEFR_LEVELS.map((level) => el('option', {value: level}, level || '—')));
    } else {
        control = el(kind === 'area' ? 'textarea' : 'input', {'aria-label': label, rows: kind === 'area' ? 3 : null, type: kind === 'area' ? null : 'text', dir: 'auto'});
    }
    control.value = kind === 'cefr' ? String(w[key] || '') : fieldText(w, key, kind);
    control.setAttribute('data-field', key);
    control.addEventListener('change', () => {
        const value = kind === 'cefr' ? control.value : fieldValue(control.value, kind);
        if (JSON.stringify(value) === JSON.stringify(kind === 'cefr' ? String(w[key] || '') : fieldValue(fieldText(w, key, kind), kind))) { return; }
        void commit(w, {[key]: value}, status);
    });
    return control;
}

async function renderDetail() {
    const root = document.getElementById('words-detail');
    const w = words.find((x) => x.id === selectedId);
    root.replaceChildren();
    shown = {id: w?.id ?? null, updatedAt: w?.updatedAt ?? null};
    if (!w) { return; }
    const status = el('p', {class: 'note'});
    const title = el('input', {class: 'word-title', type: 'text', dir: 'auto', 'aria-label': 'Palavra'});
    title.value = w.word || '';
    title.setAttribute('data-field', 'word');
    title.addEventListener('change', () => {
        const word = title.value.trim();
        if (word && word !== w.word) { void commit(w, {word}, status); }
    });
    const dl = el('dl', {class: 'word-fields'});
    for (const [key, label, kind] of FIELDS) {
        dl.append(el('dt', {}, label), el('dd', {}, fieldControl(w, key, kind, label, status)));
    }
    const components = (w.components || []).map((c) => `${c.part}${c.reading ? ` (${c.reading})` : ''}: ${c.meaning}`).join(' · ');
    for (const [k, v] of [['Componentes', components], ['Janela', w.windowTitle], ['Salva em', new Date(w.createdAt).toLocaleString('pt-BR')]]) {
        if (v) { dl.append(el('dt', {}, k), el('dd', {class: 'fixed'}, String(v))); }
    }
    const toAnki = el('button', {class: 'primary', html: `${icon('cards', 'sm')}${w.ankiNoteId ? 'Abrir no Anki' : 'Adicionar ao Anki'}`});
    toAnki.addEventListener('click', async () => {
        if (w.ankiNoteId) {
            try { await createConnection(store.anki).guiBrowseNote(w.ankiNoteId); }
            catch (e) { note.textContent = `Anki: ${e?.message || e}`; }
            return;
        }
        toAnki.disabled = true;
        try {
            const msg = await addToAnki(w);
            note.textContent = msg;
            // Keep configuration and connection errors visible instead of replacing their message.
            if (msg === 'Adicionada ao Anki.') {
                await refreshWords();
                document.getElementById('save-state').textContent = msg;
            }
        } catch (e) { note.textContent = `Anki: ${e?.message || e}`; }
        finally { toAnki.disabled = false; }
    });
    const del = el('button', {html: `${icon('trash', 'sm')}Excluir`});
    del.addEventListener('click', async () => { await api.deleteWord(w.id); await refreshWords(); });
    const note = status;
    const detail = el('div', {class: 'word-detail'}, title, dl, el('div', {class: 'actions'}, toAnki, del), note);
    root.append(detail);
    if (w.image) {
        const data = await api.wordImage(w.image);
        if (data && shown.id === w.id) { detail.append(imageSection(w, `data:image/png;base64,${data}`, status)); }
    }
}

// ------------------------------------------------------------------ picture and crop

/**
 * The saved picture, cropped to `imageCrop` ([x, y, width, height] of the whole image), with a
 * button that opens the crop editor, like Lookupper's card image and its crop dialog.
 */
function imageSection(w, src, status) {
    const canvas = el('canvas', {class: 'word-shot', 'aria-label': 'Imagem do contexto'});
    const image = new Image();
    const draw = () => {
        const [x, y, width, height] = cropOf(w, image);
        canvas.width = width;
        canvas.height = height;
        canvas.getContext('2d').drawImage(image, x, y, width, height, 0, 0, width, height);
    };
    image.onload = draw;
    image.src = src;
    const crop = el('button', {class: 'ghost', html: `${icon('crop', 'sm')}Recortar`});
    crop.addEventListener('click', () => openCropper(image, cropOf(w, image), async (rect) => {
        await commit(w, {imageCrop: rect}, status);
        draw();
    }));
    return el('div', {class: 'word-image'},
        el('div', {class: 'word-image-head'}, el('span', {class: 'caption-inline'}, 'Imagem'), crop),
        canvas);
}

/** The card's crop clamped to the image; the whole image without one (cards saved before). */
function cropOf(w, image) {
    const [iw, ih] = [image.naturalWidth || image.width, image.naturalHeight || image.height];
    const c = Array.isArray(w.imageCrop) && w.imageCrop.length === 4 ? w.imageCrop.map(Number) : [0, 0, iw, ih];
    const x = Math.min(Math.max(0, Math.round(c[0])), Math.max(0, iw - 1));
    const y = Math.min(Math.max(0, Math.round(c[1])), Math.max(0, ih - 1));
    return [x, y, Math.max(1, Math.min(Math.round(c[2]), iw - x)), Math.max(1, Math.min(Math.round(c[3]), ih - y))];
}

const MIN_CROP = 8;

/**
 * A dialog over the window with the whole picture and a free crop box: drag inside the box to
 * move it, drag a handle to resize it, drag elsewhere to draw a new one. `onSave` gets
 * [x, y, width, height] in image pixels, or null for the whole picture.
 */
function openCropper(image, start, onSave) {
    const [iw, ih] = [image.naturalWidth, image.naturalHeight];
    let rect = {x: start[0], y: start[1], w: start[2], h: start[3]};
    const picture = el('img', {src: image.src, alt: '', draggable: 'false'});
    const box = el('div', {class: 'crop-box'},
        ...['nw', 'n', 'ne', 'e', 'se', 's', 'sw', 'w'].map((h) => el('span', {class: `crop-handle ${h}`, 'data-handle': h})));
    const stage = el('div', {class: 'crop-stage'}, picture, box);
    const size = el('span', {class: 'note'});
    const place = () => {
        box.style.left = `${(rect.x / iw) * 100}%`;
        box.style.top = `${(rect.y / ih) * 100}%`;
        box.style.width = `${(rect.w / iw) * 100}%`;
        box.style.height = `${(rect.h / ih) * 100}%`;
        size.textContent = `${Math.round(rect.w)} × ${Math.round(rect.h)} px`;
    };
    const close = () => { dialog.remove(); document.removeEventListener('keydown', keys, true); };
    const save = (value) => { close(); void onSave(value); };
    const ok = el('button', {class: 'primary'}, 'Salvar recorte');
    ok.addEventListener('click', () => save([rect.x, rect.y, rect.w, rect.h].map(Math.round)));
    const whole = el('button', {}, 'Imagem inteira');
    whole.addEventListener('click', () => save(null));
    const cancel = el('button', {class: 'ghost'}, 'Cancelar');
    cancel.addEventListener('click', close);
    const dialog = el('div', {class: 'crop-dialog', role: 'dialog', 'aria-label': 'Recortar a imagem'},
        el('div', {class: 'crop-panel'},
            el('h3', {}, 'Recortar a imagem'),
            stage,
            el('div', {class: 'actions'}, size, el('span', {class: 'spacer'}), cancel, whole, ok)));
    const keys = (ev) => {
        if (ev.key === 'Escape') { ev.preventDefault(); close(); }
        if (ev.key === 'Enter') { ev.preventDefault(); ok.click(); }
    };
    document.addEventListener('keydown', keys, true);

    // Pointer positions in image pixels.
    const at = (ev) => {
        const r = picture.getBoundingClientRect();
        return {x: Math.min(iw, Math.max(0, ((ev.clientX - r.left) / r.width) * iw)), y: Math.min(ih, Math.max(0, ((ev.clientY - r.top) / r.height) * ih))};
    };
    let drag = null;
    stage.addEventListener('pointerdown', (ev) => {
        ev.preventDefault();
        const p = at(ev);
        const handle = ev.target?.dataset?.handle;
        drag = handle ? {handle, from: {...rect}} : ev.target === box ? {handle: 'move', p, from: {...rect}} : {handle: 'new', p};
        stage.setPointerCapture?.(ev.pointerId);
    });
    stage.addEventListener('pointermove', (ev) => {
        if (!drag) { return; }
        const p = at(ev);
        if (drag.handle === 'move') {
            rect.x = Math.min(iw - rect.w, Math.max(0, drag.from.x + p.x - drag.p.x));
            rect.y = Math.min(ih - rect.h, Math.max(0, drag.from.y + p.y - drag.p.y));
        } else if (drag.handle === 'new') {
            rect = {x: Math.min(drag.p.x, p.x), y: Math.min(drag.p.y, p.y), w: Math.max(MIN_CROP, Math.abs(p.x - drag.p.x)), h: Math.max(MIN_CROP, Math.abs(p.y - drag.p.y))};
            rect.w = Math.min(rect.w, iw - rect.x);
            rect.h = Math.min(rect.h, ih - rect.y);
        } else {
            const f = drag.from;
            let [x0, y0, x1, y1] = [f.x, f.y, f.x + f.w, f.y + f.h];
            if (drag.handle.includes('w')) { x0 = Math.min(p.x, x1 - MIN_CROP); }
            if (drag.handle.includes('e')) { x1 = Math.max(p.x, x0 + MIN_CROP); }
            if (drag.handle.includes('n')) { y0 = Math.min(p.y, y1 - MIN_CROP); }
            if (drag.handle.includes('s')) { y1 = Math.max(p.y, y0 + MIN_CROP); }
            rect = {x: x0, y: y0, w: x1 - x0, h: y1 - y0};
        }
        place();
    });
    const end = () => { drag = null; };
    stage.addEventListener('pointerup', end);
    stage.addEventListener('pointercancel', end);
    place();
    document.body.append(dialog);
}

async function addToAnki(w) {
    const options = JSON.parse(JSON.stringify(store.anki));
    if (!isFormatReady(options)) { return 'Configure o Anki em Configurações › Anki (baralho, tipo de nota e campos).'; }
    try {
        const conn = createConnection(options);
        const note = await buildNote(noteContextFromCard(w, glossaryDictionaryNames(store.settings?.dictionaries), store.settings || {}), options, conn);
        const id = await conn.addNote(note);
        if (id === null) { return 'O Anki não aceitou a nota (talvez já exista).'; }
        if (options.suspendNewCards) {
            const cards = await conn.findCardsForNote(id);
            await conn.suspendCards(cards);
        }
        await api.saveWord({...w, ankiNoteId: id});
        return 'Adicionada ao Anki.';
    } catch (e) {
        return `Anki: ${e?.message || e}`;
    }
}

async function syncAll() {
    const btn = document.getElementById('anki-sync');
    const pending = words.filter((w) => !w.ankiNoteId);
    if (pending.length === 0) { btn.title = 'Todas as palavras já estão no Anki.'; return; }
    btn.disabled = true;
    let ok = 0;
    let last = '';
    for (const w of pending) {
        last = await addToAnki(w);
        if (last === 'Adicionada ao Anki.') { ok++; }
    }
    btn.disabled = false;
    btn.title = `${ok} de ${pending.length} adicionadas. ${ok < pending.length ? last : ''}`;
    document.getElementById('save-state').textContent = btn.title;
    await refreshWords();
}

async function exportWords() {
    const items = filtered();
    if (items.length === 0) { return; }
    const path = await saveDialog({
        defaultPath: 'lookitan-palavras.csv',
        filters: [{name: 'Planilha CSV', extensions: ['csv']}, {name: 'Quizlet (texto)', extensions: ['txt']}],
    });
    if (!path) { return; }
    const content = path.toLowerCase().endsWith('.txt') ? toQuizlet(items) : `﻿${toCsv(items)}`;
    await invoke('write_text_file', {path, content});
    document.getElementById('save-state').textContent = `Exportadas ${items.length} palavras · ${escapeHtml(path)}`;
}
