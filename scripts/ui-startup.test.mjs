// Run with: node --experimental-vm-modules --test scripts/ui-startup.test.mjs
// Exercise the actual UI modules with a small DOM and delayed Tauri responses.
import assert from 'node:assert/strict';
import {readFile} from 'node:fs/promises';
import {resolve} from 'node:path';
import test from 'node:test';
import {createContext, SourceTextModule, SyntheticModule} from 'node:vm';

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

test('region geometry appears when selected and screen mode preserves the custom dimensions', async () => {
    const fixture = await settingsFixture({captureWidth: 1000, captureHeight: 300});
    const {view, root, settings} = fixture;
    const scopeControl = () => {
        const row = descendants(root).find((node) => node.className === 'card-row' && node.textContent.includes('Área lida'));
        return descendants(row).find((node) => node.tagName === 'select');
    };
    assert.equal(view.document.getElementById('cap-w'), undefined);
    scopeControl().value = 'region';
    scopeControl().dispatch('change');
    assert.equal(settings.ocrScope, 'region');
    assert.equal(view.document.getElementById('cap-w').value, '1000');
    assert.equal(view.document.getElementById('cap-h').value, '300');
    assert.equal(view.document.getElementById('capture-preset').value, 'custom');
    scopeControl().value = 'screen';
    scopeControl().dispatch('change');
    assert.equal(view.document.getElementById('cap-w'), undefined);
    assert.equal(settings.captureWidth, 1000);
    assert.equal(settings.captureHeight, 300);
    assert.equal(fixture.saves(), 2);
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
    assert.deepEqual(fixture.subscriptions, ['engine-status', 'dictionaries-changed', 'words-changed', 'settings-changed']);
    assert(fixture.saveState.textContent.includes('dictionary read failed'));
    fixture.initialization.resolve();
    await flush();
});

test('settings loading failure is visible instead of an unhandled rejection', async () => {
    const fixture = await mainFixture(true);
    assert(fixture.saveState.textContent.includes('settings read failed'));
    assert.equal(fixture.saveState.attributes.role, 'alert');
});
