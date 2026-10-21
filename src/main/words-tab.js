// Words tab: saved words with search and sort, details, Anki sync and export.

import {save as saveDialog} from '@tauri-apps/plugin-dialog';
import {invoke} from '@tauri-apps/api/core';
import {api, escapeHtml} from '../lib/api.js';
import {createConnection, buildNote, glossaryDictionaryNames, isFormatReady} from '../lib/anki/notes.js';
import {noteContextFromCard, toCsv, toQuizlet} from '../lib/cards.js';
import {icon} from '../lib/icons.js';
import {store} from './store.js';
import {el} from './ui.js';

let words = [];
let selectedId = null;

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
            el('div', {class: 'w'}, w.word, w.ankiNoteId ? el('span', {class: 'faint', style: 'margin-left:8px', html: icon('cards', 'sm'), title: 'No Anki'}) : null),
            el('div', {class: 'tr'}, w.translation || w.definition || ''));
        item.addEventListener('click', () => { selectedId = w.id; renderList(); });
        list.append(item);
    }
    renderDetail();
}

async function renderDetail() {
    const root = document.getElementById('words-detail');
    const w = words.find((x) => x.id === selectedId);
    root.replaceChildren();
    if (!w) { return; }
    const rows = [
        ['Forma de dicionário', w.dictionaryForm], ['Tradução', w.translation], ['Definição', w.definition],
        ['Frase', w.sentence], ['Tradução da frase', (w.contextTranslation || '').replace(/\*\*/g, '')],
        ['Leitura', w.reading], ['IPA', w.ipa ? `/${w.ipa}/` : ''], ['CEFR', w.cefr], ['Classe gramatical', (w.partOfSpeech || []).join(', ')],
        ['Sinônimos', (w.synonyms || []).join(', ')],
        ['Componentes', (w.components || []).map((c) => `${c.part}${c.reading ? ` (${c.reading})` : ''}: ${c.meaning}`).join(' · ')],
        ['Explicação', w.explanation], ['Janela', w.windowTitle], ['Salva em', new Date(w.createdAt).toLocaleString('pt-BR')],
    ].filter(([, v]) => v);
    const dl = el('dl');
    for (const [k, v] of rows) { dl.append(el('dt', {}, k), el('dd', {}, String(v))); }
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
    const note = el('p', {class: 'note'});
    const detail = el('div', {class: 'word-detail'}, el('h2', {}, w.word), dl, el('div', {class: 'actions'}, toAnki, del), note);
    root.append(detail);
    if (w.image) {
        const data = await api.wordImage(w.image);
        if (data) { detail.append(el('img', {src: `data:image/png;base64,${data}`, alt: ''})); }
    }
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
