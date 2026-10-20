// Configurações › Áudio: the word audio sources, tried in this order (as Yomitan's "Configure audio
// playback sources"): local audio databases, custom URLs (plain or JSON) and the system voice.

import {invoke} from '@tauri-apps/api/core';
import {open} from '@tauri-apps/plugin-dialog';
import {AUDIO_SOURCE_TYPES, audioFile, findAudio, normalizeAudioOptions} from '../lib/audio.js';
import {icon} from '../lib/icons.js';
import {store, save} from './store.js';
import {cardRow, el, select, textInput, toggle} from './ui.js';

const PLACEHOLDERS = {
    'custom': 'https://exemplo.com/audio?term={term}&reading={reading}',
    'custom-json': 'http://127.0.0.1:5050/?term={term}&reading={reading}',
};

const HINTS = {
    'local-db': 'O android.db (áudio dentro do arquivo) ou o entries.db do Local Audio Server for Yomichan, com as pastas <fonte>_files ao lado.',
    'custom': 'Uma URL que responde com o próprio áudio. Variáveis: {term}, {reading}, {language}.',
    'custom-json': 'Uma URL que responde com uma lista de áudios no formato do Yomitan (audioSourceList). Variáveis: {term}, {reading}, {language}.',
    'tts': 'A voz do sistema, no idioma do OCR. Sempre encontra algo; deixe por último.',
};

function audio() {
    store.settings.audio = normalizeAudioOptions(store.settings.audio);
    return store.settings.audio;
}

function sourceRow(root, source, i, count) {
    const options = audio();
    const redraw = () => renderAudioSection(root);
    const move = (to) => {
        const [item] = options.sources.splice(i, 1);
        options.sources.splice(to, 0, item);
        save();
        redraw();
    };
    const button = (iconName, title, onClick, disabled = false) => {
        const b = el('button', {class: 'ghost icon', title, 'aria-label': title, html: icon(iconName, 'sm')});
        b.disabled = disabled;
        b.addEventListener('click', onClick);
        return b;
    };
    const type = select(AUDIO_SOURCE_TYPES, source.type, (v) => { source.type = v; save(); redraw(); });
    type.setAttribute('data-setting', `audio.sources.${i}.type`);
    let field = null;
    if (source.type === 'local-db') {
        const path = textInput(source.path, (v) => { source.path = v.trim(); save(); }, {placeholder: 'android.db ou entries.db'});
        path.setAttribute('data-setting', `audio.sources.${i}.path`);
        const pick = el('button', {html: `${icon('folder', 'sm')}Escolher…`});
        pick.addEventListener('click', async () => {
            const chosen = await open({multiple: false, directory: false, filters: [{name: 'Banco de áudio', extensions: ['db', 'sqlite', 'sqlite3']}]});
            if (chosen) { source.path = String(chosen); save(); redraw(); }
        });
        field = el('div', {class: 'audio-field'}, path, pick);
    } else if (source.type === 'custom' || source.type === 'custom-json') {
        const url = textInput(source.url, (v) => { source.url = v.trim(); save(); }, {placeholder: PLACEHOLDERS[source.type]});
        url.setAttribute('data-setting', `audio.sources.${i}.url`);
        field = el('div', {class: 'audio-field'}, url);
    }
    return el('div', {class: 'card-row stack audio-source'},
        el('div', {class: 'head'},
            el('span', {class: 'num'}, String(i + 1)),
            el('div', {class: 'txt'}, type, el('div', {class: 'd'}, HINTS[source.type])),
            toggle(source.enabled, (v) => { source.enabled = v; save(); }),
            button('chevronUp', 'Subir', () => move(i - 1), i === 0),
            button('chevronDown', 'Descer', () => move(i + 1), i === count - 1),
            button('trash', 'Remover', () => { options.sources.splice(i, 1); save(); redraw(); })),
        field);
}

/** Looks a word up in every source and plays the first recording, to check the setup. */
function testRow() {
    const word = textInput('読む', () => {});
    const out = el('span', {class: 'muted'});
    const run = el('button', {class: 'primary', html: `${icon('speaker', 'sm')}Testar`});
    run.addEventListener('click', async () => {
        out.className = 'muted';
        out.textContent = 'Procurando…';
        const term = word.value.trim();
        const io = {invoke};
        try {
            const found = await findAudio(audio(), {term, reading: '', language: String(store.settings.ocrLanguage || 'en').split('-')[0]}, io);
            if (found.length === 0) { out.className = 'error'; out.textContent = 'Nenhum áudio encontrado.'; return; }
            out.className = 'ok';
            out.textContent = `${found.length} encontrado(s): ${found.slice(0, 4).map((f) => f.name).join(', ')}${found.length > 4 ? '…' : ''}`;
            const first = found.find((f) => f.kind !== 'tts');
            const file = first ? await audioFile(first, io) : null;
            if (file) {
                const bytes = Uint8Array.from(atob(file.data), (c) => c.charCodeAt(0));
                void new Audio(URL.createObjectURL(new Blob([bytes], {type: file.mime}))).play();
            } else if ('speechSynthesis' in window) {
                const u = new SpeechSynthesisUtterance(term);
                u.lang = store.settings.ocrLanguage || 'en-US';
                window.speechSynthesis.speak(u);
            }
        } catch (e) {
            out.className = 'error';
            out.textContent = String(e?.message || e);
        }
    });
    return el('div', {class: 'card-row'}, el('span', {class: 'ico', html: icon('speaker')}),
        el('div', {class: 'txt'}, el('div', {class: 't'}, 'Testar as fontes'), el('div', {class: 'd'}, 'Procura a palavra em todas as fontes ligadas e toca o primeiro áudio.')),
        el('div', {class: 'ctl audio-test'}, word, run, out));
}

export function renderAudioSection(root) {
    const options = audio();
    root.replaceChildren(
        cardRow({icon: 'play', title: 'Tocar o áudio automaticamente', desc: 'Toca o primeiro áudio encontrado assim que o popup abre.',
            control: toggle(options.autoPlay, (v) => { options.autoPlay = v; save(); })}),
        ...options.sources.map((source, i) => sourceRow(root, source, i, options.sources.length)),
        (() => {
            const add = el('button', {html: `${icon('plus', 'sm')}Adicionar fonte de áudio`});
            add.addEventListener('click', () => {
                options.sources.push({type: 'local-db', enabled: true, path: '', url: ''});
                save();
                renderAudioSection(root);
            });
            return el('div', {class: 'card-row'}, el('span', {class: 'ico', html: icon('list')}),
                el('div', {class: 'txt'}, el('div', {class: 't'}, 'Fontes de áudio'), el('div', {class: 'd'},
                    'Tentadas de cima para baixo no botão de áudio do popup e no marcador {audio} do Anki. Clique direito no botão do popup lista todos os áudios encontrados.')),
                el('div', {class: 'ctl'}, add));
        })(),
        testRow(),
    );
}
