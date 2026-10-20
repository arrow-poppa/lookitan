// Configurações › Áudio, laid out like Yomitan's "Configure audio playback sources": a list of
// sources in priority order (drag a row up or down, or Alt+↑/↓), each switched on or off, plus how
// the pop-up shows the other recordings found.

import {invoke} from '@tauri-apps/api/core';
import {open} from '@tauri-apps/plugin-dialog';
import {AUDIO_SOURCE_TYPES, audioFile, findAudio, mergeSubsources, normalizeAudioOptions} from '../lib/audio.js';
import {icon} from '../lib/icons.js';
import {store, save} from './store.js';
import {cardRow, el, select, sortableRow, textInput, toggle} from './ui.js';

const PLACEHOLDERS = {
    'custom': 'https://exemplo.com/audio?term={term}&reading={reading}',
    'custom-json': 'http://127.0.0.1:5050/?term={term}&reading={reading}',
};

const HINTS = {
    'local-db': 'O android.db (com o áudio dentro) ou o entries.db do Local Audio Server for Yomichan, com as pastas <fonte>_files ao lado.',
    'custom': 'Uma URL que responde com o próprio áudio. Variáveis: {term}, {reading}, {language}.',
    'custom-json': 'Uma URL que responde com uma lista de áudios no formato do Yomitan, como o servidor de áudio local do Anki. Variáveis: {term}, {reading}, {language}.',
    'tts': 'A voz do sistema, no idioma do OCR. Sempre encontra algo, por isso fica bem por último.',
};

/** The audio options, normalized once and then edited in place (rows keep references to them). */
function options() {
    if (!store.settings.audio || store.settings.audio.normalized !== true) {
        store.settings.audio = {...normalizeAudioOptions(store.settings.audio), normalized: true};
    }
    return store.settings.audio;
}

/** Display names of local-audio-yomichan's sources. */
const SUBSOURCE_NAMES = {nhk16: 'NHK16', shinmeikai8: 'Shinmeikai 8 (SMK8)', forvo: 'Forvo', jpod: 'JPod101', jpod_alternate: 'JPod101 Alt', ozk5: 'OZK5', daijisen: 'Daijisen', taas: 'TAAS'};

/**
 * The sources inside a local database (nhk16, forvo…), in the user's priority order: drag to
 * reorder, switch off the ones not wanted. Read from the file each time the row is drawn.
 */
function subsourceList(source) {
    const box = el('div', {class: 'audio-subsources'}, el('div', {class: 'd muted'}, 'Lendo as fontes do arquivo…'));
    const draw = () => {
        const list = el('ol', {class: 'audio-sources sub'});
        source.subsources.forEach((sub, i) => {
            const row = el('li', {class: 'audio-sub', tabindex: '0', title: 'Arraste para mudar a prioridade'},
                el('span', {class: 'grip', html: icon('drag', 'sm')}),
                el('span', {class: 'num'}, String(i + 1)),
                el('span', {class: 'name'}, SUBSOURCE_NAMES[sub.name] || sub.name),
                toggle(sub.enabled, (v) => { sub.enabled = v; save(); }));
            sortableRow(list, row, {ignore: '.switch', onDrop: (from, to) => {
                if (to !== from) { source.subsources.splice(to, 0, source.subsources.splice(from, 1)[0]); save(); }
                draw();
            }});
            list.append(row);
        });
        box.replaceChildren(el('div', {class: 'd'}, 'Fontes dentro deste arquivo, em ordem de prioridade (arraste para reordenar):'), list);
    };
    invoke('audio_db_sources', {path: source.path}).then((names) => {
        const merged = mergeSubsources(source.subsources, names || []);
        if (JSON.stringify(merged) !== JSON.stringify(source.subsources)) { source.subsources = merged; save(); }
        draw();
    }, (e) => { box.replaceChildren(el('div', {class: 'd error'}, String(e?.message || e))); });
    return box;
}

function sourceRow(redraw, source, i) {
    const all = options().sources;
    const type = select(AUDIO_SOURCE_TYPES, source.type, (v) => { source.type = v; save(); redraw(); });
    type.setAttribute('data-setting', `audio.sources.${i}.type`);
    let field = null;
    if (source.type === 'local-db') {
        const path = textInput(source.path, (v) => { source.path = v.trim(); save(); }, {placeholder: 'Caminho do android.db ou entries.db'});
        path.setAttribute('data-setting', `audio.sources.${i}.path`);
        // A path typed by hand: read the file's sources once the field is left.
        path.addEventListener('change', () => redraw());
        const pick = el('button', {html: `${icon('folder', 'sm')}Escolher…`});
        pick.addEventListener('click', async () => {
            const chosen = await open({multiple: false, directory: false, filters: [{name: 'Banco de áudio', extensions: ['db', 'sqlite', 'sqlite3']}]});
            if (chosen) { source.path = String(chosen); save(); redraw(); }
        });
        field = el('div', {}, el('div', {class: 'audio-field'}, path, pick), source.path ? subsourceList(source) : null);
    } else if (source.type === 'custom' || source.type === 'custom-json') {
        const url = textInput(source.url, (v) => { source.url = v.trim(); save(); }, {placeholder: PLACEHOLDERS[source.type]});
        url.setAttribute('data-setting', `audio.sources.${i}.url`);
        field = el('div', {class: 'audio-field'}, url);
    }
    const remove = el('button', {class: 'ghost icon', title: 'Remover esta fonte', 'aria-label': 'Remover esta fonte', html: icon('trash', 'sm')});
    remove.addEventListener('click', () => { all.splice(i, 1); save(); redraw(); });
    const row = el('li', {class: 'audio-source', tabindex: '0', title: 'Arraste para mudar a prioridade (ou Alt+↑/↓)'},
        el('span', {class: 'grip', html: icon('drag', 'sm')}),
        el('span', {class: 'num'}, String(i + 1)),
        el('div', {class: 'audio-body'},
            el('div', {class: 'audio-line'}, type, toggle(source.enabled, (v) => { source.enabled = v; save(); }), remove),
            el('div', {class: 'd muted'}, HINTS[source.type]),
            field));
    row.addEventListener('keydown', (ev) => {
        if (!ev.altKey || (ev.key !== 'ArrowUp' && ev.key !== 'ArrowDown') || ev.target !== row) { return; }
        ev.preventDefault?.();
        const to = i + (ev.key === 'ArrowUp' ? -1 : 1);
        if (to < 0 || to >= all.length) { return; }
        all.splice(to, 0, all.splice(i, 1)[0]);
        save();
        redraw(to);
    });
    return row;
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
            const found = await findAudio(options(), {term, reading: '', language: String(store.settings.ocrLanguage || 'en').split('-')[0]}, io);
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
        el('div', {class: 'txt'}, el('div', {class: 't'}, 'Testar as fontes'), el('div', {class: 'd'}, 'Procura a palavra em todas as fontes ligadas, na ordem da lista, e toca o primeiro áudio.')),
        el('div', {class: 'ctl audio-test'}, word, run, out));
}

export function renderAudioSection(root, focusIndex = null) {
    const audio = options();
    const redraw = (focus = null) => renderAudioSection(root, focus);
    const list = el('ol', {class: 'audio-sources'});
    audio.sources.forEach((source, i) => {
        const row = sourceRow(redraw, source, i);
        sortableRow(list, row, {
            ignore: '.switch, input, select, textarea, button, .audio-subsources',
            onDrop: (from, to) => {
                if (to !== from) { audio.sources.splice(to, 0, audio.sources.splice(from, 1)[0]); save(); }
                redraw();
            },
        });
        list.append(row);
    });
    const add = el('button', {html: `${icon('plus', 'sm')}Adicionar fonte`});
    add.addEventListener('click', () => {
        audio.sources.push({type: 'local-db', enabled: true, path: '', url: ''});
        save();
        redraw();
    });
    root.replaceChildren(
        cardRow({icon: 'play', title: 'Tocar o áudio automaticamente', desc: 'Toca o primeiro áudio encontrado assim que o popup abre.',
            control: toggle(audio.autoPlay, (v) => { audio.autoPlay = v; save(); })}),
        cardRow({icon: 'speaker', title: 'Os outros áudios no popup', desc: 'O botão de áudio toca o primeiro encontrado. Os demais aparecem num menu do clique direito ou como botões ao lado.', wide: true,
            control: select([
                {value: 'menu', label: 'Menu no clique direito do botão de áudio'},
                {value: 'buttons', label: 'Um botão para cada áudio'},
            ], audio.others, (v) => { audio.others = v; save(); })}),
        el('div', {class: 'card-row stack'},
            el('div', {class: 'head'}, el('span', {class: 'ico', html: icon('list')}),
                el('div', {class: 'txt'}, el('div', {class: 't'}, 'Fontes de áudio'),
                    el('div', {class: 'd'}, 'Em ordem de prioridade: a de cima é tentada primeiro. Arraste uma fonte para cima ou para baixo para mudar a ordem.')),
                add),
            list),
        testRow(),
    );
    if (focusIndex !== null) { list.children[focusIndex]?.focus?.(); }
}
