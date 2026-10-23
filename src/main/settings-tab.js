// Settings tab: essentials, shortcuts, OCR, per-app profiles, word audio, Anki and application options.

import {getVersion} from '@tauri-apps/api/app';
import {ask} from '@tauri-apps/plugin-dialog';
import {api, escapeHtml} from '../lib/api.js';
import {buttonName, detectLayout} from '../lib/gamepad-buttons.js';
import {icon} from '../lib/icons.js';
import {renderAnkiSection} from './anki-panel.js';
import {renderAudioSection} from './audio-panel.js';
import {transferRow} from './settings-transfer.js';
import {store, save} from './store.js';
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
];
const GAMEPAD_DEFAULTS = {hotkey: [], navigation: true, confirm: ['A'], close: ['B'], save: ['X'], pronounce: ['Y'], explain: ['RB'], anki: ['LB']};
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
 * and let go. `get()` and `set(buttons)` read and store them.
 */
function gamepadComboBox(get, set) {
    const val = el('span', {class: 'val'});
    const show = () => {
        const text = gamepadLabel(get());
        val.textContent = text || 'Não definido';
        val.classList.toggle('placeholder', !text);
    };
    show();
    const clear = el('button', {class: 'ghost icon', title: 'Remover', html: icon('close', 'sm')});
    const box = el('div', {class: 'hotkey', tabindex: '0', html: icon('gamepad')}, val, clear);
    let capturing = false;
    const stop = () => {
        if (!capturing) { return; }
        capturing = false;
        box.classList.remove('capturing');
        void api.gamepadCapture(false);
        show();
    };
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
                const other = GAMEPAD_ACTIONS.find(([k]) => k !== key && sameButtons(g[k], buttons));
                if (other) { g[other[0]] = g[key]; }
                g[key] = buttons;
                save();
                rerender();
            });
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

function engineStatusText() {
    if (!status) { return 'Carregando…'; }
    if (status.runtimeError) { return `ONNX Runtime: ${status.runtimeError}`; }
    if (status.error) { return status.error; }
    if (!status.detModel) { return 'Carregando o motor…'; }
    const engine = `Detecção ${status.detModel} na ${status.detOnGpu ? (status.session === 'windows' ? 'GPU (DirectML)' : 'GPU (WebGPU)') : 'CPU'} · reconhecimento ${status.recModel} na ${status.recOnGpu ? 'GPU' : 'CPU'}`;
    return status.segmenter ? `${engine} · ${status.segmenter}` : engine;
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
        if (modeChanged) { save(); }
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
}

function render(root) {
    const s = store.settings;
    const session = status?.session || 'windows';
    const isWindows = session === 'windows';
    const isWayland = session === 'kde-wayland' || session === 'other-wayland';
    root.replaceChildren();

    const languageSelect = select(languages.length ? languages.map((l) => ({value: l.tag, label: l.name})) :
        [{value: s.ocrLanguage, label: s.ocrLanguage}], s.ocrLanguage, (v) => { s.ocrLanguage = v; save(); });
    languageSelect.id = 'ocr-language';
    root.append(heading('Essenciais'), el('div', {class: 'group'},
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
    const screenEngine = s.screenOcrEngine ?? s.ocrEngine;
    const usesLens = screenEngine === 'lens' || s.ocrEngine === 'lens';
    const engineDesc = (engine) => engine === 'lens'
        ? 'Envia a captura ao Google (API não oficial). Se falhar, o PP-OCRv6 assume.'
        : 'Roda no seu computador; nada sai dele.';
    const engineSelect = (value, set) => select([
        {value: 'paddle', label: 'PP-OCRv6 (local)'},
        {value: 'lens', label: 'Google Lens (nuvem)'},
    ], value, (v) => { set(v); save(); rerender(); });
    root.append(heading('OCR'), el('div', {class: 'group'},
        cardRow({icon: 'scan', title: 'OCR em volta do cursor', desc: `Lê a palavra sob o cursor. ${engineDesc(s.ocrEngine)}`, wide: true,
            control: engineSelect(s.ocrEngine, (v) => { s.ocrEngine = v; })}),
        cardRow({icon: 'scan', title: 'OCR da tela inteira', desc: `Usado quando o atalho é apertado fora de uma palavra; depois é só clicar na palavra. ${engineDesc(screenEngine)}`, wide: true,
            control: engineSelect(screenEngine, (v) => { s.screenOcrEngine = v; })}),
        usesLens ? cardRow({icon: 'cloud', title: 'Chave da API do Google Lens', desc: 'Opcional nos instaladores do GitHub, que já vêm com uma chave. Cole a sua se ela parar de funcionar.', wide: true, control: lensKey}) : null,
        captureRegionRows(),
        cardRow({icon: 'settings', title: 'Usar a placa de vídeo', desc: isWindows ? 'DirectML, com volta automática para a CPU.' : 'WebGPU via Vulkan (experimental), com volta automática para a CPU.', control: toggle(s.useGpu, (v) => { s.useGpu = v; save(); })}),
        cardRow({icon: 'settings', title: 'Escolher a placa de vídeo', desc: 'Para computadores com duas placas, como Intel junto com NVIDIA. Desligado, o sistema escolhe.',
            control: toggle(!!s.chooseGpu, (v) => { s.chooseGpu = v; save(); rerender(); })}),
        s.chooseGpu ? cardRow({icon: 'settings', title: 'Placa usada no OCR', desc: 'Vale a partir da próxima leitura.', wide: true,
            control: select([
                {value: 'highPerformance', label: 'Dedicada (mais rápida)'},
                {value: 'lowPower', label: 'Integrada (gasta menos energia)'},
            ], s.gpuPreference || 'highPerformance', (v) => { s.gpuPreference = v; save(); })}) : null,
        el('div', {class: 'card-row'}, el('span', {class: 'ico', html: icon('alert')}),
            el('div', {class: 'txt'}, el('div', {class: 't'}, 'Estado do motor'), el('div', {class: 'd', id: 'engine-status'}, engineStatusText()))),
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
