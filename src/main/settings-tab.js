// Settings tab: essentials, shortcuts, OCR, per-app profiles, word audio, Anki and application options.

import {getVersion} from '@tauri-apps/api/app';
import {ask} from '@tauri-apps/plugin-dialog';
import {api, escapeHtml} from '../lib/api.js';
import {buttonName, detectLayout} from '../lib/gamepad-buttons.js';
import {icon} from '../lib/icons.js';
import {renderAnkiSection} from './anki-panel.js';
import {renderAudioSection} from './audio-panel.js';
import {transferRow} from './settings-transfer.js';
import {store, save, saveNow} from './store.js';
import {cardRow, collapser, el, heading, segmented, select, textInput, toggle} from './ui.js';

let languages = [];
let status = null;

const MOUSE_NAMES = {left: 'Botão esquerdo', right: 'Botão direito', middle: 'Botão do meio', x1: 'Botão lateral 1 (voltar)', x2: 'Botão lateral 2 (avançar)'};

function acceleratorFrom(ev) {
    const mods = [];
    if (ev.ctrlKey) { mods.push('CommandOrControl'); }
    if (ev.altKey) { mods.push('Alt'); }
    if (ev.shiftKey) { mods.push('Shift'); }
    if (ev.metaKey) { mods.push('Super'); }
    const code = ev.code;
    if (/^(Control|Alt|Shift|Meta)(Left|Right)$/.test(code)) { return null; }
    let key = code;
    if (/^Key[A-Z]$/.test(code)) { key = code.slice(3); } else if (/^Digit\d$/.test(code)) { key = code.slice(5); }
    if (mods.length === 0 && !/^F\d{1,2}$/.test(key)) { return null; }
    return [...mods, key].join('+');
}

function prettyAccelerator(acc) {
    return acc ? acc.replace('CommandOrControl', 'Ctrl').split('+').join(' + ') : '';
}

function keyboardBox() {
    const val = el('span', {class: 'val'}, prettyAccelerator(store.settings.hotkey) || 'Não definido');
    if (!store.settings.hotkey) { val.classList.add('placeholder'); }
    const reset = el('button', {class: 'ghost icon', title: 'Restaurar Alt + D', html: icon('refresh', 'sm')});
    const box = el('div', {class: 'hotkey', tabindex: '0', html: icon('keyboard')}, val, reset);
    box.addEventListener('click', (ev) => {
        if (ev.target.closest('button')) { return; }
        box.classList.add('capturing');
        val.textContent = 'Pressione as teclas…';
        box.focus();
    });
    box.addEventListener('blur', () => {
        box.classList.remove('capturing');
        val.textContent = prettyAccelerator(store.settings.hotkey) || 'Não definido';
    });
    box.addEventListener('keydown', (ev) => {
        if (!box.classList.contains('capturing')) { return; }
        ev.preventDefault();
        if (ev.key === 'Escape') { box.blur(); return; }
        const acc = acceleratorFrom(ev);
        if (acc) {
            store.settings.hotkey = acc;
            save();
            box.blur();
        }
    });
    reset.addEventListener('click', () => {
        store.settings.hotkey = 'Alt+D';
        val.textContent = 'Alt + D';
        val.classList.remove('placeholder');
        save();
    });
    return box;
}

function mouseLabel(m) {
    if (!m || m.button === 'none') { return ''; }
    const parts = [];
    if (m.ctrl) { parts.push('Ctrl'); }
    if (m.alt) { parts.push('Alt'); }
    if (m.shift) { parts.push('Shift'); }
    parts.push(MOUSE_NAMES[m.button] || m.button);
    return parts.join(' + ') + (m.longPress ? ' (segurar)' : '');
}

function mouseBox(supported) {
    const m = store.settings.mouseHotkey;
    const val = el('span', {class: 'val'}, mouseLabel(m) || 'Não definido');
    if (!mouseLabel(m)) { val.classList.add('placeholder'); }
    const clear = el('button', {class: 'ghost icon', title: 'Remover', html: icon('close', 'sm')});
    const box = el('div', {class: 'hotkey', tabindex: '0', html: icon('mouse')}, val, clear);
    if (!supported) {
        box.style.opacity = '0.5';
        val.textContent = 'Disponível no Windows';
        return box;
    }
    let capturing = false;
    box.addEventListener('click', (ev) => {
        if (ev.target.closest('button') || capturing) { return; }
        capturing = true;
        box.classList.add('capturing');
        val.textContent = 'Clique aqui com o botão desejado (pode segurar Alt, Ctrl ou Shift)…';
    });
    box.addEventListener('contextmenu', (ev) => { if (capturing) { ev.preventDefault(); } });
    box.addEventListener('mousedown', (ev) => {
        if (!capturing || ev.target.closest('button')) { return; }
        ev.preventDefault();
        const button = ['left', 'middle', 'right', 'x1', 'x2'][ev.button] || 'none';
        if ((button === 'left' || button === 'right') && !(ev.altKey || ev.ctrlKey || ev.shiftKey)) {
            val.textContent = 'Use Alt, Ctrl ou Shift junto com o botão esquerdo ou direito.';
            return;
        }
        store.settings.mouseHotkey = {button, alt: ev.altKey, ctrl: ev.ctrlKey, shift: ev.shiftKey, longPress: m.longPress};
        capturing = false;
        box.classList.remove('capturing');
        val.textContent = mouseLabel(store.settings.mouseHotkey);
        val.classList.remove('placeholder');
        save();
    });
    clear.addEventListener('click', () => {
        store.settings.mouseHotkey = {button: 'none', alt: false, ctrl: false, shift: false, longPress: false};
        val.textContent = 'Não definido';
        val.classList.add('placeholder');
        save();
    });
    return box;
}

// Controller buttons by their Xbox names, as gamepad.rs reads them.
const GAMEPAD_ACTIONS = [
    ['confirm', 'Confirmar', 'Clica no botão marcado no pop-up, ou consulta a palavra marcada na tela.'],
    ['close', 'Fechar', 'Fecha o pop-up e deixa as palavras na tela; de novo, fecha a consulta.'],
    ['save', 'Salvar palavra', 'Salva no próprio Lookitan (Palavras).'],
    ['anki', 'Enviar ao Anki', 'O botão do Anki no pop-up.'],
    ['pronounce', 'Pronunciar', 'Toca o áudio da palavra.'],
    ['explain', 'Explicar', 'Gera a explicação da IA.'],
    ['meme', 'Modo meme', 'Liga e desliga o modo meme das imagens.'],
];
const GAMEPAD_DEFAULTS = {hotkey: [], navigation: true, confirm: ['A'], close: ['B'], save: ['X'], pronounce: ['Y'], explain: ['RB'], anki: ['LB'], meme: []};
// The connected pad's layout, for the buttons' names (a PlayStation pad shows ✕ ○ □ △).
let gamepadLayout = 'xbox';
// One "gamepad-capture" listener for the window; it calls the box recording now.
let gamepadCaptureHandler = null;
let gamepadCaptureListening = false;

/** The settings' controller options, missing ones filled in place (the rows share this object). */
function gamepadSettings() {
    const s = store.settings;
    s.gamepad ||= {};
    for (const [key, value] of Object.entries(GAMEPAD_DEFAULTS)) {
        const current = s.gamepad[key];
        if (current === undefined) { s.gamepad[key] = Array.isArray(value) ? [...value] : value; }
        // A single button, as an earlier build saved the actions.
        else if (typeof current === 'string') { s.gamepad[key] = current ? [current] : []; }
    }
    return s.gamepad;
}

function gamepadLabel(buttons) {
    return (buttons || []).map((b) => buttonName(b, gamepadLayout)).join(' + ');
}

const sameButtons = (a, b) => a.length === b.length && a.every((x) => b.includes(x));

/**
 * A box that records buttons from the controller: click, hold one button or several together
 * and let go. `get()` and `set(buttons)` read and store them. × leaves it empty; with `defaults`,
 * ↺ puts them back.
 */
function gamepadComboBox(get, set, defaults = []) {
    const val = el('span', {class: 'val'});
    const show = () => {
        const text = gamepadLabel(get());
        val.textContent = text || 'Não definido';
        val.classList.toggle('placeholder', !text);
    };
    show();
    const clear = el('button', {class: 'ghost icon', title: 'Deixar vazio', 'aria-label': 'Deixar vazio', html: icon('close', 'sm')});
    const reset = defaults.length
        ? el('button', {class: 'ghost icon', title: `Voltar ao padrão (${gamepadLabel(defaults)})`, 'aria-label': 'Voltar ao padrão', html: icon('refresh', 'sm')})
        : null;
    const box = el('div', {class: 'hotkey', tabindex: '0', html: icon('gamepad')}, val, reset, clear);
    let capturing = false;
    const stop = () => {
        if (!capturing) { return; }
        capturing = false;
        box.classList.remove('capturing');
        void api.gamepadCapture(false);
        show();
    };
    reset?.addEventListener('click', () => {
        set([...defaults]);
        show();
    });
    const handler = ({buttons, done}) => {
        if (!capturing) { return; }
        if (!done) { val.textContent = `${gamepadLabel(buttons)}…`; return; }
        capturing = false;
        box.classList.remove('capturing');
        if (buttons.length) { set(buttons); }
        show();
    };
    box.addEventListener('click', (ev) => {
        if (ev.target.closest('button') || capturing) { return; }
        capturing = true;
        gamepadCaptureHandler = handler;
        box.classList.add('capturing');
        val.textContent = 'Segure um botão ou vários no controle e solte…';
        void api.gamepadCapture(true);
        box.focus();
    });
    box.addEventListener('blur', stop);
    box.addEventListener('keydown', (ev) => { if (ev.key === 'Escape') { stop(); } });
    if (!gamepadCaptureListening) {
        gamepadCaptureListening = true;
        void api.onGamepadCapture((payload) => gamepadCaptureHandler?.(payload));
    }
    clear.addEventListener('click', () => {
        set([]);
        show();
    });
    return box;
}

/** "Conectado: …", checked each time the settings are drawn. */
function gamepadStatusLine() {
    const line = el('span', {}, 'Combinação de botões para consultar a palavra sob o cursor.');
    void api.gamepadStatus().then((name) => {
        line.textContent = name
            ? `Combinação de botões para consultar a palavra sob o cursor. Conectado: ${name}.`
            : 'Combinação de botões para consultar a palavra sob o cursor. Nenhum controle conectado agora.';
        // A PlayStation pad renames the buttons; the settings redraw once with its names.
        const layout = name ? detectLayout(name) : gamepadLayout;
        if (layout !== gamepadLayout) {
            gamepadLayout = layout;
            rerender();
        }
    }, () => {});
    return line;
}

/**
 * The Gamepad row with the look-up combination, folding like Lookupper's: under it the navigation
 * switch with its legend and, with it on, each action's button or buttons. A combination another
 * action has swaps with it.
 */
function gamepadSection() {
    const g = gamepadSettings();
    const legend = ['Selecionar: direcional ou analógico esquerdo', 'Rolar: analógico direito',
        ...GAMEPAD_ACTIONS.filter(([key]) => g[key].length).map(([key, label]) => `${label}: ${gamepadLabel(g[key])}`)].join(' · ');
    const body = [cardRow({icon: 'gamepad', title: 'Navegação pelo gamepad',
        desc: `${legend}. Com o pop-up aberto, anda pelos botões dele; fechado, pelas palavras na tela. Desligue se o seu controle já move o cursor do mouse (Steam Input).`,
        control: toggle(g.navigation, (v) => { g.navigation = v; save(); rerender(); })})];
    if (g.navigation) {
        for (const [key, label, desc] of GAMEPAD_ACTIONS) {
            const box = gamepadComboBox(() => g[key], (buttons) => {
                const other = buttons.length ? GAMEPAD_ACTIONS.find(([k]) => k !== key && sameButtons(g[k], buttons)) : null;
                if (other) { g[other[0]] = g[key]; }
                g[key] = buttons;
                save();
                rerender();
            }, GAMEPAD_DEFAULTS[key]);
            box.setAttribute('data-setting', `gamepad.${key}`);
            body.push(cardRow({icon: 'gamepad', title: label, desc: `${desc} Um botão ou vários juntos.`, wide: true, control: box}));
        }
    }
    const collapse = collapser('gamepad', () => body);
    const hotkey = gamepadComboBox(() => g.hotkey, (buttons) => { g.hotkey = buttons; save(); });
    hotkey.setAttribute('data-setting', 'gamepad.hotkey');
    const head = cardRow({icon: 'gamepad', title: 'Gamepad', desc: gamepadStatusLine(), wide: true,
        control: el('div', {class: 'gamepad-ctl'}, hotkey, collapse.button)});
    collapse.apply();
    return [head, ...body];
}

/** "Estado do motor": what each selector reads with, one line each (`engines_status` in lib.rs). */
function engineStatusText() {
    if (!status) { return 'Carregando…'; }
    if (status.runtimeError) { return `ONNX Runtime: ${status.runtimeError}`; }
    const lines = (status.engines || []).map(([selector, models]) => `${selector}: ${models}`);
    if (status.error) { lines.push(`O PP-OCRv6 não carregou: ${status.error}`); }
    if (status.segmenter) { lines.push(status.segmenter); }
    return lines.join('\n') || 'Carregando o motor…';
}

/**
 * Japanese, Chinese and Korean: the word analyzer the recognition language downloads, with a
 * button to delete it or, once deleted, to download it, as Lookupper's analyzer models have.
 */
function segmenterRow() {
    const row = el('div', {id: 'segmenter-row'});
    fillSegmenterRow(row);
    return row;
}

/** The engines whose files the app downloads when they are chosen (`Downloaded` in lib.rs). */
const DOWNLOADED_ENGINES = {
    meiki: {title: 'Modelos do MeikiOCR (31 MB)',
        desc: 'De rtr46/meikiocr no Hugging Face, licença LGPL-3.0; baixados uma vez e conferidos.',
        removeAsk: 'Apagar os modelos do MeikiOCR? Enquanto não forem baixados de novo, ele não lê.',
        removeTitle: 'Apagar o MeikiOCR'},
    screenai: {title: 'Arquivos do Chrome ScreenAI (cerca de 76 MB)',
        desc: 'A biblioteca de OCR do Chrome, do Google, baixada do repositório de pacotes do Chromium (CIPD), como o owocr faz: a versão mais recente quando você baixa, conferida pelo SHA-256 que o CIPD dá. Para atualizar, apague e baixe de novo. Não é software livre e não tem licença própria. Roda na CPU, num processo à parte que fecha depois de 5 minutos sem uso.',
        removeAsk: 'Apagar os arquivos do Chrome ScreenAI? Enquanto não forem baixados de novo, ele não lê.',
        removeTitle: 'Apagar o Chrome ScreenAI'},
    oneocr: {title: 'Arquivos do OneOCR (do Recorte do Windows)',
        desc: 'O OCR do Recorte do Windows 11, da Microsoft: copiados do Recorte instalado neste computador, como o owocr faz. Sem ele (no Windows 10, por exemplo), cerca de 80 MB são baixados do pacote do Recorte no servidor da Microsoft, com o link dado pelo site store.rg-adguard.net (de terceiros), como o LunaTranslator faz, e conferidos pela assinatura da Microsoft. Não é software livre. Roda na CPU, num processo à parte que fecha depois de 5 minutos sem uso.',
        removeAsk: 'Apagar os arquivos do OneOCR? Enquanto não forem instalados de novo, ele não lê.',
        removeTitle: 'Apagar o OneOCR', action: 'Instalar', done: 'instalados'},
    indra: {title: 'Modelos do IndraOCR (21 MB)',
        desc: 'A detecção (LineNet) e os reconhecedores do IndraOCR, da Datalab, no Hugging Face: uma versão fixa, conferida. Os pesos têm a licença AI Pubs Open RAIL-M modificada, que você aceita antes de baixar.',
        removeAsk: 'Apagar os modelos do IndraOCR? Enquanto não forem baixados de novo, nem ele nem a detecção LineNet leem.',
        removeTitle: 'Apagar o IndraOCR',
        license: 'Os pesos do IndraOCR (Datalab) usam a licença AI Pubs Open RAIL-M modificada (huggingface.co/datalab-to/indra-ocr, arquivo LICENSE, que vem junto com os modelos). Ela permite uso pessoal, em pesquisa e em empresas com menos de US$ 20 milhões de receita ou de investimento; proíbe o uso por quem oferece um produto que concorra com os da Datalab e usos ilegais; e diz que os resultados do modelo também ficam sob ela. Ao baixar, você aceita esses termos. Baixar?'},
};

/** The files of the chosen engines that are downloaded: their state, with a button to download or
 * delete them. */
function modelsRows() {
    const row = el('div', {id: 'models-rows'});
    fillModelsRows(row);
    return row;
}

function fillModelsRows(rows) {
    rows.replaceChildren();
    const s = store.settings;
    // Each selector's engine, and the models it reads with: LineNet is IndraOCR's, and the custom
    // engine's recognizer may be another engine's.
    const chosen = new Set();
    for (const pointer of [true, false]) {
        const engine = selectorEngine(pointer);
        chosen.add(engine);
        if (modelRows(engine, pointer).length === 0) { continue; }
        const c = currentChoice(engine);
        if (c.det === 'indra:linenet') { chosen.add('indra'); }
        if (engine === 'custom') { chosen.add(c.rec); }
    }
    for (const engine of Object.keys(DOWNLOADED_ENGINES)) {
        if (chosen.has(engine)) { rows.append(modelsRow(engine)); }
    }
}

function modelsRow(engine) {
    const row = el('div');
    const info = DOWNLOADED_ENGINES[engine];
    const model = status?.models?.[engine];
    const downloading = /^estão sendo baixados/.test(model?.download || '');
    const device = engine === 'meiki' && status?.meikiOnGpu != null ? ` · reconhecimento na ${status.meikiOnGpu ? 'GPU' : 'CPU'}` : '';
    const state = model?.installed ? `${info.done || 'baixados'}${device}` : (model?.download || `não ${info.done || 'baixados'}`);
    const error = el('div', {class: 'd', style: 'color: var(--danger)', hidden: true});
    const run = async (button, action) => {
        error.hidden = true;
        button.disabled = true;
        try { await action(); } catch (e) {
            error.textContent = String(e?.message || e);
            error.hidden = false;
        } finally { button.disabled = false; }
    };
    let button;
    if (model?.installed) {
        button = el('button', {html: `${icon('trash', 'sm')}Apagar`});
        button.addEventListener('click', () => void run(button, async () => {
            if (!await ask(info.removeAsk, {title: info.removeTitle, kind: 'warning', okLabel: 'Apagar', cancelLabel: 'Cancelar'})) { return; }
            await api.removeOcrModels(engine);
        }));
    } else {
        button = el('button', {html: `${icon('download', 'sm')}${info.action || 'Baixar'}`, disabled: downloading});
        button.addEventListener('click', () => void run(button, async () => {
            if (info.license && !await ask(info.license, {title: 'Licença do IndraOCR', kind: 'warning', okLabel: 'Aceitar e baixar', cancelLabel: 'Cancelar'})) { return; }
            await api.installOcrModels(engine, !!info.license);
        }));
    }
    row.append(el('div', {class: 'card-row'}, el('span', {class: 'ico', html: icon('download')}),
        el('div', {class: 'txt'}, el('div', {class: 't'}, info.title), el('div', {class: 'd'}, `${info.desc} Estado: ${state}`), error),
        el('div', {class: 'ctl'}, button)));
    return row;
}

const OWN_MODELS = {rec_arabic: 'Árabe', rec_cyrillic: 'Cirílico', rec_el: 'Grego', rec_korean: 'Coreano'};
const EXPERIMENTAL_MODELS = {rec_ru_tiny: 'Tiny RU (experimental)'};

/**
 * PP-OCRv6's recognizer for the OCR language: one that reads with rec_tiny can switch to rec_small,
 * on its own; Russian can switch to its experimental model; one on rec_small or on a model of its
 * own shows only that model (rec_tiny and rec_small have no Cyrillic, Arabic or Hangul). Null until
 * the languages are known.
 */
function paddleRecognizer() {
    const s = store.settings;
    const tag = String(s.ocrLanguage || '').toLowerCase();
    const language = languages.find((l) => l.tag.toLowerCase() === tag);
    if (!language?.rec) { return null; }
    const small = () => (Array.isArray(s.smallRecognizer) ? s.smallRecognizer : []);
    const others = () => small().filter((t) => String(t).toLowerCase() !== tag);
    // PP-OCR's models for the language: what is offered, which is chosen and how a choice is kept.
    let options, value, choose, desc;
    if (language.rec === 'rec_tiny') {
        desc = 'Vale só para este idioma. O small acerta mais, mas na CPU leva cerca de 4× mais tempo por linha.';
        options = [{value: 'tiny', label: 'Tiny (padrão)'}, {value: 'small', label: 'Small'}];
        value = others().length < small().length ? 'small' : 'tiny';
        choose = (v) => { s.smallRecognizer = v === 'small' ? [...others(), language.tag] : others(); };
    } else if (language.experimental) {
        // Russian: its own model, or PP-OCRv6 Tiny RU on trial (Settings `experimentalRecognizer`).
        const chosen = () => (Array.isArray(s.experimentalRecognizer) ? s.experimentalRecognizer : []);
        const rest = () => chosen().filter((t) => String(t).toLowerCase() !== tag);
        desc = 'Este idioma tem um modelo próprio. O Tiny RU é experimental: foi treinado em documentos e manuscritos e, em texto de tela, errou mais nos nossos testes.';
        options = [
            {value: language.rec, label: `${OWN_MODELS[language.rec] || language.rec} (padrão)`},
            {value: language.experimental, label: EXPERIMENTAL_MODELS[language.experimental] || language.experimental},
        ];
        value = rest().length < chosen().length ? language.experimental : language.rec;
        choose = (v) => { s.experimentalRecognizer = v === language.experimental ? [...rest(), language.tag] : rest(); };
    } else {
        const only = language.rec === 'rec_small';
        desc = only ? 'Este idioma só tem o modelo small do PP-OCR.' : 'Este idioma tem um modelo próprio no PP-OCR.';
        options = [{value: language.rec, label: only ? 'Small (padrão)' : `${OWN_MODELS[language.rec] || language.rec} (padrão)`}];
        value = language.rec;
        choose = () => {};
    }
    const control = select(options, value, (v) => { choose(v); save(); });
    control.disabled = options.length < 2;
    return {control, desc, choices: options.length};
}

/** The detectors (`Detector::id` in settings.rs), by name. */
const DETECTORS = {'paddle:det_tiny': 'PP-OCRv6 Tiny', 'paddle:det_small': 'PP-OCRv6 Small', 'indra:linenet': 'IndraOCR LineNet'};
/** Engines that find and read the text themselves. */
const PAGE_ENGINES = {oneocr: 'OneOCR', screenai: 'Chrome ScreenAI', lens: 'Google Lens', bing: 'Bing'};
const LOCAL_NAMES = {paddle: 'PP-OCRv6', meiki: 'MeikiOCR', indra: 'IndraOCR', custom: 'Personalizado'};

/** The detectors an engine can be set to (`OcrEngineKind::offers`): each engine its own (PP-OCRv6's
 * for MeikiOCR until its own comes), the custom engine and a page engine's block any. */
function offeredDetectors(engine) {
    const all = Object.keys(DETECTORS);
    if (engine === 'indra') { return ['indra:linenet']; }
    if (engine === 'paddle' || engine === 'meiki') { return all.filter((d) => d !== 'indra:linenet'); }
    return all;
}

/** An engine's models as `Settings::models` reads them: those chosen for it (Settings
 * `ocrModels`), or its own, where "Usar a placa de vídeo" (`useGpu`) said. */
function currentChoice(engine) {
    const s = store.settings;
    const gpu = s.useGpu !== false;
    const own = engine === 'custom'
        ? {det: 'paddle:det_small', detGpu: true, rec: 'paddle', recGpu: true}
        : {det: engine === 'indra' ? 'indra:linenet' : `paddle:${gpu ? 'det_small' : 'det_tiny'}`, detGpu: gpu,
            rec: engine === 'meiki' || engine === 'indra' ? engine : 'paddle', recGpu: gpu};
    const saved = s.ocrModels?.[engine];
    if (!saved) { return own; }
    return {
        det: offeredDetectors(engine).includes(saved.det) ? saved.det : own.det,
        detGpu: saved.detGpu !== false,
        rec: engine === 'custom' ? (saved.rec || own.rec) : own.rec,
        recGpu: saved.recGpu !== false,
    };
}

function setChoice(engine, field, value) {
    const s = store.settings;
    s.ocrModels = {...(s.ocrModels || {}), [engine]: {...currentChoice(engine), [field]: value}};
    save();
}

/**
 * The rows under a selector: its engine's detector and recognizer (Settings `ocrModels`), each
 * with where it runs. A page engine reads by itself: under the pointer's selector, in the automatic
 * capture, it has only the detector that finds the block under the pointer, and nothing elsewhere.
 * A row's `data-sub` names its engine.
 */
function modelRows(engine, pointer) {
    const s = store.settings;
    const c = currentChoice(engine);
    const name = LOCAL_NAMES[engine] || PAGE_ENGINES[engine] || engine;
    const device = (field) => segmented([{value: 'cpu', label: 'CPU'}, {value: 'gpu', label: 'GPU'}], c[field] ? 'gpu' : 'cpu',
        (v) => setChoice(engine, field, v === 'gpu'));
    const controls = (control, field) => el('div', {class: 'model-controls'}, control, device(field));
    const row = (title, desc, control) => {
        const r = cardRow({icon: 'scan', title, desc, wide: true, control});
        r.setAttribute('data-sub', engine);
        return r;
    };
    const offered = offeredDetectors(engine);
    const det = select(offered.map((d) => ({value: d, label: DETECTORS[d]})), c.det, (v) => { setChoice(engine, 'det', v); refreshOcrRows(); });
    det.disabled = offered.length < 2;
    if (PAGE_ENGINES[engine]) {
        if (!pointer || s.captureAuto !== true) { return []; }
        return [row('Achar o bloco sob o cursor', `Na captura automática, a detecção acha o bloco de texto sob o cursor e o ${name} lê só esse bloco. A LineNet precisa dos modelos do IndraOCR.`,
            controls(det, 'detGpu'))];
    }
    const detDesc = {
        paddle: 'O small acha mais, mas na CPU é bem mais lento; o tiny ganha pouco na GPU.',
        meiki: 'Por enquanto, a do PP-OCRv6; a do próprio MeikiOCR vem numa próxima versão.',
        indra: 'A do próprio IndraOCR (LineNet), leve. Acha linhas na horizontal.',
        custom: 'Qualquer uma das detecções locais. A LineNet precisa dos modelos do IndraOCR.',
    }[engine];
    const rows = [row('Detecção', `Acha as linhas de texto. ${detDesc} Se a GPU falhar, segue na CPU.`, controls(det, 'detGpu'))];
    const paddle = paddleRecognizer();
    if (engine === 'custom') {
        const japanese = String(s.ocrLanguage || '').toLowerCase() === 'ja-jp';
        const rec = select([
            {value: 'paddle', label: 'PP-OCRv6 (o modelo do idioma)'},
            ...(japanese || c.rec === 'meiki' ? [{value: 'meiki', label: 'MeikiOCR (japonês)'}] : []),
            {value: 'indra', label: 'IndraOCR (experimental)'},
        ], c.rec, (v) => { setChoice('custom', 'rec', v); refreshOcrRows(); });
        const recDesc = c.rec === 'meiki' && !japanese
            ? 'O MeikiOCR lê só japonês: neste idioma, ele não lê.'
            : 'Lê as linhas que a detecção acha. Sem os modelos dele, não lê.';
        rows.push(row('Reconhecimento', recDesc, controls(rec, 'recGpu')));
        if (c.rec === 'paddle' && paddle && paddle.choices > 1) {
            rows.push(row('Modelo do PP-OCRv6', paddle.desc, paddle.control));
        }
    } else if (engine === 'paddle') {
        // Shown once the languages are known.
        if (paddle) { rows.push(row('Reconhecimento', paddle.desc, controls(paddle.control, 'recGpu'))); }
    } else {
        const rec = select([{value: engine, label: name}], engine, () => {});
        rec.disabled = true;
        rows.push(row('Reconhecimento', `O do ${name}. Se a GPU falhar, segue na CPU.`, controls(rec, 'recGpu')));
    }
    return rows;
}

/** The engine a selector reads with. */
function selectorEngine(pointer) {
    const s = store.settings;
    const engine = s.ocrEngine || 'paddle';
    return pointer ? engine : (s.screenOcrEngine ?? engine);
}

function engineDesc(engine) {
    const japanese = store.settings.ocrLanguage === 'ja-JP';
    switch (engine) {
    case 'lens': return 'Envia a captura ao Google (API não oficial). Se falhar, a consulta mostra o erro.';
    case 'bing': return 'Envia a captura à Microsoft, pelo site do Bing (não é uma API oficial e pode mudar sem aviso), como o owocr faz. Se falhar, a consulta mostra o erro.';
    case 'meiki': return `A detecção do PP-OCRv6 acha as linhas e o MeikiOCR, treinado em texto de jogos japoneses, as lê. Roda no seu computador. ${japanese ? 'Sem os modelos baixados, ele não lê.' : 'Ele lê só japonês: neste idioma, não lê.'}`;
    case 'indra': return 'A detecção e o reconhecimento do próprio IndraOCR, leves e feitos para a CPU (experimental). Roda no seu computador. Sem os modelos baixados, ele não lê.';
    case 'oneocr': return 'O OCR do Recorte do Windows 11, da Microsoft, com os arquivos copiados do Recorte deste computador ou baixados do servidor da Microsoft. Roda no seu computador, nada sai dele; sem os arquivos, ele não lê.';
    case 'custom': return 'A detecção e o reconhecimento que você escolher abaixo, de qualquer motor local, para testar combinações. Roda no seu computador.';
    case 'screenai': return 'A biblioteca de OCR do Chrome, que reconhece qualquer idioma e acha o de cada linha sozinha. Roda no seu computador, nada sai dele; sem os arquivos baixados, ele não lê.';
    default: return 'Roda no seu computador; nada sai dele.';
    }
}

/** A selector's row, its engine's models under it (folded with the chevron), as `modelRows`. The
 * whole screen's shows none when it reads with the pointer's engine: the same models. */
function engineRows(pointer) {
    const s = store.settings;
    const engine = selectorEngine(pointer);
    const shared = !pointer && engine === selectorEngine(true);
    const models = shared ? [] : modelRows(engine, pointer);
    const japanese = s.ocrLanguage === 'ja-JP';
    const windows = (status?.session || 'windows') === 'windows';
    // MeikiOCR reads Japanese only: it is offered with Japanese, and kept in the list while chosen.
    const choose = select([
        {value: 'paddle', label: 'PP-OCRv6 (local)'},
        ...(japanese || engine === 'meiki' ? [{value: 'meiki', label: 'MeikiOCR (local, japonês)'}] : []),
        {value: 'screenai', label: 'Chrome ScreenAI (local, qualquer idioma)'},
        {value: 'indra', label: 'IndraOCR (local, experimental)'},
        {value: 'custom', label: 'Personalizado (DET + REC)'},
        // OneOCR is the Windows Snipping Tool's.
        ...(windows || engine === 'oneocr' ? [{value: 'oneocr', label: 'OneOCR (local, do Windows)'}] : []),
        {value: 'lens', label: 'Google Lens (nuvem)'},
        {value: 'bing', label: 'Bing (nuvem)'},
    ], engine, (v) => {
        if (pointer) { s.ocrEngine = v; } else { s.screenOcrEngine = v; }
        save();
        rerender();
    });
    let control = choose;
    if (models.length) {
        const collapse = collapser(`ocrModels.${pointer ? 'pointer' : 'screen'}`, () => models);
        control = el('div', {class: 'model-controls'}, choose, collapse.button);
        collapse.apply();
    }
    const title = pointer ? 'OCR em volta do cursor' : 'OCR da tela inteira';
    const lead = pointer ? 'Lê a palavra sob o cursor.' : 'Usado quando o atalho é apertado fora de uma palavra; depois é só clicar na palavra.';
    const note = shared && LOCAL_NAMES[engine] ? ' Com os mesmos modelos do OCR em volta do cursor.' : '';
    return [cardRow({icon: 'scan', title, desc: `${lead} ${engineDesc(engine)}${note}`, wide: true, control}), ...models];
}

/** Both selectors with their engines' models, in one box a change of language or capture refills. */
function ocrEnginesBox() {
    const box = el('div', {id: 'ocr-engines', class: 'group'});
    fillOcrEngines(box);
    return box;
}

function fillOcrEngines(box) {
    box.replaceChildren(...engineRows(true), ...engineRows(false));
}

/** Draws the selectors' models and the downloads they need again. */
function refreshOcrRows() {
    const box = document.getElementById('ocr-engines');
    if (box) { fillOcrEngines(box); }
    const models = document.getElementById('models-rows');
    if (models) { fillModelsRows(models); }
}

function fillSegmenterRow(row) {
    row.replaceChildren();
    if (!status?.segmenter) { return; }
    const [, label = '', state = status.segmenter] = /^separação de palavras \(([^)]*)\)\s*:?\s*(.*)$/.exec(status.segmenter) || [];
    const busy = /baixando|carregando|aguardando/.test(state);
    const failed = /indisponível/.test(status.segmenter);
    const error = el('div', {class: 'd', style: 'color: var(--danger)', hidden: true});
    const run = async (button, action) => {
        error.hidden = true;
        button.disabled = true;
        try { await action(); } catch (e) {
            error.textContent = String(e?.message || e);
            error.hidden = false;
        } finally { button.disabled = false; }
    };
    const buttons = [];
    if (status.segmenterInstalled) {
        const remove = el('button', {html: `${icon('trash', 'sm')}Apagar`, disabled: busy});
        remove.addEventListener('click', () => void run(remove, async () => {
            if (!await ask(`Apagar a separação de palavras (${label})? Ela é baixada de novo na próxima consulta neste idioma.`,
                {title: 'Apagar a separação de palavras', kind: 'warning', okLabel: 'Apagar', cancelLabel: 'Cancelar'})) { return; }
            await api.removeSegmenter();
        }));
        buttons.push(remove);
    } else {
        const download = el('button', {html: `${icon('download', 'sm')}Baixar`, disabled: busy && !failed});
        download.addEventListener('click', () => void run(download, () => api.installSegmenter()));
        buttons.push(download);
    }
    row.append(el('div', {class: 'card-row'}, el('span', {class: 'ico', html: icon('book')}),
        el('div', {class: 'txt'}, el('div', {class: 't'}, `Separação de palavras (${label})`),
            el('div', {class: 'd'}, `Divide o texto em palavras e acha a forma de dicionário. Estado: ${state}`), error),
        el('div', {class: 'ctl'}, ...buttons)));
}

function captureRegionRows() {
    const s = store.settings;
    const currentPreset = () => (s.captureAuto === true ? 'auto' : 'custom');
    let presets;
    function dimension(key, label, minimum, fallback, id) {
        const input = el('input', {id, type: 'number', min: minimum, max: 4096, step: 1,
            'aria-label': `${label} da captura em pixels`});
        input.value = s[key] ?? fallback;
        input.addEventListener('change', () => {
            const value = Number(input.value);
            if (!Number.isInteger(value) || value < minimum || value > 4096) {
                input.value = s[key] ?? fallback;
                return;
            }
            input.value = value;
            if (value !== s[key]) { s[key] = value; save(); }
        });
        return {input, label: el('label', {class: 'capture-dimension'}, el('span', {}, `${label} (px)`), input)};
    }
    const width = dimension('captureWidth', 'Largura', 200, 900, 'cap-w');
    const height = dimension('captureHeight', 'Altura', 60, 260, 'cap-h');
    // The size is edited only under "Personalizada".
    const size = cardRow({icon: 'focus', title: 'Tamanho da captura', desc: 'Região em volta do cursor, em pixels.', wide: true,
        control: el('div', {class: 'capture-size'}, width.label, height.label)});
    size.hidden = currentPreset() !== 'custom';
    // "Automático" reads the whole monitor but recognizes only the block of text under the
    // pointer, like Lookupper's pointer mode; "Personalizada" reads a fixed region of the size below.
    const choices = [{value: 'auto', label: 'Automático (o bloco de texto sob o cursor)'}, {value: 'custom', label: 'Personalizada'}];
    presets = select(choices, currentPreset(), (value) => {
        size.hidden = value !== 'custom';
        const auto = value === 'auto';
        const modeChanged = (s.captureAuto === true) !== auto;
        s.captureAuto = auto;
        if (modeChanged) { save(); refreshOcrRows(); }
    });
    presets.id = 'capture-preset';
    presets.setAttribute('aria-label', 'Região de captura do OCR');
    return [
        cardRow({icon: 'focus', title: 'Região de captura', desc: 'Automático lê só o bloco de texto sob o cursor, como o Lookupper. Personalizada lê uma região fixa em volta dele, do tamanho que você escolher.', wide: true, control: presets}),
        size,
    ];
}

function profilesCard() {
    const profiles = store.settings.profiles;
    const list = el('div', {class: 'profiles'});
    const header = el('div', {class: 'card-row', style: 'border-radius: 8px 8px 0 0'},
        el('span', {class: 'ico', html: icon('list')}),
        el('div', {class: 'txt'}, el('div', {class: 't'}, `Perfis de jogos e programas (${profiles.length})`), el('div', {class: 'd'}, 'Configure cada jogo ou programa separadamente. Eles aparecem aqui quando você usa o Lookitan neles. Por padrão, a tela não congela.')),
        el('div', {class: 'ctl'}));
    const addInput = textInput('', () => {}, {placeholder: 'programa.exe ou classe da janela'});
    addInput.style.minWidth = '220px';
    const add = el('button', {html: `${icon('plus', 'sm')}Adicionar`});
    add.addEventListener('click', () => {
        const exe = addInput.value.trim();
        if (!exe || profiles.some((p) => p.executable.toLowerCase() === exe.toLowerCase())) { return; }
        profiles.push({executable: exe, displayName: exe, focusOverlay: false, freeze: 'none'});
        save();
        rerender();
    });
    const collapse = collapser('profiles', () => [list],
        (collapsed) => { header.style.borderRadius = collapsed ? '8px' : '8px 8px 0 0'; });
    header.querySelector('.ctl').append(addInput, add, collapse.button);

    for (const p of profiles) {
        const focus = el('input', {type: 'checkbox'});
        focus.checked = p.focusOverlay;
        focus.addEventListener('change', () => { p.focusOverlay = focus.checked; save(); });
        const freeze = select([
            {value: 'none', label: 'Sem congelar'},
            {value: 'frame', label: 'Congelar'},
            {value: 'process', label: 'Pausar o programa'},
        ], p.freeze === 'default' ? 'none' : p.freeze, (v) => { p.freeze = v; save(); });
        freeze.title = 'Sem congelar: a tela continua ao vivo e só as palavras ficam destacadas. Congelar: uma imagem parada da tela ' +
            'fica por cima durante a consulta (na leitura da tela inteira, levemente desfocada). Pausar o programa: suspende o programa durante a consulta.';
        const del = el('button', {class: 'ghost icon', title: 'Remover', html: icon('trash', 'sm')});
        del.addEventListener('click', () => {
            store.settings.profiles = profiles.filter((x) => x !== p);
            save();
            rerender();
        });
        list.append(el('div', {class: 'profile'},
            el('span', {class: 'badge'}, (p.displayName || p.executable || '?').trim().charAt(0).toUpperCase()),
            el('div', {class: 'names'}, el('div', {}, p.displayName || p.executable), el('div', {class: 'x'}, p.executable)),
            el('label', {class: 'check', title: 'Dá o foco do teclado ao overlay (para jogos que prendem o teclado)'}, focus, 'Foco'),
            freeze, del));
    }
    collapse.apply();
    return el('div', {class: 'panel', style: 'overflow:hidden'}, header, list);
}

let rootEl = null;

function rerender() {
    if (rootEl) { render(rootEl); }
}

export async function initSettingsTab(root) {
    rootEl = root;
    // Settings must stay usable while the OCR models load (or OCR is busy).
    render(root);
    await Promise.all([
        api.ocrLanguages().then((available) => {
            languages = available;
            const control = document.getElementById('ocr-language');
            if (control) {
                control.replaceChildren(...languages.map((l) => el('option', {value: l.tag}, l.name)));
                control.value = store.settings.ocrLanguage;
            }
            refreshOcrRows();
        }),
        refreshStatus(),
    ]);
}

export async function refreshStatus() {
    const previousSession = status?.session || 'windows';
    status = await api.engineStatus();
    if (rootEl && status.session !== previousSession) { render(rootEl); return; }
    const s = document.getElementById('engine-status');
    if (s) { s.textContent = engineStatusText(); }
    const row = document.getElementById('segmenter-row');
    if (row) { fillSegmenterRow(row); }
    const models = document.getElementById('models-rows');
    if (models) { fillModelsRows(models); }
}

/** Switches to another profile (or Global) and shows its settings: every tab is drawn again from
 * them, so the window reloads once the switch is saved. */
async function switchProfile(change) {
    try {
        await saveNow();
        await change();
        location.reload();
    } catch (e) {
        const state = document.getElementById('save-state');
        if (state) { state.textContent = `Erro no perfil: ${e?.message || e}`; }
    }
}

/**
 * Configurações › "Perfil": every setting but the saved words, chosen by hand. Global is the
 * settings as before profiles; a profile keeps only what it changes from them (settings.rs,
 * `sync_profile`), so what it leaves alone follows Global.
 */
function profileRows() {
    const s = store.settings;
    const profiles = s.settingsProfiles || [];
    const active = profiles.find((p) => p.id === s.activeSettingsProfile) || null;
    const choose = select([{value: '', label: 'Global'}, ...profiles.map((p) => ({value: p.id, label: p.name}))],
        active ? active.id : '', (id) => switchProfile(() => api.chooseSettingsProfile(id)));
    choose.id = 'settings-profile';
    choose.setAttribute('aria-label', 'Perfil');
    const add = el('button', {type: 'button', html: `${icon('plus', 'sm')}Novo perfil`});
    // "Novo perfil" asks for the name first; the profile is made only with "Criar" (or Enter).
    const newName = textInput('', () => {}, {placeholder: 'Nome do novo perfil'});
    newName.setAttribute('aria-label', 'Nome do novo perfil');
    const create = el('button', {type: 'button', html: `${icon('plus', 'sm')}Criar`});
    const cancel = el('button', {type: 'button', class: 'ghost'}, 'Cancelar');
    const creating = el('div', {class: 'profile-controls', hidden: true}, newName, create, cancel);
    const showCreating = (on) => {
        creating.hidden = !on;
        choose.hidden = on;
        add.hidden = on;
        if (on) { newName.value = ''; newName.focus?.(); }
    };
    add.addEventListener('click', () => showCreating(true));
    cancel.addEventListener('click', () => showCreating(false));
    const createProfile = () => {
        if (create.disabled) { return; }
        // One profile per click: the window reloads once it is made.
        create.disabled = true;
        cancel.disabled = true;
        const name = newName.value.trim() || `Perfil ${profiles.length + 1}`;
        void switchProfile(() => api.createSettingsProfile(name)).finally(() => {
            create.disabled = false;
            cancel.disabled = false;
        });
    };
    create.addEventListener('click', createProfile);
    newName.addEventListener('keydown', (e) => {
        if (e.key === 'Enter') { createProfile(); }
        if (e.key === 'Escape') { showCreating(false); }
    });
    const changed = active ? Object.keys(active.overrides || {}).length : 0;
    const controls = el('div', {class: 'profile-controls'}, choose, add, creating);
    const head = cardRow({icon: 'settings', title: 'Perfil', wide: true,
        desc: active
            ? `Tudo o que você mudar no app (menos as palavras salvas) fica só neste perfil; o resto continua vindo do Global.${changed ? '' : ' Por enquanto ele está igual ao Global.'}`
            : 'As configurações de sempre. Crie um perfil (por exemplo, um para japonês, com outro idioma, outros dicionários e outros prompts) e troque aqui quando quiser; as palavras salvas são as mesmas em todos.',
        control: controls});
    if (!active) { return [head]; }
    // The name and "Apagar" of the profile in use fold under the "Perfil" row.
    // Renamed when the field is left or Enter is pressed, not on every key.
    const name = textInput(active.name, () => {});
    name.setAttribute('aria-label', 'Nome do perfil');
    const rename = () => {
        const v = name.value.trim();
        if (!v) { name.value = active.name; return; }
        if (v === active.name) { return; }
        active.name = v;
        const option = [...(choose.options || choose.children || [])].find((o) => o.value === active.id);
        if (option) { option.textContent = v; }
        save();
    };
    name.addEventListener('change', rename);
    name.addEventListener('keydown', (e) => { if (e.key === 'Enter') { name.blur(); } });
    const remove = el('button', {type: 'button', html: `${icon('trash', 'sm')}Apagar`});
    remove.addEventListener('click', async () => {
        if (!await ask(`Apagar o perfil "${active.name}"? O app volta a usar o Global.`, {title: 'Apagar perfil', kind: 'warning', okLabel: 'Apagar', cancelLabel: 'Cancelar'})) { return; }
        await switchProfile(() => api.deleteSettingsProfile(active.id));
    });
    const body = cardRow({icon: 'list', title: 'Nome do perfil', wide: true, control: el('div', {class: 'profile-controls'}, name, remove)});
    const collapse = collapser('settingsProfile', () => [body], () => {}, true);
    controls.append(collapse.button);
    collapse.apply();
    return [head, body];
}

function render(root) {
    const s = store.settings;
    const session = status?.session || 'windows';
    const isWindows = session === 'windows';
    const isWayland = session === 'kde-wayland' || session === 'other-wayland';
    root.replaceChildren();

    const languageSelect = select(languages.length ? languages.map((l) => ({value: l.tag, label: l.name})) :
        [{value: s.ocrLanguage, label: s.ocrLanguage}], s.ocrLanguage, (v) => {
        s.ocrLanguage = v;
        save();
        refreshOcrRows();
    });
    languageSelect.id = 'ocr-language';
    root.append(heading('Essenciais'), el('div', {class: 'group'},
        ...profileRows(),
        cardRow({icon: 'scan', title: 'Idioma de reconhecimento', desc: 'O idioma do texto na tela', wide: true,
            control: languageSelect}),
        segmenterRow(),
        cardRow({icon: 'swap', title: 'Modo do Lookitan', desc: 'Dicionário popup para uma palavra, ou tradução de todo o texto da tela',
            control: segmented([
                {value: 'popup', label: 'Dicionário popup', icon: 'book'},
                {value: 'translator', label: 'Tradutor de tela (em breve)', icon: 'translate'},
            ], s.appMode, (v) => { s.appMode = v; save(); })}),
        cardRow({icon: 'bookmark', title: 'Salvar palavras automaticamente', control: toggle(s.saveWordsAutomatically, (v) => { s.saveWordsAutomatically = v; save(); })}),
    ));

    const longPress = toggle(s.mouseHotkey.longPress, (v) => { s.mouseHotkey.longPress = v; save(); rerender(); }, {on: 'Segurar', off: 'Clicar'});
    root.append(heading('Atalhos'), el('div', {class: 'group'},
        cardRow({icon: 'keyboard', title: 'Teclado', desc: 'Pressione para consultar a palavra sob o cursor.', wide: true, control: keyboardBox()}),
        cardRow({icon: 'mouse', title: 'Mouse', desc: isWindows ? 'Um botão do mouse, com Alt, Ctrl ou Shift se quiser.' : 'No Linux, use o atalho de teclado ou o comando abaixo.', wide: true, control: mouseBox(isWindows)}),
        isWindows ? cardRow({icon: 'mouse', title: 'Acionar o mouse', desc: 'Clicar dispara na hora; segurar dispara após meio segundo e mantém o clique normal.', control: longPress}) : null,
        ...gamepadSection(),
        isWayland ? cardRow({icon: 'keyboard', title: 'Atalho no Wayland', desc: 'Em Configurações do Sistema › Atalhos › Adicionar novo › Comando, use este comando:', wide: true,
            control: (() => { const i = textInput('lookitan --scan', () => {}); i.readOnly = true; return i; })()}) : null,
    ));

    const lensKey = textInput(s.lensApiKey, (v) => { s.lensApiKey = v.trim(); save(); }, {type: 'password', placeholder: 'Opcional'});
    const usesLens = selectorEngine(true) === 'lens' || selectorEngine(false) === 'lens';
    root.append(heading('OCR'), el('div', {class: 'group'},
        ocrEnginesBox(),
        modelsRows(),
        usesLens ? cardRow({icon: 'cloud', title: 'Chave da API do Google Lens', desc: 'Opcional nos instaladores do GitHub, que já vêm com uma chave. Cole a sua se ela parar de funcionar.', wide: true, control: lensKey}) : null,
        captureRegionRows(),
        cardRow({icon: 'settings', title: 'Escolher a placa de vídeo', desc: 'Para computadores com duas placas, como Intel junto com NVIDIA. Desligado, o sistema escolhe.',
            control: toggle(!!s.chooseGpu, (v) => { s.chooseGpu = v; save(); rerender(); })}),
        s.chooseGpu ? cardRow({icon: 'settings', title: 'Placa usada no OCR', desc: 'Vale a partir da próxima leitura.', wide: true,
            control: select([
                {value: 'highPerformance', label: 'Dedicada (mais rápida)'},
                {value: 'lowPower', label: 'Integrada (gasta menos energia)'},
            ], s.gpuPreference || 'highPerformance', (v) => { s.gpuPreference = v; save(); })}) : null,
        el('div', {class: 'card-row'}, el('span', {class: 'ico', html: icon('alert')}),
            el('div', {class: 'txt'}, el('div', {class: 't'}, 'Estado do motor'), el('div', {class: 'd', id: 'engine-status', style: 'white-space: pre-line'}, engineStatusText()))),
    ));

    root.append(heading('Jogos e programas'), profilesCard());

    root.append(heading('Áudio'));
    const audioRoot = el('div', {class: 'group', id: 'audio-settings'});
    root.append(audioRoot);
    renderAudioSection(audioRoot);

    root.append(heading('Anki'));
    const ankiRoot = el('div', {class: 'group', id: 'anki-settings'});
    root.append(ankiRoot);
    renderAnkiSection(ankiRoot);

    const version = el('span', {class: 'muted'});
    void getVersion().then((v) => { version.textContent = `Versão ${v}`; });
    root.append(heading('Aplicativo'), el('div', {class: 'group'},
        cardRow({icon: 'power', title: 'Iniciar o Lookitan com o sistema', control: toggle(s.autostart, (v) => { s.autostart = v; save(); })}),
        transferRow(),
        el('div', {class: 'card-row'}, el('img', {src: '/app-icon.svg', style: 'width:24px;height:24px'}),
            el('div', {class: 'txt'}, el('div', {class: 't'}, 'Lookitan'), version),
            el('div', {class: 'ctl'}, el('span', {class: 'note', html: escapeHtml('GPL-3.0 · BYOK de yomitan-byok-ai · KWin via MaruPop · PP-OCR (Apache-2.0)')}))),
    ));
}
