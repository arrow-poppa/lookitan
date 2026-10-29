// Run with: node --experimental-vm-modules --test scripts/ui-startup.test.mjs
// Exercise the actual UI modules with a small DOM and delayed Tauri responses.
import assert from 'node:assert/strict';
import {readFile} from 'node:fs/promises';
import {resolve} from 'node:path';
import test from 'node:test';
import {createContext, SourceTextModule, SyntheticModule} from 'node:vm';
import {normalizeAiOptions} from '../src/lib/ai/defaults.js';

const project = resolve(import.meta.dirname, '..');
const file = (path) => resolve(project, path);
const flush = () => new Promise((done) => setImmediate(done));
function deferred() {
    let resolve, reject;
    const promise = new Promise((yes, no) => { resolve = yes; reject = no; });
    return {promise, resolve, reject};
}

class Element {
    constructor(tag = 'div') {
        this.tagName = tag;
        this.children = [];
        this.attributes = {};
        this.listeners = {};
        this.style = {};
        this.dataset = {};
        this._text = '';
        this.className = '';
        this.classList = {
            toggle: (name, on) => {
                const names = new Set(this.className.split(' ').filter(Boolean));
                if (on) { names.add(name); } else { names.delete(name); }
                this.className = [...names].join(' ');
            },
            add: (name) => this.classList.toggle(name, true),
            remove: (name) => this.classList.toggle(name, false),
            contains: (name) => this.className.split(' ').includes(name),
        };
    }
    set textContent(text) { this._text = text; this.children = []; }
    get textContent() { return this._text + this.children.map((child) => child.textContent).join(''); }
    set value(value) { this._value = String(value); }
    get value() {
        if (this._value !== undefined) { return this._value; }
        if (this.tagName === 'select') { return (this.children.find((child) => child.selected) || this.children[0])?.value || ''; }
        return '';
    }
    setAttribute(name, value) {
        this.attributes[name] = value;
        if (name === 'id') { this.id = value; }
        if (name === 'value') { this.value = value; }
    }
    append(...children) { this.children.push(...children); }
    replaceChildren(...children) { this._text = ''; this.children = children; }
    addEventListener(type, listener) { (this.listeners[type] ||= []).push(listener); }
    dispatch(type) { for (const listener of this.listeners[type] || []) { listener({target: this}); } }
    querySelector(selector) { return descendants(this).find((node) => node.classList.contains(selector.slice(1))); }
}
function descendants(root) { return root.children.flatMap((child) => [child, ...descendants(child)]); }
function dom() {
    const roots = [];
    const document = {
        createElement: (tag) => new Element(tag),
        createTextNode: (text) => { const node = new Element('#text'); node.textContent = text; return node; },
        getElementById: (id) => roots.flatMap((root) => [root, ...descendants(root)]).find((node) => node.id === id),
        querySelectorAll: () => [],
    };
    return {document, roots, add: (id) => { const node = new Element(); node.id = id; roots.push(node); return node; }};
}
async function load(path, globals, stubs) {
    const context = createContext({...globals, Node: Element, console: {error() {}}, Intl});
    const modules = new Map();
    // The promise is kept at once, so a module imported twice at the same time (a diamond in the
    // import graph) is created and linked only once.
    function get(id) {
        if (!modules.has(id)) {
            modules.set(id, (async () => {
                const exports = stubs[id];
                const module = exports ? new SyntheticModule(Object.keys(exports), function () {
                    for (const [name, value] of Object.entries(exports)) { this.setExport(name, value); }
                }, {context, identifier: id}) : new SourceTextModule(await readFile(id, 'utf8'), {context, identifier: id});
                return module;
            })());
        }
        return modules.get(id);
    }
    const module = await get(file(path));
    await module.link((specifier, owner) => get(specifier.startsWith('.') ? resolve(owner.identifier, '..', specifier) : specifier));
    await module.evaluate();
    return module.namespace;
}

async function settingsFixture(overrides = {}) {
    const view = dom();
    const root = view.add('settings-col');
    const languages = deferred();
    const status = deferred();
    let saves = 0;
    const calls = [];
    let later = null;
    const gamepadCapture = {handler: null};
    const settings = {ocrLanguage: 'en-US', appMode: 'popup', profiles: [], mouseHotkey: {button: 'none'}, useGpu: true,
        ocrScope: 'screen', captureWidth: 900, captureHeight: 260, ...overrides};
    const ui = await load('src/main/settings-tab.js', {document: view.document, location: {reload: () => calls.push('reload')}}, {
        '@tauri-apps/api/app': {getVersion: async () => 'test'},
        '@tauri-apps/plugin-dialog': {ask: async () => true},
        [file('src/lib/api.js')]: {api: {ocrLanguages: () => languages.promise, engineStatus: () => later ?? status.promise,
            installSegmenter: async () => { calls.push('install'); }, removeSegmenter: async () => { calls.push('remove'); },
            gamepadStatus: async () => 'Xbox Controller', gamepadCapture: async (on) => { calls.push(`capture ${on}`); },
            onGamepadCapture: async (handler) => { gamepadCapture.handler = handler; },
            chooseSettingsProfile: async (id) => { calls.push(`choose ${id}`); }, createSettingsProfile: async (name) => { calls.push(`create ${name}`); },
            deleteSettingsProfile: async (id) => { calls.push(`delete ${id}`); }}, escapeHtml: String},
        [file('src/lib/icons.js')]: {icon: () => ''},
        [file('src/main/anki-panel.js')]: {renderAnkiSection: async () => {}},
        [file('src/main/audio-panel.js')]: {renderAudioSection: () => {}},
        [file('src/main/settings-transfer.js')]: {transferRow: () => null},
        [file('src/main/store.js')]: { saveNow: async () => {},store: {settings, anki: {}}, save: () => { saves += 1; }},
    });
    const initialization = ui.initSettingsTab(root);
    return {view, root, settings, languages, status, initialization, calls, ui, gamepadCapture,
        setStatus: (next) => { later = Promise.resolve(next); }, saves: () => saves, finish: async () => {
        languages.resolve([{tag: 'en-US', name: 'English'}]);
        status.resolve({session: 'windows'});
        await initialization;
    }};
}

test('Configurações chooses a settings profile above the recognition language, then shows its settings', async () => {
    const fixture = await settingsFixture({settingsProfiles: [{id: 'p1', name: 'Japonês', overrides: {ocrLanguage: 'ja-JP'}}], activeSettingsProfile: ''});
    await fixture.finish();
    const all = descendants(fixture.root);
    const picker = all.find((node) => node.id === 'settings-profile');
    assert(picker, 'the profile row is shown');
    assert(all.indexOf(picker) < all.indexOf(all.find((node) => node.id === 'ocr-language')), 'above the recognition language');
    assert.deepEqual(picker.children.map((o) => o.textContent), ['Global', 'Japonês']);
    picker.value = 'p1';
    picker.dispatch('change');
    await new Promise((done) => setTimeout(done, 0));
    assert.deepEqual(fixture.calls.filter((c) => c.startsWith('choose') || c === 'reload'), ['choose p1', 'reload'], 'the switch is made by the back end, then the window shows it');
    const inProfile = await settingsFixture({settingsProfiles: [{id: 'p1', name: 'Japonês', overrides: {}}], activeSettingsProfile: 'p1'});
    await inProfile.finish();
    const rename = descendants(inProfile.root).find((node) => node.attributes?.['aria-label'] === 'Nome do perfil');
    assert(rename, 'a profile in use can be renamed');
    const nameRow = inProfile.root.children[1].children.find((row) => descendants(row).includes(rename));
    assert.equal(nameRow?.hidden, true, 'the name row starts folded under "Perfil"');
});

test('"Novo perfil" asks for the name and makes one profile, however often Criar is clicked', async () => {
    const fixture = await settingsFixture({settingsProfiles: [], activeSettingsProfile: ''});
    await fixture.finish();
    const all = descendants(fixture.root);
    const button = (label) => all.find((node) => node.tagName === 'button' && (node.attributes.html || node.innerHTML || node.textContent || '').includes(label));
    button('Novo perfil').dispatch('click');
    assert.deepEqual(fixture.calls.filter((c) => c.startsWith('create')), [], 'nothing is made before the name');
    const field = all.find((node) => node.attributes?.['aria-label'] === 'Nome do novo perfil');
    assert.equal(field.parentNode?.hidden ?? false, false, 'the name field is shown');
    field.value = 'Japonês';
    button('Criar').dispatch('click');
    button('Criar').dispatch('click');
    await new Promise((done) => setTimeout(done, 0));
    assert.deepEqual(fixture.calls.filter((c) => c.startsWith('create') || c === 'reload'), ['create Japonês', 'reload']);
});

async function dictionaryFixture(overrides = {}) {
    const view = dom();
    view.add('add-dict');
    const root = view.add('dict-detail');
    const list = view.add('dict-list');
    const ai = normalizeAiOptions(overrides);
    const settings = {definitionLanguage: 'en-US', translationLanguage: 'pt-BR', hideWordTranslation: false};
    let saves = 0;
    const ui = await load('src/main/dicts-tab.js', {document: view.document}, {
        '@tauri-apps/plugin-dialog': {open: async () => null, ask: async () => true},
        [file('src/lib/api.js')]: {api: {listDictionaries: async () => [
            {id: 'ai-word', kind: 'ai-word', enabled: true}, {id: 'ai-context', kind: 'ai-context', enabled: true},
            {id: 'ai-images', kind: 'ai-images', enabled: true},
        ]}, escapeHtml: String},
        [file('src/lib/icons.js')]: {icon: () => ''},
        [file('src/lib/ai/explain.js')]: {DEFAULT_MODELS: {}, explain: async () => 'test answer'},
        [file('src/main/store.js')]: { saveNow: async () => {},store: {settings, ai}, save: () => { saves++; }},
    });
    await ui.initDictsTab();
    const control = (path) => descendants(root).find((node) => node.attributes['data-setting'] === `ai.${path}`);
    const change = (path, value, event = 'change') => {
        const node = control(path);
        assert(node, `${path} must have a settings control`);
        node.value = value;
        node.dispatch(event);
    };
    return {view, root, list, ai, settings, control, change, saves: () => saves};
}

test('every BYOK provider option remains reachable when switching AI providers', async () => {
    const fixture = await dictionaryFixture();
    // All provider fields in yomitan-byok-ai/ext/data/schemas/options-schema.json.
    const fields = {
        openai: ['apiKey', 'model'],
        gemini: ['apiKey', 'model', 'thinkingLevel'],
        deepseek: ['apiKey', 'model', 'thinkingMode', 'thinkingIntensity'],
        custom: ['apiKey', 'model', 'endpoint', 'providerRoutingMode', 'providerSlugs',
            'providerAllowFallbacks', 'thinkingMode', 'thinkingIntensity', 'customThinkingIntensity', 'requestBodyJson'],
    };
    for (const [provider, keys] of Object.entries(fields)) {
        fixture.change('provider', provider);
        for (const key of keys) { assert(fixture.control(`${provider}.${key}`), `${provider}.${key} must be visible`); }
        for (const key of ['provider', 'temperature', 'customPrompt', 'systemPrompt', 'autoGenerateOnLookup',
            'streamResponse', 'cancelPendingRequests', 'autoExplain', 'cardPrompt', 'wordTranslationPrompt']) {
            assert(fixture.control(key), `${key} must be visible`);
        }
        // Removed at the user's request: Lookupper has no such option, and the AI reads the word
        // without the dictionaries.
        assert(!fixture.control('unknownWordFallback'));
        assert(!descendants(fixture.root).some((node) => node.tagName === '#text' && node.textContent === 'null'));
    }
});

test('custom thinking effort, manual values, routing and JSON survive provider switches', async () => {
    const fixture = await dictionaryFixture({provider: 'custom', custom: {model: 'saved-model', apiKey: 'test-only-key',
        endpoint: 'http://localhost:1234/v1/chat/completions', thinkingMode: 'enabled', thinkingIntensity: 'custom',
        customThinkingIntensity: '{"max_tokens":2000}', requestBodyJson: '{"top_p":0.9}'}});
    const {control, change, ai} = fixture;
    assert.equal(control('custom.customThinkingIntensity').value, '{"max_tokens":2000}');
    const efforts = control('custom.thinkingIntensity').children.map((option) => option.value);
    for (const value of ['', 'none', 'minimal', 'low', 'medium', 'high', 'xhigh', 'max', 'custom']) { assert(efforts.includes(value)); }
    // Each selector has its own row; it cannot be clipped by a second 220px selector.
    for (const row of descendants(fixture.root).filter((node) => node.className === 'card-row')) {
        assert(descendants(row).filter((node) => node.tagName === 'select').length <= 1);
    }
    change('custom.thinkingIntensity', 'medium');
    change('custom.providerRoutingMode', 'only');
    change('custom.providerSlugs', 'deepinfra/turbo, fireworks', 'input');
    change('custom.customThinkingIntensity', '4096', 'input');
    change('provider', 'gemini');
    change('provider', 'custom');
    assert.equal(control('custom.thinkingIntensity').value, 'medium');
    assert.equal(control('custom.customThinkingIntensity').value, '4096');
    assert.equal(control('custom.providerRoutingMode').value, 'only');
    assert.equal(control('custom.providerSlugs').value, 'deepinfra/turbo, fireworks');
    assert.equal(ai.custom.model, 'saved-model');
    assert.equal(ai.custom.apiKey, 'test-only-key');
    assert.equal(ai.custom.endpoint, 'http://localhost:1234/v1/chat/completions');
    assert.equal(ai.custom.requestBodyJson, '{"top_p":0.9}');
    assert(fixture.saves() >= 6);
});

test('the context translator has its own provider, which changes only the translator', async () => {
    const fixture = await dictionaryFixture({provider: 'custom', custom: {model: 'word-model', thinkingIntensity: 'low', customThinkingIntensity: '2048'}});
    fixture.list.children[1].dispatch('click');
    const own = 'sectionProviders.ai-context.';
    assert(fixture.control('contextPrompt'));
    assert(fixture.control('autoTranslateContext'), 'the translator has its own automatic switch');
    assert(fixture.control('hideContextTranslation'), 'the translator can hide its translation');
    for (const key of ['provider', 'custom.model', 'temperature', 'streamResponse', 'cancelPendingRequests', 'systemPrompt']) {
        assert(fixture.control(`${own}${key}`), `the translator has its own ${key}`);
    }
    for (const key of ['provider', 'custom.thinkingIntensity', 'autoGenerateOnLookup', 'autoExplain', 'streamResponse']) {
        assert(!fixture.control(key), `${key} belongs to "IA — palavra no contexto"`);
    }
    assert.equal(fixture.control(`${own}custom.thinkingIntensity`).value, 'low', 'it starts as a copy of the word card provider');
    fixture.change(`${own}custom.model`, 'context-model', 'input');
    fixture.change(`${own}custom.thinkingIntensity`, 'high');
    fixture.change(`${own}provider`, 'gemini');
    const section = fixture.ai.sectionProviders['ai-context'];
    assert.equal(section.provider, 'gemini');
    assert.equal(section.custom.model, 'context-model');
    assert.equal(section.custom.thinkingIntensity, 'high');
    assert.equal(fixture.ai.provider, 'custom', 'the word card keeps its provider');
    assert.equal(fixture.ai.custom.model, 'word-model');
    assert.equal(fixture.ai.custom.thinkingIntensity, 'low');
    assert.equal(fixture.ai.sectionProviders['ai-images'].custom.model, 'word-model', 'and so do the pictures');
});

test('the images dictionary has its own image query prompts, switch and provider', async () => {
    const fixture = await dictionaryFixture({provider: 'openai', openai: {model: 'kept-model'}});
    fixture.list.children[2].dispatch('click');
    for (const key of ['autoShowImages', 'imageQueryUseAi', 'imageQuerySystemPrompt', 'imageQueryPrompt',
        'sectionProviders.ai-images.provider', 'sectionProviders.ai-images.openai.model']) {
        assert(fixture.control(key), `${key} must be visible`);
    }
    for (const key of ['provider', 'openai.model', 'autoGenerateOnLookup', 'autoExplain', 'systemPrompt']) {
        assert(!fixture.control(key), `${key} belongs to "IA — palavra no contexto"`);
    }
    assert.equal(fixture.control('imageQueryPrompt').value, fixture.ai.imageQueryPrompt);
    fixture.change('imageQueryPrompt', 'Word: {{target}}', 'input');
    assert.equal(fixture.ai.imageQueryPrompt, 'Word: {{target}}');
    fixture.change('sectionProviders.ai-images.openai.model', 'cheap-model', 'input');
    assert.equal(fixture.ai.sectionProviders['ai-images'].openai.model, 'cheap-model');
    assert.equal(fixture.ai.openai.model, 'kept-model');
});

test('the pop-up order is changed by dragging a row or with Alt+arrows, without HTML drag and drop', async () => {
    const view = dom();
    view.add('add-dict');
    view.add('dict-detail');
    const list = view.add('dict-list');
    let order = [{id: 'ai-word', kind: 'ai-word', enabled: true}, {id: 'ai-context', kind: 'ai-context', enabled: true}, {id: 'local', kind: 'mdict', name: 'Local', enabled: true}];
    const moves = [];
    const ui = await load('src/main/dicts-tab.js', {document: view.document, setTimeout}, {
        '@tauri-apps/plugin-dialog': {open: async () => null, ask: async () => true},
        [file('src/lib/api.js')]: {api: {
            listDictionaries: async () => order,
            updateDictionary: async (id, {moveTo}) => {
                moves.push([id, moveTo]);
                const item = order.find((d) => d.id === id);
                order = order.filter((d) => d !== item);
                order.splice(moveTo, 0, item);
                return order;
            },
        }, escapeHtml: String},
        [file('src/lib/icons.js')]: {icon: () => ''},
        [file('src/lib/ai/explain.js')]: {DEFAULT_MODELS: {}, explain: async () => ''},
        [file('src/main/store.js')]: { saveNow: async () => {},store: {settings: {definitionLanguage: 'en-US', translationLanguage: 'pt-BR'}, ai: normalizeAiOptions({})}, save: () => {}},
    });
    await ui.initDictsTab();
    // Rows whose middles are at 20, 60 and 100 px.
    assert.equal(ui.dropIndex([20, 60, 100], 75, 0), 1, 'the first row dragged past the second');
    assert.equal(ui.dropIndex([20, 60, 100], 130, 0), 2, 'to the end');
    assert.equal(ui.dropIndex([20, 60, 100], 5, 2), 0, 'the last row dragged to the top');
    assert.equal(ui.dropIndex([20, 60, 100], 90, 2), 2, 'not past any row: it stays');
    for (const row of list.children) {
        assert.equal(row.attributes.draggable, undefined, 'no HTML drag and drop, which the WebView does not deliver');
        assert(row.listeners.pointerdown && row.listeners.pointermove && row.listeners.pointerup);
    }
    const key = (row, k) => row.listeners.keydown[0]({key: k, altKey: true, preventDefault() {}});
    key(list.children[0], 'ArrowDown');
    await new Promise((done) => setImmediate(done));
    assert.deepEqual(moves, [['ai-word', 1]]);
    assert.deepEqual(order.map((d) => d.id), ['ai-context', 'ai-word', 'local']);
    key(list.children[0], 'ArrowUp');
    await new Promise((done) => setImmediate(done));
    assert.equal(moves.length, 1, 'the top row cannot move up');
    // A mouse drag: rows are 40 px high, so their middles are at 20, 60 and 100.
    list.insertBefore = (node, ref) => {
        list.children = list.children.filter((row) => row !== node);
        const at = ref ? list.children.indexOf(ref) : list.children.length;
        list.children.splice(at, 0, node);
    };
    for (const row of list.children) {
        row.getBoundingClientRect = () => ({top: list.children.indexOf(row) * 40, height: 40});
        row.setPointerCapture = () => {};
    }
    const last = list.children[2];
    const pointer = (type, clientY) => last.listeners[type][0]({type, clientY, pointerId: 7, button: 0, target: {closest: () => null}});
    pointer('pointerdown', 100);
    pointer('pointermove', 98);
    assert.equal(list.children[2], last, 'a few pixels are not a drag');
    pointer('pointermove', 10);
    assert.equal(list.children[0], last, 'the row moves among the others while dragged');
    pointer('pointerup', 10);
    await new Promise((done) => setImmediate(done));
    assert.deepEqual(moves.at(-1), ['local', 0]);
    assert.deepEqual(order.map((d) => d.id), ['local', 'ai-context', 'ai-word']);
});

test('settings render and GPU toggle work while OCR status is pending', async () => {
    const fixture = await settingsFixture();
    const {view, root, settings, languages, status, initialization} = fixture;
    assert(root.children.length > 0, 'settings must render before any backend response');
    const languageControl = view.document.getElementById('ocr-language');
    assert(languageControl, 'recognition language control is already mounted');
    const gpuRow = descendants(root).find((node) => node.className === 'card-row' && node.textContent.includes('Usar a placa de vídeo'));
    const gpu = descendants(gpuRow).find((node) => node.tagName === 'input');
    gpu.checked = false;
    gpu.dispatch('change');
    assert.equal(settings.useGpu, false);
    assert.equal(fixture.saves(), 1);
    languages.resolve([{tag: 'en-US', name: 'English'}, {tag: 'pt-BR', name: 'Português'}]);
    await flush();
    assert.equal(languageControl.children.length, 2);
    assert.equal(languageControl.value, 'en-US');
    languageControl.value = 'pt-BR';
    languageControl.dispatch('change');
    assert.equal(settings.ocrLanguage, 'pt-BR');
    status.resolve({session: 'windows', detModel: 'test-det', recModel: 'test-rec', detOnGpu: false, recOnGpu: false});
    await initialization;
    assert(view.document.getElementById('engine-status').textContent.includes('test-det'));
    assert.equal(view.document.getElementById('ocr-language'), languageControl, 'status must not replace the edited controls');
});

test('the gamepad section folds, records combinations and each action takes one button or several', async () => {
    const fixture = await settingsFixture();
    await fixture.finish();
    await flush();
    assert(fixture.root.textContent.includes('Conectado: Xbox Controller'));
    const box = (key) => descendants(fixture.root).find((node) => node.attributes['data-setting'] === `gamepad.${key}`);
    const record = (key, buttons) => {
        const target = box(key);
        target.focus = () => {};
        target.closest = () => null;
        target.dispatch('click');
        fixture.gamepadCapture.handler({buttons, done: false});
        const live = target.textContent;
        fixture.gamepadCapture.handler({buttons, done: true});
        return live;
    };
    assert.equal(record('hotkey', ['LB', 'Y']), 'LB + Y…');
    assert(fixture.calls.includes('capture true'));
    assert.deepEqual([...fixture.settings.gamepad.hotkey], ['LB', 'Y']);
    // Actions: a combination, and a combination another action has swaps with it.
    record('save', ['LT', 'A']);
    assert.deepEqual([...fixture.settings.gamepad.save], ['LT', 'A']);
    record('confirm', ['LT', 'A']);
    assert.deepEqual([...fixture.settings.gamepad.confirm], ['LT', 'A']);
    assert.deepEqual([...fixture.settings.gamepad.save], ['A'], 'the action that had LT + A takes A');
    for (const key of ['explain', 'anki', 'pronounce', 'close', 'meme']) { assert(box(key), `${key} has its box`); }
    // × leaves an action empty; ↺ puts its default back.
    const buttonOf = (key, title) => box(key).children.find((node) => node.tagName === 'button' && String(node.attributes.title).startsWith(title));
    buttonOf('pronounce', 'Deixar vazio').dispatch('click');
    assert.deepEqual([...fixture.settings.gamepad.pronounce], []);
    assert.deepEqual([...fixture.settings.gamepad.meme], [], 'an empty action takes nothing from another empty one');
    buttonOf('pronounce', 'Voltar ao padrão').dispatch('click');
    assert.deepEqual([...fixture.settings.gamepad.pronounce], ['Y']);
    assert(!buttonOf('meme', 'Voltar ao padrão'), 'meme mode has no default button');
    assert(fixture.root.textContent.includes('Explicar: RB'));
    // The chevron folds everything under the Gamepad row, as in Lookupper.
    const head = descendants(fixture.root).find((node) => node.className === 'gamepad-ctl');
    const chevron = head.children.find((node) => node.className === 'ghost icon collapse');
    chevron.dispatch('click');
    const navigation = descendants(fixture.root).find((node) => node.className === 'card-row' && node.textContent.startsWith('Navegação pelo gamepad'));
    assert.equal(navigation.hidden, true);
});

test('the word analyzer row deletes it, then downloads it', async () => {
    const fixture = await settingsFixture({ocrLanguage: 'ja-JP'});
    const {view, languages, status, initialization, calls} = fixture;
    const row = () => view.document.getElementById('segmenter-row');
    assert.equal(row().children.length, 0, 'nothing before the status arrives');
    languages.resolve([{tag: 'ja-JP', name: 'Japanese'}]);
    status.resolve({session: 'windows', segmenter: 'separação de palavras (Sudachi): pronta', segmenterInstalled: true});
    await initialization;
    assert(row().textContent.includes('Separação de palavras (Sudachi)'));
    assert(row().textContent.includes('Estado: pronta'));
    const button = (label) => descendants(row()).find((node) => node.tagName === 'button' && (node.attributes.html || node.innerHTML || '').includes(label));
    assert.equal(button('Baixar'), undefined, 'installed: only Apagar');
    button('Apagar').dispatch('click');
    await flush();
    assert.deepEqual(calls, ['remove']);
    // The status after the removal: no files, so only Baixar.
    fixture.setStatus({session: 'windows', segmenter: 'separação de palavras (Sudachi): não instalada', segmenterInstalled: false});
    await fixture.ui.refreshStatus();
    assert.equal(button('Apagar'), undefined);
    button('Baixar').dispatch('click');
    await flush();
    assert.deepEqual(calls, ['remove', 'install']);
});

test('the video card choice stays hidden until its toggle is on', async () => {
    const fixture = await settingsFixture();
    const {root, settings} = fixture;
    const row = (title) => descendants(root).find((node) => node.className === 'card-row' && node.textContent.includes(title));
    assert.equal(row('Placa usada no OCR'), undefined, 'off by default: the system picks the card');
    const choose = descendants(row('Escolher a placa de vídeo')).find((node) => node.tagName === 'input');
    choose.checked = true;
    choose.dispatch('change');
    assert.equal(settings.chooseGpu, true);
    const card = descendants(row('Placa usada no OCR')).find((node) => node.tagName === 'select');
    assert.equal(card.value, 'highPerformance');
    card.value = 'lowPower';
    card.dispatch('change');
    assert.equal(settings.gpuPreference, 'lowPower');
    await fixture.finish();
});

test('in the manual mode a tiny language chooses tiny or small for itself, and the others show their only model', async () => {
    const fixture = await settingsFixture({ocrLanguage: 'pt-BR', smallRecognizer: ['de-DE'], ocrManual: true});
    const {view, settings} = fixture;
    const recognizer = () => descendants(view.document.getElementById('recognizer-row')).find((node) => node.tagName === 'select');
    assert.equal(recognizer(), undefined, 'shown once the languages are known');
    fixture.languages.resolve([{tag: 'ja-JP', name: 'Japanese', rec: 'rec_small'}, {tag: 'pt-BR', name: 'Portuguese (Brazil)', rec: 'rec_tiny'},
        {tag: 'ar-SA', name: 'Arabic', rec: 'rec_arabic'}]);
    fixture.status.resolve({session: 'windows'});
    await fixture.initialization;
    assert.deepEqual(recognizer().children.map((o) => o.textContent), ['Tiny (padrão)', 'Small']);
    assert.equal(recognizer().value, 'tiny');
    recognizer().value = 'small';
    recognizer().dispatch('change');
    assert.equal(JSON.stringify(settings.smallRecognizer), '["de-DE","pt-BR"]');
    recognizer().value = 'tiny';
    recognizer().dispatch('change');
    assert.equal(JSON.stringify(settings.smallRecognizer), '["de-DE"]', 'other languages keep their choice');
    const language = view.document.getElementById('ocr-language');
    for (const [tag, only] of [['ja-JP', 'Small (padrão)'], ['ar-SA', 'Árabe (padrão)']]) {
        language.value = tag;
        language.dispatch('change');
        assert.deepEqual(recognizer().children.map((o) => o.textContent), [only]);
        assert.equal(recognizer().disabled, true);
    }
});

test('in the manual mode Russian keeps PP-OCRv5 Cyrillic by default and can try Tiny RU; the other Cyrillic languages cannot', async () => {
    const fixture = await settingsFixture({ocrLanguage: 'ru-RU', ocrManual: true, experimentalRecognizer: ['xx-XX']});
    const {view, settings} = fixture;
    fixture.languages.resolve([{tag: 'ru-RU', name: 'Russian', rec: 'rec_cyrillic', experimental: 'rec_ru_tiny'},
        {tag: 'uk-UA', name: 'Ukrainian', rec: 'rec_cyrillic'}, {tag: 'bg-BG', name: 'Bulgarian', rec: 'rec_cyrillic'}]);
    fixture.status.resolve({session: 'windows'});
    await fixture.initialization;
    const recognizer = () => descendants(view.document.getElementById('recognizer-row')).find((node) => node.tagName === 'select');
    assert.deepEqual(recognizer().children.map((o) => o.textContent), ['Cirílico (padrão)', 'Tiny RU (experimental)']);
    assert.equal(recognizer().value, 'rec_cyrillic', 'PP-OCRv5 Cyrillic is the default');
    assert.notEqual(recognizer().disabled, true);
    recognizer().value = 'rec_ru_tiny';
    recognizer().dispatch('change');
    assert.equal(JSON.stringify(settings.experimentalRecognizer), '["xx-XX","ru-RU"]');
    recognizer().value = 'rec_cyrillic';
    recognizer().dispatch('change');
    assert.equal(JSON.stringify(settings.experimentalRecognizer), '["xx-XX"]', 'back to Cyrillic; other entries kept');
    recognizer().value = 'rec_ru_tiny';
    recognizer().dispatch('change');
    const language = view.document.getElementById('ocr-language');
    for (const tag of ['uk-UA', 'bg-BG']) {
        language.value = tag;
        language.dispatch('change');
        assert.deepEqual(recognizer().children.map((o) => o.textContent), ['Cirílico (padrão)'], tag);
        assert.equal(recognizer().disabled, true);
    }
    language.value = 'ru-RU';
    language.dispatch('change');
    assert.equal(recognizer().value, 'rec_ru_tiny', 'the choice is restored');
});

test('the manual OCR mode picks the detector and puts each model on the CPU or the GPU; off, nothing new shows', async () => {
    const fixture = await settingsFixture({ocrLanguage: 'ru-RU', useGpu: true});
    const {root, settings} = fixture;
    fixture.languages.resolve([{tag: 'ru-RU', name: 'Russian', rec: 'rec_cyrillic'}, {tag: 'pt-BR', name: 'Portuguese (Brazil)', rec: 'rec_tiny'}]);
    fixture.status.resolve({session: 'windows'});
    await fixture.initialization;
    const row = (title) => descendants(root).find((node) => node.className === 'card-row' && node.textContent.includes(title));
    const switches = (r) => descendants(r).filter((node) => node.className === 'segmented');
    assert.equal(row('Modelo de detecção'), undefined, 'off by default: the automatic choice');
    assert(row('Usar a placa de vídeo'), 'off: the one GPU switch');
    const recognizerRow = () => fixture.view.document.getElementById('recognizer-row');
    assert.equal(recognizerRow().children.length, 0, 'off: no recognizer choice either');
    const at = (title) => descendants(root).filter((node) => node.className === 'card-row').findIndex((node) => node.textContent.includes(title));
    assert.equal(at('Escolher modelos manualmente') + 1, at('Estado do motor'), 'right above the engine state');
    const manual = descendants(row('Escolher modelos manualmente')).find((node) => node.tagName === 'input');
    manual.checked = true;
    manual.dispatch('change');
    assert.deepEqual([settings.ocrManual, settings.manualDet, settings.manualDetGpu, settings.manualRecGpu], [true, 'det_small', true, true],
        'the first time, from the automatic choice');
    assert.equal(row('Usar a placa de vídeo'), undefined, 'each model has its own switch instead');
    const detector = row('Modelo de detecção');
    const detSelect = descendants(detector).find((node) => node.tagName === 'select');
    assert.deepEqual(detSelect.children.map((o) => o.textContent), ['Tiny', 'Small']);
    assert.equal(detSelect.value, 'det_small');
    detSelect.value = 'det_tiny';
    detSelect.dispatch('change');
    assert.equal(settings.manualDet, 'det_tiny');
    const [detDevice] = switches(detector);
    detDevice.children[0].dispatch('click');
    assert.equal(settings.manualDetGpu, false, 'the detector on the CPU');
    assert(at('Modelo de detecção') < at('Estado do motor'));
    assert(at('Modelo de reconhecimento') < at('Estado do motor'));
    const recognizer = recognizerRow();
    assert.deepEqual(descendants(recognizer).find((node) => node.tagName === 'select').children.map((o) => o.textContent), ['Cirílico (padrão)'],
        'Cyrillic offers only its own model');
    const [recDevice] = switches(recognizer);
    assert(recDevice, 'with a CPU | GPU switch');
    recDevice.children[0].dispatch('click');
    assert.equal(settings.manualRecGpu, false);
    recDevice.children[1].dispatch('click');
    assert.equal(settings.manualRecGpu, true, 'the recognizer back on the GPU');
});

test('the capture region offers only Automático and Personalizada, and custom dimensions save on change while OCR is pending', async () => {
    const fixture = await settingsFixture({ocrScope: 'region'});
    const {view, settings} = fixture;
    const width = view.document.getElementById('cap-w');
    const height = view.document.getElementById('cap-h');
    const preset = view.document.getElementById('capture-preset');
    const sizeRow = descendants(fixture.root).find((node) => node.className === 'card-row' && node.textContent.includes('Tamanho da captura'));
    assert.equal(JSON.stringify(descendants(preset).filter((node) => node.tagName === 'option').map((node) => node.value)), JSON.stringify(['auto', 'custom']));
    assert.equal(preset.value, 'custom', 'a fixed region of the default size is Personalizada');
    assert.equal(sizeRow.hidden, false, 'Personalizada shows the size fields');
    width.value = '1200';
    width.dispatch('input');
    assert.equal(settings.captureWidth, 900, 'partial typing must not save dimensions');
    assert.equal(fixture.saves(), 0);
    width.dispatch('change');
    height.value = '480';
    height.dispatch('change');
    assert.equal(settings.captureWidth, 1200);
    assert.equal(settings.captureHeight, 480);
    assert.equal(preset.value, 'custom');
    assert.equal(fixture.saves(), 2);
    await fixture.finish();
    assert.equal(view.document.getElementById('cap-w'), width, 'OCR status updates must preserve the editable region controls');
});

test('Automático is a capture choice that hides the size fields and leaves the size alone', async () => {
    const fixture = await settingsFixture({ocrScope: 'region', captureAuto: true, captureWidth: 960, captureHeight: 540});
    const {view, settings} = fixture;
    const preset = view.document.getElementById('capture-preset');
    const sizeRow = descendants(fixture.root).find((node) => node.className === 'card-row' && node.textContent.includes('Tamanho da captura'));
    assert.equal(preset.value, 'auto');
    assert.equal(sizeRow.hidden, true);
    preset.value = 'custom';
    preset.dispatch('change');
    assert.equal(settings.captureAuto, false, 'a fixed region turns the automatic capture off');
    assert.equal(sizeRow.hidden, false);
    assert.equal(fixture.saves(), 1);
    preset.value = 'auto';
    preset.dispatch('change');
    assert.equal(settings.captureAuto, true);
    assert.equal(sizeRow.hidden, true);
    assert.equal(settings.captureWidth, 960, 'the fixed size is kept for when the region is chosen again');
    assert.equal(fixture.saves(), 2);
    await fixture.finish();
});

test('invalid capture dimensions never persist and valid boundary values remain editable', async () => {
    const fixture = await settingsFixture({ocrScope: 'region', captureWidth: 810, captureHeight: 220});
    const {view, settings} = fixture;
    const width = view.document.getElementById('cap-w');
    const height = view.document.getElementById('cap-h');
    assert.equal(view.document.getElementById('capture-preset').value, 'custom');
    for (const invalid of ['', 'NaN', 'Infinity', '0', '-100', '199', '4097', '200.5']) {
        width.value = invalid;
        width.dispatch('change');
        assert.equal(settings.captureWidth, 810);
        assert.equal(width.value, '810');
    }
    for (const invalid of ['', 'NaN', 'Infinity', '0', '59', '4097', '60.5']) {
        height.value = invalid;
        height.dispatch('change');
        assert.equal(settings.captureHeight, 220);
        assert.equal(height.value, '220');
    }
    assert.equal(fixture.saves(), 0);
    width.value = '200';
    width.dispatch('change');
    height.value = '60';
    height.dispatch('change');
    assert.equal(settings.captureWidth, 200);
    assert.equal(settings.captureHeight, 60);
    width.value = '4096';
    width.dispatch('change');
    height.value = '4096';
    height.dispatch('change');
    assert.equal(settings.captureWidth, 4096);
    assert.equal(settings.captureHeight, 4096);
    assert.equal(fixture.saves(), 4);
    await fixture.finish();
});

test('typing a size keeps Personalizada, and choosing it again preserves the current capture size', async () => {
    const fixture = await settingsFixture({ocrScope: 'region'});
    const {view, settings} = fixture;
    const preset = view.document.getElementById('capture-preset');
    preset.value = 'custom';
    preset.dispatch('change');
    assert.equal(settings.captureWidth, 900);
    assert.equal(settings.captureHeight, 260);
    assert.equal(fixture.saves(), 0);
    const width = view.document.getElementById('cap-w');
    const height = view.document.getElementById('cap-h');
    width.value = '960';
    width.dispatch('change');
    height.value = '540';
    height.dispatch('change');
    assert.equal(preset.value, 'custom', 'the fields stay open while they are edited');
    assert.equal(settings.captureWidth, 960);
    assert.equal(fixture.saves(), 2);
    await fixture.finish();
});

test('whole-screen OCR engine is chosen separately and defaults to the pointer engine', async () => {
    const fixture = await settingsFixture({ocrEngine: 'paddle', screenOcrEngine: null});
    const {root, settings} = fixture;
    const engineControl = (title) => {
        const row = descendants(root).find((node) => node.className === 'card-row' && node.textContent.includes(title));
        return descendants(row).find((node) => node.tagName === 'select');
    };
    assert.equal(engineControl('OCR da tela inteira').value, 'paddle');
    engineControl('OCR da tela inteira').value = 'lens';
    engineControl('OCR da tela inteira').dispatch('change');
    assert.equal(settings.screenOcrEngine, 'lens');
    assert.equal(settings.ocrEngine, 'paddle');
    assert(descendants(root).some((node) => node.className === 'card-row' && node.textContent.includes('Chave da API do Google Lens')));
    await fixture.finish();
});

test('MeikiOCR is offered for Japanese only, ScreenAI always, and their files rows show while they are chosen', async () => {
    const offered = (root, title) => {
        const row = descendants(root).find((node) => node.className === 'card-row' && node.textContent.includes(title));
        return descendants(row).filter((node) => node.tagName === 'option').map((node) => node.value);
    };
    const english = await settingsFixture({ocrLanguage: 'en-US', ocrEngine: 'paddle'});
    assert.deepEqual(offered(english.root, 'OCR em volta do cursor'), ['paddle', 'screenai', 'lens']);
    await english.finish();

    const fixture = await settingsFixture({ocrLanguage: 'ja-JP', ocrEngine: 'paddle', screenOcrEngine: null});
    const {root, settings} = fixture;
    const modelsRow = () => descendants(root).some((node) => node.className === 'card-row' && node.textContent.includes('Modelos do MeikiOCR'));
    assert.deepEqual(offered(root, 'OCR em volta do cursor'), ['paddle', 'meiki', 'screenai', 'lens']);
    assert(!modelsRow());
    const control = () => descendants(descendants(root).find((node) => node.className === 'card-row' && node.textContent.includes('OCR em volta do cursor')))
        .find((node) => node.tagName === 'select');
    control().value = 'meiki';
    control().dispatch('change');
    assert.equal(settings.ocrEngine, 'meiki');
    assert(modelsRow(), 'the models row appears with MeikiOCR');
    const screenControl = () => descendants(descendants(root).find((node) => node.className === 'card-row' && node.textContent.includes('OCR da tela inteira')))
        .find((node) => node.tagName === 'select');
    screenControl().value = 'screenai';
    screenControl().dispatch('change');
    assert.equal(settings.screenOcrEngine, 'screenai');
    assert(modelsRow(), 'MeikiOCR still reads around the pointer');
    assert(descendants(root).some((node) => node.className === 'card-row' && node.textContent.includes('Arquivos do Chrome ScreenAI')));
    await fixture.finish();
});

test('the capture region is always editable and there is no scope selector', async () => {
    const fixture = await settingsFixture({captureWidth: 1000, captureHeight: 300});
    const {view, root, settings} = fixture;
    assert(!descendants(root).some((node) => node.className === 'card-row' && node.textContent.includes('Área lida')));
    assert.equal(view.document.getElementById('cap-w').value, '1000');
    assert.equal(view.document.getElementById('cap-h').value, '300');
    assert.equal(view.document.getElementById('capture-preset').value, 'custom');
    assert.equal(settings.captureWidth, 1000);
    assert.equal(settings.captureHeight, 300);
    assert.equal(fixture.saves(), 0);
    await fixture.finish();
});

async function mainFixture(rejectSettings = false) {
    const view = dom();
    const saveState = view.add('save-state');
    view.add('settings-col');
    const tabs = ['words', 'dicts', 'settings'].map((name) => { const tab = view.add(`tab-${name}`); tab.hidden = true; return tab; });
    const buttons = ['words', 'dicts', 'settings'].map((name) => { const button = new Element('button'); button.dataset.tab = name; return button; });
    view.document.querySelectorAll = (selector) => selector === '.tab' ? tabs : buttons;
    const storage = new Map([['tab', 'stale-tab-name']]);
    const settings = deferred();
    const initialization = deferred();
    const subscriptions = [];
    await load('src/main/main.js', {
        document: view.document,
        localStorage: {getItem: (key) => storage.get(key), setItem: (key, value) => storage.set(key, value)},
    }, {
        '@tauri-apps/api/event': {listen: async (name) => { subscriptions.push(name); return () => {}; }},
        [file('src/lib/api.js')]: {api: {getSettings: async () => ({profiles: []})}},
        [file('src/lib/icons.js')]: {hydrateIcons: () => {}},
        [file('src/main/store.js')]: { saveNow: async () => {},loadStore: () => settings.promise, store: {settings: {profiles: []}}},
        [file('src/main/words-tab.js')]: {initWordsTab: async () => {}, refreshWords: async () => {}},
        [file('src/main/dicts-tab.js')]: {initDictsTab: async () => { throw new Error('dictionary read failed'); }, refreshDicts: async () => {}},
        [file('src/main/settings-tab.js')]: {initSettingsTab: () => initialization.promise, refreshStatus: async () => {}},
    });
    assert.equal(tabs[2].hidden, false, 'invalid saved tab must fall back to settings');
    buttons[0].dispatch('click');
    assert.equal(tabs[0].hidden, false, 'navigation must work while get_settings is pending');
    if (rejectSettings) { settings.reject(new Error('settings read failed')); } else { settings.resolve(); }
    await flush();
    return {subscriptions, saveState, initialization};
}

test('subscriptions attach while status is pending and one tab failure is shown', async () => {
    const fixture = await mainFixture();
    assert.deepEqual(fixture.subscriptions, ['engine-status', 'dictionaries-changed', 'words-changed', 'navigate-settings', 'settings-changed']);
    assert(fixture.saveState.textContent.includes('dictionary read failed'));
    fixture.initialization.resolve();
    await flush();
});

test('settings loading failure is visible instead of an unhandled rejection', async () => {
    const fixture = await mainFixture(true);
    assert(fixture.saveState.textContent.includes('settings read failed'));
    assert.equal(fixture.saveState.attributes.role, 'alert');
});

async function ankiPanelFixture(createConnection, overrides = {}) {
    const view = dom();
    const root = view.add('anki-settings');
    let saves = 0;
    const anki = {enable: true, server: 'http://127.0.0.1:8765', apiKey: '', tags: ['lookitan'],
        cardFormats: [{deck: '', model: '', fields: {}}], ...overrides};
    const panel = await load('src/main/anki-panel.js', {document: view.document}, {
        [file('src/lib/api.js')]: {escapeHtml: String},
        [file('src/lib/icons.js')]: {icon: () => ''},
        [file('src/main/store.js')]: { saveNow: async () => {},store: {anki}, save: () => { saves++; }},
        [file('src/lib/anki/notes.js')]: {createConnection, fieldValue: (field) => field.value, MARKERS: ['expression', 'glossary'],
            dictionaryMarkers: () => [], glossaryDictionaryNames: () => [],
            suggestMarker: (_name, index) => index === 0 ? '{expression}' : '{glossary}'},
    });
    const initialization = panel.renderAnkiSection(root);
    const rowFor = (title) => descendants(root).find((node) => node.className === 'card-row' &&
        node.children.find((child) => child.className === 'txt')?.children[0]?.textContent === title);
    return {view, root, anki, panel, initialization, rowFor, saves: () => saves};
}

test('Anki controls render before connecting and stale responses cannot overwrite an edited server', async () => {
    const version = deferred();
    const fixture = await ankiPanelFixture(() => ({getVersion: () => version.promise,
        getDeckNames: async () => ['Study'], getModelNames: async () => ['Basic']}));
    assert(fixture.root.children.length > 1, 'Anki connection must not block its settings controls');
    assert(fixture.root.textContent.includes('escolha o baralho e o tipo de nota'));
    const server = descendants(fixture.rowFor('Conexão')).find((node) => node.tagName === 'input');
    server.value = 'http://127.0.0.1:8766';
    server.dispatch('input');
    assert.equal(fixture.anki.server, 'http://127.0.0.1:8766');
    assert.equal(fixture.saves(), 1);
    version.resolve(6);
    await fixture.initialization;
    assert.equal(descendants(fixture.rowFor('Conexão')).find((node) => node.tagName === 'input'), server);
    assert(fixture.root.textContent.includes('Clique em Verificar'));
    assert.equal(fixture.root.textContent.includes('Conectado ao AnkiConnect'), false);
});

test('the Anki section collapses below its on/off switch and expands again', async () => {
    const fixture = await ankiPanelFixture(() => ({getVersion: async () => 6, getDeckNames: async () => ['Study'], getModelNames: async () => ['Basic']}));
    await fixture.initialization;
    const head = fixture.rowFor('Integração com o Anki');
    const button = descendants(head).find((node) => node.tagName === 'button' && node.className.includes('collapse'));
    assert(button, 'the switch row has a collapse button');
    assert.notEqual(fixture.rowFor('Baralho').hidden, true);
    button.dispatch('click');
    assert.equal(fixture.rowFor('Baralho').hidden, true);
    assert.equal(fixture.rowFor('Conexão').hidden, true);
    assert.notEqual(head.hidden, true, 'the switch stays visible');
    button.dispatch('click');
    assert.equal(fixture.rowFor('Baralho').hidden, false);
});

test('Anki connection errors stay visible alongside usable setup controls', async () => {
    const fixture = await ankiPanelFixture(() => ({getVersion: async () => { throw new Error('URL denied by HTTP scope'); }}));
    await fixture.initialization;
    assert(fixture.root.textContent.includes('URL denied by HTTP scope'));
    assert(fixture.rowFor('Baralho'));
    assert(fixture.rowFor('Tipo de nota'));
    assert.equal(fixture.saves(), 0);
});

test('Anki field mappings preserve edits and cannot reuse fields from a previously selected model', async () => {
    const nextVersion = deferred();
    let connections = 0;
    const fixture = await ankiPanelFixture(() => {
        connections++;
        return {getVersion: () => connections >= 3 ? nextVersion.promise : Promise.resolve(6),
            getDeckNames: async () => ['Study'], getModelNames: async () => ['Basic', 'Other'],
            getModelFieldNames: async (model) => model === 'Basic' ? ['Front', 'Back'] : ['Term', 'Meaning']};
    }, {cardFormats: [{deck: 'Study', model: 'Basic', fields: {Front: {value: '<b>{expression}</b>'}}}]});
    await fixture.initialization;
    const format = fixture.anki.cardFormats[0];
    assert.equal(format.fields.Front.value, '<b>{expression}</b>');
    assert.equal(format.fields.Back.value, '{glossary}');
    assert.equal(fixture.saves(), 1);
    await fixture.panel.renderAnkiSection(fixture.root);
    assert.equal(fixture.saves(), 1, 'checking the connection must not save unchanged mappings');
    const model = descendants(fixture.rowFor('Tipo de nota')).find((node) => node.tagName === 'select');
    model.value = 'Other';
    model.dispatch('change');
    assert.equal(format.model, 'Other');
    assert.deepEqual(Object.keys(format.fields), [], 'old model fields must not fill the new model during connection');
    nextVersion.resolve(6);
    await flush();
    assert.deepEqual(Object.keys(format.fields), ['Term', 'Meaning']);
    assert.equal(format.fields.Term.value, '{expression}');
    assert.equal(fixture.saves(), 3);
});

test('saved word Anki errors stay visible and the add button can be retried', async () => {
    const view = dom();
    for (const id of ['words-list', 'words-detail', 'words-search', 'words-sort', 'anki-sync', 'words-export', 'save-state']) { view.add(id); }
    view.document.getElementById('words-sort').value = 'newest';
    let listRequests = 0;
    const words = await load('src/main/words-tab.js', {document: view.document}, {
        '@tauri-apps/plugin-dialog': {save: async () => null},
        '@tauri-apps/api/core': {invoke: async () => null},
        [file('src/lib/api.js')]: {escapeHtml: String, api: {listWords: async () => { listRequests++; return [{id: 'word-1', word: 'hello', createdAt: 1}]; }}},
        [file('src/lib/icons.js')]: {icon: () => ''},
        [file('src/lib/cards.js')]: {noteContextFromCard: (card) => ({word: card.word}), toCsv: () => '', toQuizlet: () => ''},
        [file('src/main/store.js')]: { saveNow: async () => {},store: {anki: {enable: true}}},
        [file('src/lib/anki/notes.js')]: {isFormatReady: () => true, buildNote: async () => ({fields: {Front: 'hello'}}), glossaryDictionaryNames: () => [],
            createConnection: () => ({addNote: async () => { throw new Error('Anki unavailable'); }})},
    });
    await words.initWordsTab();
    await flush();
    const detail = view.document.getElementById('words-detail');
    const button = descendants(detail).find((node) => node.tagName === 'button' && node.className === 'primary');
    button.dispatch('click');
    await flush();
    assert(detail.textContent.includes('Anki unavailable'));
    assert.equal(button.disabled, false);
    assert.equal(listRequests, 1, 'failed addition must not erase its message by refreshing the word list');
});

test('a saved word is edited in place, field by field, as in Lookupper', async () => {
    const view = dom();
    for (const id of ['words-list', 'words-detail', 'words-search', 'words-sort', 'anki-sync', 'words-export', 'save-state']) { view.add(id); }
    view.document.getElementById('words-sort').value = 'newest';
    const saved = [];
    let stored = {id: 'word-1', word: 'optimized', dictionaryForm: 'optimise', partOfSpeech: ['verb'], cefr: 'C1', createdAt: 1, updatedAt: 1};
    const words = await load('src/main/words-tab.js', {document: view.document}, {
        '@tauri-apps/plugin-dialog': {save: async () => null},
        '@tauri-apps/api/core': {invoke: async () => null},
        [file('src/lib/api.js')]: {escapeHtml: String, api: {
            listWords: async () => [{...stored}],
            saveWord: async (card) => { saved.push(JSON.parse(JSON.stringify(card))); stored = {...card, updatedAt: card.updatedAt + 1}; return {...stored}; },
        }},
        [file('src/lib/icons.js')]: {icon: () => ''},
        [file('src/lib/cards.js')]: {noteContextFromCard: (card) => ({word: card.word}), toCsv: () => '', toQuizlet: () => ''},
        [file('src/main/store.js')]: { saveNow: async () => {},store: {anki: {enable: true}}},
        [file('src/lib/anki/notes.js')]: {isFormatReady: () => true, buildNote: async () => ({}), glossaryDictionaryNames: () => [], createConnection: () => ({})},
    });
    await words.initWordsTab();
    await flush();
    const detail = view.document.getElementById('words-detail');
    const field = (key) => descendants(detail).find((node) => node.attributes['data-field'] === key);
    assert.equal(field('dictionaryForm').value, 'optimise');
    assert.equal(field('cefr').value, 'C1');
    const form = field('dictionaryForm');
    form.value = ' optimize ';
    form.dispatch('change');
    await flush();
    assert.equal(saved.length, 1);
    assert.equal(saved[0].dictionaryForm, 'optimize', 'trimmed');
    assert.equal(field('dictionaryForm'), form, 'the form is not redrawn while it is being edited');
    field('partOfSpeech').value = 'verb, adjective,';
    field('partOfSpeech').dispatch('change');
    await flush();
    assert.deepEqual(saved[1].partOfSpeech, ['verb', 'adjective']);
    field('cefr').dispatch('change');
    await flush();
    assert.equal(saved.length, 2, 'an unchanged field is not saved again');
    // The "words-changed" the save sends reloads the list, still without redrawing the form.
    await words.refreshWords();
    assert.equal(field('dictionaryForm'), form);
    assert(detail.textContent.includes('Salvo.'));
});

async function audioPanelFixture(audio) {
    const view = dom();
    const root = view.add('audio-settings');
    let saves = 0;
    const settings = {ocrLanguage: 'ja-JP', audio};
    const panel = await load('src/main/audio-panel.js', {document: view.document}, {
        '@tauri-apps/api/core': {invoke: async (command) => (command === 'audio_db_sources' ? ['nhk16', 'forvo']
            : command === 'edge_tts_voices' ? [{id: 'en-US-AriaNeural', locale: 'en-US', gender: 'Female'}, {id: 'ja-JP-KeitaNeural', locale: 'ja-JP', gender: 'Male'},
                {id: 'ja-JP-NanamiNeural', locale: 'ja-JP', gender: 'Female'}] : [])},
        '@tauri-apps/plugin-dialog': {open: async () => null},
        [file('src/lib/api.js')]: {escapeHtml: String},
        [file('src/lib/icons.js')]: {icon: () => ''},
        [file('src/main/store.js')]: { saveNow: async () => {},store: {settings}, save: () => { saves++; }},
    });
    panel.renderAudioSection(root);
    const rows = (className) => descendants(root).filter((node) => node.className === className);
    const button = (row, title) => descendants(row).find((node) => node.tagName === 'button' && node.attributes.title === title);
    const types = () => JSON.parse(JSON.stringify(settings.audio.sources.map((s) => s.type)));
    return {root, settings, rows, button, types, saves: () => saves};
}

test('audio sources move with the up and down arrows, as in Yomitan, and the database sources too', async () => {
    const fixture = await audioPanelFixture({sources: [{type: 'tts'}, {type: 'jpod101'}, {type: 'local-db', path: 'a.db'}]});
    let rows = fixture.rows('audio-source');
    assert.equal(rows.length, 3);
    assert.equal(fixture.button(rows[0], 'Subir').disabled, true, 'the first cannot go up');
    assert.equal(fixture.button(rows[2], 'Descer').disabled, true, 'the last cannot go down');
    assert(!descendants(fixture.root).some((node) => node.className === 'grip'), 'no drag handles');
    const before = fixture.saves();
    fixture.button(rows[0], 'Descer').dispatch('click');
    assert.deepEqual(fixture.types(), ['jpod101', 'tts', 'local-db']);
    assert.equal(fixture.saves(), before + 1);
    rows = fixture.rows('audio-source');
    fixture.button(rows[2], 'Subir').dispatch('click');
    assert.deepEqual(fixture.types(), ['jpod101', 'local-db', 'tts']);
    assert(fixture.root.textContent.includes('Ordem: JapanesePod101 › Banco de áudio local (.db) › Voz do sistema'));

    await flush();
    const subs = fixture.rows('audio-sub');
    assert.deepEqual(subs.map((row) => row.textContent.includes('NHK16')), [true, false], 'the file sources, in the default order');
    fixture.button(subs[1], 'Subir').dispatch('click');
    assert.equal(JSON.stringify(fixture.settings.audio.sources[1].subsources.map((s) => s.name)), JSON.stringify(['forvo', 'nhk16']));
});

test('every Yomitan audio source can be chosen, and a new custom URL source shows its URL field', async () => {
    const fixture = await audioPanelFixture({sources: [{type: 'tts'}]});
    const select = descendants(fixture.root).find((node) => node.attributes['data-setting'] === 'audio.sources.0.type');
    assert.deepEqual(select.children.map((o) => o.attributes.value),
        ['local-db', 'jpod101', 'language-pod-101', 'jisho', 'lingua-libre', 'wiktionary', 'custom', 'custom-json', 'edge-tts', 'tts']);
    const add = descendants(fixture.root).find((node) => node.tagName === 'button' && String(node.innerHTML).includes('Adicionar fonte'));
    add.dispatch('click');
    assert.deepEqual(fixture.types(), ['tts', 'local-db']);
    const type = descendants(fixture.root).find((node) => node.attributes['data-setting'] === 'audio.sources.1.type');
    type.value = 'custom';
    type.dispatch('change');
    assert.deepEqual(fixture.types(), ['tts', 'custom']);
    assert(descendants(fixture.root).some((node) => node.attributes['data-setting'] === 'audio.sources.1.url'), 'the URL field appears');
});

test('the audio section collapses below its first row and expands again', async () => {
    const fixture = await audioPanelFixture({sources: [{type: 'tts'}]});
    const head = fixture.root.children[0];
    const button = descendants(head).find((node) => node.tagName === 'button' && node.className.includes('collapse'));
    assert(button, 'the first row has a collapse button');
    button.dispatch('click');
    assert(fixture.root.children.slice(1).every((node) => node.hidden === true));
    assert.notEqual(head.hidden, true);
    button.dispatch('click');
    assert(fixture.root.children.slice(1).every((node) => node.hidden === false));
});

test('an Edge source and the context voice list every Edge voice, grouped by language', async () => {
    const fixture = await audioPanelFixture({sources: [{type: 'edge-tts'}], context: {type: 'edge-tts', voice: ''}});
    await flush();
    const voiceLists = descendants(fixture.root).filter((node) => node.tagName === 'select' && node.attributes['aria-label'] === 'Voz do Edge');
    assert.equal(voiceLists.length, 2, 'one for the context, one for the source');
    const groups = voiceLists[0].children.filter((node) => node.tagName === 'optgroup');
    assert.equal(voiceLists[0].children[0].attributes.value, '', 'automatic first');
    assert.equal(JSON.stringify(groups.map((g) => g.children.map((o) => o.attributes.value))),
        JSON.stringify([['en-US-AriaNeural'], ['ja-JP-KeitaNeural', 'ja-JP-NanamiNeural']]), 'every voice, not only the OCR language, one group per language');
    // The context row comes before the list of sources.
    voiceLists[0].value = 'ja-JP-NanamiNeural';
    voiceLists[0].dispatch('change');
    assert.equal(fixture.settings.audio.context.voice, 'ja-JP-NanamiNeural');
    voiceLists[1].value = 'ja-JP-KeitaNeural';
    voiceLists[1].dispatch('change');
    assert.equal(fixture.settings.audio.sources[0].voice, 'ja-JP-KeitaNeural');
});

test('settings saved before the context voice existed still open', async () => {
    const fixture = await audioPanelFixture({normalized: true, autoPlay: false, others: 'menu', sources: [{type: 'tts', enabled: true, path: '', url: '', subsources: []}]});
    assert.equal(fixture.settings.audio.context.type, 'tts');
    assert(fixture.root.textContent.includes('Voz para ler o contexto'));
});

test('the Edge voice list also loads when voices come with Microsoft field names', async () => {
    const view = dom();
    const root = view.add('audio-settings');
    const settings = {ocrLanguage: 'ja-JP', audio: {sources: [{type: 'edge-tts'}]}};
    const panel = await load('src/main/audio-panel.js', {document: view.document}, {
        '@tauri-apps/api/core': {invoke: async (command) => (command === 'edge_tts_voices'
            ? [{ShortName: 'ja-JP-NanamiNeural', Locale: 'ja-JP', Gender: 'Female'}] : [])},
        '@tauri-apps/plugin-dialog': {open: async () => null},
        [file('src/lib/api.js')]: {escapeHtml: String},
        [file('src/lib/icons.js')]: {icon: () => ''},
        [file('src/main/store.js')]: { saveNow: async () => {},store: {settings}, save: () => {}},
    });
    panel.renderAudioSection(root);
    await flush();
    assert(!root.textContent.includes('Carregando as vozes'), 'the list does not stay loading');
    const list = descendants(root).find((node) => node.tagName === 'select' && node.attributes['aria-label'] === 'Voz do Edge');
    assert(descendants(list).some((node) => node.attributes.value === 'ja-JP-NanamiNeural'));
});

test('the audio test result sits under its description, not beside the button', async () => {
    const fixture = await audioPanelFixture({sources: [{type: 'tts'}]});
    const result = descendants(fixture.root).find((node) => String(node.className).includes('audio-result'));
    assert(result, 'a result line');
    const row = fixture.root.children.at(-1);
    const txt = row.children.find((node) => node.className === 'txt');
    assert(txt.children.includes(result), 'inside the text column');
});
