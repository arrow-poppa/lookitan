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
    async function get(id) {
        if (modules.has(id)) { return modules.get(id); }
        const exports = stubs[id];
        const module = exports ? new SyntheticModule(Object.keys(exports), function () {
            for (const [name, value] of Object.entries(exports)) { this.setExport(name, value); }
        }, {context, identifier: id}) : new SourceTextModule(await readFile(id, 'utf8'), {context, identifier: id});
        modules.set(id, module);
        await module.link((specifier) => get(specifier.startsWith('.') ? resolve(id, '..', specifier) : specifier));
        return module;
    }
    const module = await get(file(path));
    await module.evaluate();
    return module.namespace;
}

async function settingsFixture(overrides = {}) {
    const view = dom();
    const root = view.add('settings-col');
    const languages = deferred();
    const status = deferred();
    let saves = 0;
    const settings = {ocrLanguage: 'en-US', appMode: 'popup', profiles: [], mouseHotkey: {button: 'none'}, useGpu: true,
        ocrScope: 'screen', captureWidth: 900, captureHeight: 260, ...overrides};
    const ui = await load('src/main/settings-tab.js', {document: view.document}, {
        '@tauri-apps/api/app': {getVersion: async () => 'test'},
        [file('src/lib/api.js')]: {api: {ocrLanguages: () => languages.promise, engineStatus: () => status.promise}, escapeHtml: String},
        [file('src/lib/icons.js')]: {icon: () => ''},
        [file('src/main/anki-panel.js')]: {renderAnkiSection: async () => {}},
        [file('src/main/store.js')]: {store: {settings, anki: {}}, save: () => { saves += 1; }},
    });
    const initialization = ui.initSettingsTab(root);
    return {view, root, settings, languages, status, initialization, saves: () => saves, finish: async () => {
        languages.resolve([{tag: 'en-US', name: 'English'}]);
        status.resolve({session: 'windows'});
        await initialization;
    }};
}

async function dictionaryFixture(overrides = {}) {
    const view = dom();
    view.add('add-dict');
    const root = view.add('dict-detail');
    const list = view.add('dict-list');
    const ai = normalizeAiOptions(overrides);
    const settings = {definitionLanguage: 'en-US', translationLanguage: 'pt-BR', hideWordTranslation: false};
    let saves = 0;
    const ui = await load('src/main/dicts-tab.js', {document: view.document}, {
        '@tauri-apps/plugin-dialog': {open: async () => null},
        [file('src/lib/api.js')]: {api: {listDictionaries: async () => [
            {id: 'ai-word', kind: 'ai-word', enabled: true}, {id: 'ai-context', kind: 'ai-context', enabled: true},
        ]}, escapeHtml: String},
        [file('src/lib/icons.js')]: {icon: () => ''},
        [file('src/lib/ai/explain.js')]: {DEFAULT_MODELS: {}, explain: async () => 'test answer'},
        [file('src/main/store.js')]: {store: {settings, ai}, save: () => { saves++; }},
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
            'streamResponse', 'cancelPendingRequests', 'unknownWordFallback', 'autoExplain', 'cardPrompt', 'wordTranslationPrompt']) {
            assert(fixture.control(key), `${key} must be visible`);
        }
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

test('context translator exposes the shared provider settings without resetting them', async () => {
    const fixture = await dictionaryFixture({provider: 'custom', custom: {thinkingIntensity: 'low', customThinkingIntensity: '2048'}});
    fixture.list.children[1].dispatch('click');
    assert.equal(fixture.control('custom.thinkingIntensity').value, 'low');
    assert.equal(fixture.control('custom.customThinkingIntensity').value, '2048');
    assert(fixture.control('contextPrompt'));
    assert.equal(fixture.saves(), 0);
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

test('custom capture dimensions save on change and presets restore the original while OCR is pending', async () => {
    const fixture = await settingsFixture({ocrScope: 'region'});
    const {view, settings} = fixture;
    const width = view.document.getElementById('cap-w');
    const height = view.document.getElementById('cap-h');
    const preset = view.document.getElementById('capture-preset');
    assert.equal(preset.value, 'original');
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
    for (const [value, expectedWidth, expectedHeight] of [['16:9', 960, 540], ['4:3', 800, 600], ['1:1', 640, 640], ['original', 900, 260]]) {
        preset.value = value;
        preset.dispatch('change');
        assert.equal(settings.captureWidth, expectedWidth);
        assert.equal(settings.captureHeight, expectedHeight);
        assert.equal(width.value, String(expectedWidth));
        assert.equal(height.value, String(expectedHeight));
    }
    assert.equal(fixture.saves(), 6);
    await fixture.finish();
    assert.equal(view.document.getElementById('cap-w'), width, 'OCR status updates must preserve the editable region controls');
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

test('manual dimensions recognize a preset and choosing custom preserves the current capture size', async () => {
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
    assert.equal(preset.value, '16:9');
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
        [file('src/main/store.js')]: {loadStore: () => settings.promise, store: {settings: {profiles: []}}},
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
        [file('src/main/store.js')]: {store: {anki}, save: () => { saves++; }},
        [file('src/lib/anki/notes.js')]: {createConnection, fieldValue: (field) => field.value, MARKERS: ['expression', 'glossary'],
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
        [file('src/main/store.js')]: {store: {anki: {enable: true}}},
        [file('src/lib/anki/notes.js')]: {isFormatReady: () => true, buildNote: async () => ({fields: {Front: 'hello'}}),
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
