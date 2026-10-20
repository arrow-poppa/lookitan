// Configurações › Áudio, laid out like Yomitan's "Configure audio playback sources": a list of
// sources in priority order (moved with the ↑/↓ buttons, or Alt+↑/↓), each switched on or off, plus
// how the pop-up shows the other recordings found. The whole section folds under its first row.

import {invoke} from '@tauri-apps/api/core';
import {open} from '@tauri-apps/plugin-dialog';
import {AUDIO_SOURCE_TYPES, CONTEXT_VOICE_TYPES, audioFile, edgeEntry, edgeVoiceName, findAudio, mergeSubsources, normalizeAudioOptions} from '../lib/audio.js';
import {icon} from '../lib/icons.js';
import {store, save} from './store.js';
import {cardRow, collapser, el, select, textInput, toggle} from './ui.js';

const PLACEHOLDERS = {
    'custom': 'https://exemplo.com/audio?term={term}&reading={reading}',
    'custom-json': 'http://127.0.0.1:5050/?term={term}&reading={reading}',
};

const HINTS = {
    'local-db': 'O android.db (com o áudio dentro) ou o entries.db do Local Audio Server for Yomichan, com as pastas <fonte>_files ao lado.',
    'jpod101': 'O áudio do JapanesePod101, só para japonês. A leitura da palavra vem do Jisho.org.',
    'language-pod-101': 'O dicionário do LanguagePod101 no idioma do OCR (japonês, português, inglês, espanhol e outros).',
    'jisho': 'O áudio da página da palavra no Jisho.org, só para japonês.',
    'lingua-libre': 'Gravações de falantes nativos do Lingua Libre (Wikimedia Commons), em vários idiomas.',
    'wiktionary': 'As gravações do Wikcionário (Wikimedia Commons), em vários idiomas, com a região de quem fala.',
    'custom': 'Uma URL que responde com o próprio áudio. Variáveis: {term}, {reading}, {language}.',
    'custom-json': 'Uma URL que responde com uma lista de áudios no formato do Yomitan, como o servidor de áudio local do Anki. Variáveis: {term}, {reading}, {language}.',
    'edge-tts': 'As vozes neurais do "Ler em voz alta" do Microsoft Edge, como no Fushi. Serviço online não oficial: envia a palavra à Microsoft e pode parar se ela mudar o protocolo.',
    'tts': 'A voz do sistema, no idioma do OCR. Sempre encontra algo, por isso fica bem por último.',
};

let edgeVoices = null;

/**
 * A choice of Edge neural voice for the OCR language ("" is the language's default voice). The
 * list comes from the app once per window; a voice saved for another language stays listed.
 */
function edgeVoiceSelect(current, onChange) {
    const locale = String(store.settings.ocrLanguage || 'en-US');
    const base = locale.split('-')[0].toLowerCase();
    const box = el('span', {class: 'edge-voice'}, el('span', {class: 'muted'}, 'Carregando as vozes…'));
    edgeVoices ||= invoke('edge_tts_voices');
    edgeVoices.then((voices) => {
        const own = (voices || []).filter((v) => v.locale.toLowerCase().split('-')[0] === base);
        if (current && !own.some((v) => v.id === current)) { own.unshift({id: current, locale: current.split('-').slice(0, 2).join('-'), gender: ''}); }
        const gender = (g) => (g === 'Female' ? ' (feminina)' : g === 'Male' ? ' (masculina)' : '');
        const list = select([{value: '', label: 'Automática (a padrão do idioma)'},
            ...own.map((v) => ({value: v.id, label: `${edgeVoiceName(v.id)} · ${v.locale}${gender(v.gender)}`}))], current, onChange);
        list.setAttribute('aria-label', 'Voz do Edge');
        box.replaceChildren(list, previewButton(() => list.value));
    }, (e) => {
        edgeVoices = null;
        box.replaceChildren(el('span', {class: 'error'}, `Não deu para carregar as vozes: ${String(e?.message || e)}`));
    });
    return box;
}

/** Plays a short sample of the chosen Edge voice. */
function previewButton(voice) {
    const b = el('button', {class: 'ghost icon', type: 'button', title: 'Ouvir esta voz', 'aria-label': 'Ouvir esta voz', html: icon('speaker', 'sm')});
    b.addEventListener('click', async () => {
        const sample = {ja: 'こんにちは、今日もいい天気ですね。', pt: 'Olá, como vai você?', es: 'Hola, ¿cómo estás?', fr: 'Bonjour, comment ça va ?', de: 'Hallo, wie geht es dir?'};
        const locale = String(store.settings.ocrLanguage || 'en-US');
        const file = await audioFile(edgeEntry(sample[locale.split('-')[0]] || 'Hello, how are you today?', voice(), locale), {invoke}).catch(() => null);
        if (file) { void playFile(file); }
    });
    return b;
}

function playFile(file) {
    const bytes = Uint8Array.from(atob(file.data), (c) => c.charCodeAt(0));
    return new Audio(URL.createObjectURL(new Blob([bytes], {type: file.mime}))).play();
}

/** The audio options, normalized once and then edited in place (rows keep references to them). */
function options() {
    if (!store.settings.audio || store.settings.audio.normalized !== true) {
        store.settings.audio = {...normalizeAudioOptions(store.settings.audio), normalized: true};
    }
    // Saved before the context voice existed.
    store.settings.audio.context ||= {type: 'tts', voice: ''};
    return store.settings.audio;
}

/** Display names of local-audio-yomichan's sources. */
const SUBSOURCE_NAMES = {nhk16: 'NHK16', shinmeikai8: 'Shinmeikai 8 (SMK8)', forvo: 'Forvo', jpod: 'JPod101', jpod_alternate: 'JPod101 Alt', ozk5: 'OZK5', daijisen: 'Daijisen', taas: 'TAAS'};

function move(items, from, to) {
    items.splice(to, 0, items.splice(from, 1)[0]);
    save();
}

/** ↑ and ↓ buttons that move item `i` of `items`, as in Yomitan's list of audio sources. */
function moveButtons(items, i, onMove) {
    const button = (step, name, label) => {
        const b = el('button', {class: 'ghost icon', type: 'button', title: label, 'aria-label': label, html: icon(name, 'sm')});
        const to = i + step;
        if (to < 0 || to >= items.length) { b.disabled = true; }
        b.addEventListener('click', () => { move(items, i, to); onMove(to); });
        return b;
    };
    return el('span', {class: 'audio-move'}, button(-1, 'chevronUp', 'Subir'), button(1, 'chevronDown', 'Descer'));
}

/** Alt+↑/↓ on a focused row moves it, like the buttons. */
function moveKeys(row, items, i, onMove) {
    row.addEventListener('keydown', (ev) => {
        if (!ev.altKey || (ev.key !== 'ArrowUp' && ev.key !== 'ArrowDown') || ev.target !== row) { return; }
        ev.preventDefault?.();
        const to = i + (ev.key === 'ArrowUp' ? -1 : 1);
        if (to < 0 || to >= items.length) { return; }
        move(items, i, to);
        onMove(to);
    });
}

/**
 * The sources inside a local database (nhk16, forvo…), in the user's priority order: move them with
 * the arrows, switch off the ones not wanted. Read from the file each time the row is drawn.
 */
function subsourceList(source) {
    const box = el('div', {class: 'audio-subsources'}, el('div', {class: 'd muted'}, 'Lendo as fontes do arquivo…'));
    const draw = (focus = null) => {
        const list = el('ol', {class: 'audio-sources sub'});
        source.subsources.forEach((sub, i) => {
            const row = el('li', {class: 'audio-sub', tabindex: '0', title: 'Use as setas para mudar a prioridade (ou Alt+↑/↓)'},
                el('span', {class: 'num'}, String(i + 1)),
                el('span', {class: 'name'}, SUBSOURCE_NAMES[sub.name] || sub.name),
                toggle(sub.enabled, (v) => { sub.enabled = v; save(); }),
                moveButtons(source.subsources, i, draw));
            moveKeys(row, source.subsources, i, draw);
            list.append(row);
        });
        box.replaceChildren(el('div', {class: 'd'}, 'Fontes dentro deste arquivo, em ordem de prioridade:'), list);
        if (focus !== null) { list.children[focus]?.focus?.(); }
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
    const type = select(AUDIO_SOURCE_TYPES, source.type, (v) => { source.type = v; save(); redraw(i); });
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
    } else if (source.type === 'edge-tts') {
        field = el('div', {class: 'audio-field'}, edgeVoiceSelect(source.voice, (v) => { source.voice = v; save(); }));
    } else if (source.type === 'custom' || source.type === 'custom-json') {
        const url = textInput(source.url, (v) => { source.url = v.trim(); save(); }, {placeholder: PLACEHOLDERS[source.type]});
        url.setAttribute('data-setting', `audio.sources.${i}.url`);
        field = el('div', {class: 'audio-field'}, url);
    }
    const remove = el('button', {class: 'ghost icon', title: 'Remover esta fonte', 'aria-label': 'Remover esta fonte', html: icon('trash', 'sm')});
    remove.addEventListener('click', () => { all.splice(i, 1); save(); redraw(); });
    const row = el('li', {class: 'audio-source', tabindex: '0', title: 'Use as setas para mudar a prioridade (ou Alt+↑/↓)'},
        el('span', {class: 'num'}, String(i + 1)),
        el('div', {class: 'audio-body'},
            el('div', {class: 'audio-line'}, type, toggle(source.enabled, (v) => { source.enabled = v; save(); redraw(i); }), moveButtons(all, i, redraw), remove),
            el('div', {class: 'd muted'}, HINTS[source.type]),
            field));
    moveKeys(row, all, i, redraw);
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
            let file = null;
            for (const entry of found.filter((f) => f.kind !== 'tts')) {
                file = await audioFile(entry, io).catch(() => null);
                if (file) { break; }
            }
            if (file) {
                void playFile(file);
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
    audio.sources.forEach((source, i) => list.append(sourceRow(redraw, source, i)));
    const add = el('button', {html: `${icon('plus', 'sm')}Adicionar fonte`});
    add.addEventListener('click', () => {
        audio.sources.push({type: 'local-db', enabled: true, path: '', url: '', voice: '', subsources: []});
        save();
        redraw(audio.sources.length - 1);
    });
    const label = (type) => AUDIO_SOURCE_TYPES.find((t) => t.value === type)?.label || type;
    const order = audio.sources.filter((s) => s.enabled).map((s) => label(s.type));
    const head = cardRow({icon: 'speaker', title: 'Áudio da palavra',
        desc: order.length ? `Ordem: ${order.join(' › ')}` : 'Nenhuma fonte ligada: o botão de áudio não toca nada.'});
    root.replaceChildren(
        head,
        cardRow({icon: 'play', title: 'Tocar o áudio automaticamente', desc: 'Toca o primeiro áudio encontrado assim que o popup abre.',
            control: toggle(audio.autoPlay, (v) => { audio.autoPlay = v; save(); })}),
        cardRow({icon: 'speaker', title: 'Os outros áudios no popup', desc: 'O botão de áudio toca o primeiro encontrado. Os demais aparecem num menu do clique direito ou como botões ao lado.', wide: true,
            control: select([
                {value: 'menu', label: 'Menu no clique direito do botão de áudio'},
                {value: 'buttons', label: 'Um botão para cada áudio'},
            ], audio.others, (v) => { audio.others = v; save(); })}),
        cardRow({icon: 'speaker', title: 'Voz para ler o contexto', desc: 'O botão de áudio ao lado do contexto e do tradutor de contexto lê a frase com esta voz. Se a do Edge falhar, a do sistema lê.', wide: true,
            control: el('div', {class: 'audio-field'},
                select(CONTEXT_VOICE_TYPES, audio.context.type, (v) => { audio.context.type = v; save(); redraw(); }),
                audio.context.type === 'edge-tts' ? edgeVoiceSelect(audio.context.voice, (v) => { audio.context.voice = v; save(); }) : null)}),
        el('div', {class: 'card-row stack'},
            el('div', {class: 'head'}, el('span', {class: 'ico', html: icon('list')}),
                el('div', {class: 'txt'}, el('div', {class: 't'}, 'Fontes de áudio'),
                    el('div', {class: 'd'}, 'Em ordem de prioridade: a de cima é tentada primeiro. Use as setas ↑ ↓ para mudar a ordem.')),
                add),
            list),
        testRow(),
    );
    // Folds every row below the first, as the Anki section does.
    const collapse = collapser('audio', () => [...root.children].filter((node) => node !== head));
    head.querySelector('.ctl').append(collapse.button);
    collapse.apply();
    if (focusIndex !== null) { list.children[focusIndex]?.focus?.(); }
}
