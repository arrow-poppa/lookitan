import assert from 'node:assert/strict';
import {readFile} from 'node:fs/promises';
import {resolve} from 'node:path';
import test from 'node:test';
import {createHash} from 'node:crypto';
import {CARD_PROMPT} from '../src/lib/ai/word-card.js';
import * as realImages from '../src/lib/images.js';
import {createContext, SourceTextModule, SyntheticModule} from 'node:vm';

const project = resolve(import.meta.dirname, '..');
const file = (path) => resolve(project, path);
const flush = () => new Promise((done) => setImmediate(done));
function deferred() {
    let resolve;
    const promise = new Promise((done) => { resolve = done; });
    return {promise, resolve};
}
class Element {
    constructor() { this.style = {}; this.dataset = {}; this.innerHTML = ''; this.textContent = ''; this.listeners = {}; }
    classList = {toggle() {}, remove() {}, add() {}};
    children = [];
    append(...nodes) { this.children.push(...nodes); }
    replaceChildren(...nodes) { this.children = [...nodes]; }
    addEventListener(name, callback) { this.listeners[name] = callback; }
    setAttribute(name, value) { this[name] = value; }
    querySelector(selector) {
        if (!this.innerHTML.includes(selector.slice(1))) { return null; }
        this.parts ||= new Map();
        if (!this.parts.has(selector)) { this.parts.set(selector, new Element()); }
        return this.parts.get(selector);
    }
    querySelectorAll() { return []; }
    remove() { this.removed = true; }
}

async function fixture({explain = async () => '', ready = true, connection = {}, entryHtml = async () => '', dictionaries = [], images = {}, audioRows = async () => null, words = []} = {}) {
    const nodes = new Map();
    const document = {getElementById: (id) => {
        if (!nodes.has(id)) { nodes.set(id, new Element()); }
        return nodes.get(id);
    }, createElement: () => new Element(), addEventListener() {}};
    const calls = {settings: 0, hides: 0, blanks: [], events: [], notes: [], listeners: new Map(), invokes: []};
    const lookup = {payload: {word: 'run', sentence: 'I run daily.', entries: []}, keys: new Set()};
    const options = {enable: false, checkForDuplicates: false};
    const settings = {hideWordTranslation: false, saveWordsAutomatically: false, ocrLanguage: 'en-US'};
    const ai = {streamResponse: true, provider: 'custom', custom: {model: 'test'}, cardPrompt: 'card'};
    const entry = file('src/popup/main.js');
    const stubs = {
        '@tauri-apps/api/core': {invoke: async (command, args) => { calls.invokes.push([command, args]); return audioRows(command, args); }},
        '@tauri-apps/api/event': {listen: async (name, handler) => { calls.listeners.set(name, handler); return () => {}; }, emit: async (...args) => { calls.events.push(args); }},
        [file('src/lib/api.js')]: {
            api: {openSettings: async () => { calls.settings++; }, hidePopup: async () => { calls.hides++; }, webviewBlank: async (token) => { calls.blanks.push(token); }, entryHtml, listDictionaries: async () => dictionaries,
                listWords: async () => words, deleteWord: async (id) => { calls.deleted = id; }, wordImage: async () => null},
            entryUrl: () => '', escapeHtml: (value) => String(value).replace(/[&<>"']/g, (c) =>
                ({'&': '&amp;', '<': '&lt;', '>': '&gt;', '"': '&quot;', "'": '&#39;'})[c]),
            loadAllSettings: async () => ({settings, ai, anki: options}), renderMarkdown: (text) => String(text),
        },
        [file('src/lib/ai/explain.js')]: {explain, isConfigured: () => true},
        [file('src/lib/anki/notes.js')]: {
            createConnection: () => connection,
            formatUsesMarker: () => false,
            glossaryDictionaryNames: () => [],
            cleanGlossaryHtml: (html) => `clean(${html})`,
            isFormatReady: () => typeof ready === 'function' ? ready() : ready,
            buildNote: async (ctx) => { calls.notes.push(ctx); return {fields: {Word: ctx.word}, options: {}}; },
            // As notes.js does: the note type's fields first, in its order (tested in images.test.mjs).
            orderNoteFields: (note, names) => (names?.length ? {...note, fields: Object.fromEntries([
                ...names.map((n) => [n, note.fields[n] ?? '']), ...Object.entries(note.fields).filter(([n]) => !names.includes(n))])} : note),
        },
        [file('src/lib/cards.js')]: {cardFromLookup: () => { throw new Error('Unexpected local word save'); }},
        [file('src/lib/icons.js')]: {hydrateIcons() {}, icon: (name) => name},
        [file('src/lib/images.js')]: {...realImages, ...images},
    };
    const context = createContext({document, window: {addEventListener() {}}, localStorage: {getItem: () => null, setItem() {}},
        AbortController, DOMException, crypto: {subtle: {digest: async (_algorithm, bytes) => createHash('sha256').update(bytes).digest()}},
        TextEncoder, console, Intl, setTimeout, clearTimeout, requestAnimationFrame: (callback) => callback()});
    const modules = new Map();
    // The promise is kept at once, so a module imported twice at the same time (a diamond in the
    // import graph) is created and linked only once.
    function get(id) {
        if (!modules.has(id)) {
            modules.set(id, (async () => {
                const stub = stubs[id];
                return stub ? new SyntheticModule(Object.keys(stub), function () {
                    for (const [name, value] of Object.entries(stub)) { this.setExport(name, value); }
                }, {context, identifier: id}) : new SourceTextModule(await readFile(id, 'utf8') + (id === entry ?
                    '\nexport {runCard, runExplain, runContext, refreshAnkiButton, onAnkiClick, render, glossaries, refreshSavedCount, refreshAnkiNotes, openSavedCards, openAnkiNotes};\nexport function contextForTest(value) { state = value.lookup; ai = value.ai; anki = value.anki; settings = value.settings; }' : ''),
                {context, identifier: id});
            })());
        }
        return modules.get(id);
    }
    const module = await get(entry);
    await module.link((specifier, owner) => get(specifier.startsWith('.') ? resolve(owner.identifier, '..', specifier) : specifier));
    await module.evaluate();
    const popup = module.namespace;
    const change = (nextLookup = lookup) => popup.contextForTest({lookup: nextLookup, ai, anki: options, settings});
    change();
    return {popup, nodes, document, calls, lookup, options, settings, ai, change};
}

test('popup forwards the streaming option and renders a safe card before completion', async () => {
    const finish = deferred();
    let chunk;
    const view = await fixture({explain: async (options, _word, _sentence, request) => {
        assert.equal(options.streamResponse, true);
        chunk = request.onChunk;
        request.onChunk('', '{"headword":"<img src=x>","definition":"move qui');
        return finish.promise;
    }});
    const body = new Element();
    const pending = view.popup.runCard(body);
    await flush();
    assert(body.innerHTML.includes('move qui'));
    assert(body.innerHTML.includes('&lt;img src=x&gt;'));
    assert(!body.innerHTML.includes('<img src=x>'));
    assert(body.innerHTML.includes('Gerando'));
    assert.equal(view.lookup.aiCard, undefined, 'partial answers must not become exportable completed cards');
    chunk('', '{"headword":"run","definition":"move quickly","translation":"correr"}');
    finish.resolve('{"headword":"run","definition":"move quickly","translation":"correr"}');
    await pending;
    assert.equal(view.lookup.aiCard.translation, 'correr');
    assert(!body.innerHTML.includes('Gerando'));
});

test('local dictionaries and AI sections follow the same configured order', async () => {
    const dictionaries = [
        {id: 'local-a', kind: 'mdict', name: 'Dictionary A', enabled: true},
        {id: 'ai-word', kind: 'ai-word', name: 'AI', enabled: true},
        {id: 'local-b', kind: 'mdict', name: 'Dictionary B', enabled: true},
        {id: 'ai-context', kind: 'ai-context', name: 'Context', enabled: true},
    ];
    const view = await fixture({dictionaries});
    Object.assign(view.ai, {customPrompt: 'explain', contextPrompt: 'translate', autoGenerateOnLookup: false});
    await view.popup.render({word: 'run', sentence: 'I run daily.', entries: [
        {dictId: 'local-a', dictName: 'Dictionary A', entry: 1, headword: 'run'},
        {dictId: 'local-b', dictName: 'Dictionary B', entry: 2, headword: 'run'},
    ]});
    const sections = view.document.getElementById('sections').children;
    assert.equal(sections.length, 4);
    assert(sections[0].innerHTML.includes('Dictionary A'));
    assert(sections[1].innerHTML.includes('IA — palavra no contexto'));
    assert(sections[2].innerHTML.includes('Dictionary B'));
    assert(sections[3].innerHTML.includes('Tradutor de contexto'));
});

test('Anki glossary export retains all homographs once in each dictionary', async () => {
    const view = await fixture({entryHtml: async (id, entry) => `<p>${id}:${entry}</p>`});
    view.lookup.payload.entries = [
        {dictId: 'a', dictName: 'A', entry: 1}, {dictId: 'a', dictName: 'A', entry: 2},
        {dictId: 'a', dictName: 'A', entry: 1}, {dictId: 'b', dictName: 'B', entry: 3},
    ];
    const result = await view.popup.glossaries();
    assert.equal(result.length, 2);
    assert.equal(result[0].html, '<p>a:1</p><hr><p>a:2</p>');
    assert.equal(result[1].html, '<p>b:3</p>');
});

test('the word card and the explanation read the whole paragraph, the context translation only the sentence', async () => {
    const texts = [];
    const view = await fixture({explain: async (_options, _word, text) => { texts.push(text); return '{"headword":"run","definition":"move quickly"}'; }});
    const paragraph = 'Run club. I run daily. It keeps me well.';
    view.lookup.payload.context = paragraph;
    view.lookup.payload.contextAt = 12;
    Object.assign(view.ai, {customPrompt: 'explain', contextPrompt: 'translate'});
    const card = new Element();
    await view.popup.runCard(card);
    await view.popup.runExplain(new Element());
    await view.popup.runContext(new Element());
    assert.deepEqual(texts, [paragraph, paragraph, 'I run daily.']);
    assert(card.innerHTML.includes('Run club. I <mark>run</mark> daily.'), 'the word is marked where the OCR found it');
});

test('turning the word translation off drops it from the prompt and from the card', async () => {
    const answer = '{"headword":"run","definition":"move quickly","translation":"correr"}';
    const prompts = [];
    const view = await fixture({explain: async (_options, _word, _sentence, request) => { prompts.push(request.prompt); return answer; }});
    view.ai.cardPrompt = CARD_PROMPT;
    view.settings.showWordTranslation = false;
    const body = new Element();
    await view.popup.runCard(body);
    assert(!prompts[0].includes('"translation"'), 'the model is not asked for a translation');
    assert(prompts[0].includes('"definition"'));
    assert(body.innerHTML.includes('move quickly'));
    assert(!body.innerHTML.includes('correr'));
    view.settings.showWordTranslation = true;
    view.ai.wordTranslationPrompt = 'one word only, in {{translationLanguage}}';
    const shown = new Element();
    await view.popup.runCard(shown);
    assert(prompts[1].includes('"translation": one word only, in '), 'the word translation prompt fills the marker');
    assert(!prompts[1].includes('{{translationField}}'));
    assert(shown.innerHTML.includes('correr'));
});

test('the popup grows from the word like a TeachingTip and a new lookup cancels a pending close', async () => {
    const view = await fixture({explain: async () => '{}'});
    const card = view.document.getElementById('card');
    const vars = {};
    const classes = new Set();
    card.style.setProperty = (key, value) => { vars[key] = value; };
    card.classList = {add: (...names) => names.forEach((n) => classes.add(n)), remove: (...names) => names.forEach((n) => classes.delete(n)), toggle() {}};
    const open = view.calls.listeners.get('popup-open');
    open({payload: {x: 120, y: 0, relative: false}});
    assert.equal(vars['--tip-x'], '120px');
    assert.equal(vars['--tip-y'], '0px');
    assert(classes.has('tip-open'));
    view.document.getElementById('btn-close').listeners.click();
    assert(classes.has('tip-close') && !classes.has('tip-open'));
    open({payload: {x: 0.5, y: 0, relative: true}});
    await new Promise((done) => setTimeout(done, 250));
    assert.equal(view.calls.hides, 0, 'the reopened popup is not hidden by the earlier close');
    assert.equal(vars['--tip-x'], '50%');
    view.document.getElementById('btn-close').listeners.click();
    await new Promise((done) => setTimeout(done, 250));
    assert.equal(view.calls.hides, 1, 'closing hides the window after the 200 ms animation');
});

async function cancelFixture(cancelPendingRequests) {
    const requests = [];
    const view = await fixture({
        explain: async (_options, word, _sentence, request) => {
            requests.push({word, signal: request.signal});
            return new Promise(() => {});
        },
        dictionaries: [{kind: 'ai-word', enabled: true, name: 'IA'}],
    });
    Object.assign(view.ai, {autoGenerateOnLookup: true, cancelPendingRequests, contextPrompt: 'context', customPrompt: 'explain'});
    const show = async (word) => {
        void view.calls.listeners.get('lookup')({payload: {word, sentence: `I ${word} daily.`, entries: []}});
        for (let i = 0; i < 20; i++) { await flush(); }
    };
    const hide = async () => {
        view.calls.listeners.get('popup-hidden')();
        await flush();
    };
    return {requests, show, hide};
}

test('looking up another word aborts the previous AI request when cancelling is on', async () => {
    const {requests, show} = await cancelFixture(true);
    await show('run');
    await show('walk');
    assert.deepEqual(requests.map((r) => r.word), ['run', 'walk']);
    assert.equal(requests[0].signal.aborted, true, 'the request for the previous word is aborted');
    assert.equal(requests[1].signal.aborted, false);
});

test('with cancelling off the previous AI request keeps running', async () => {
    const {requests, show, hide} = await cancelFixture(false);
    await show('run');
    await show('walk');
    assert.equal(requests[0].signal.aborted, false);
    await hide();
    assert.equal(requests[1].signal.aborted, false, 'closing the pop-up keeps it running too');
});

test('closing the pop-up aborts its pending AI request when cancelling is on, like Yomitan', async () => {
    const {requests, show, hide} = await cancelFixture(true);
    await show('run');
    await hide();
    assert.equal(requests[0].signal.aborted, true);
});


test('late streaming chunks cannot overwrite a newer lookup', async () => {
    const finish = deferred();
    let chunk;
    const view = await fixture({explain: async (_options, _word, _sentence, request) => {
        chunk = request.onChunk;
        return finish.promise;
    }});
    const body = new Element();
    const pending = view.popup.runCard(body);
    await flush();
    const next = {payload: {word: 'walk'}, keys: new Set()};
    view.change(next);
    body.innerHTML = 'new lookup';
    chunk('', '{"headword":"run","definition":"old result"}');
    finish.resolve('{"headword":"run","definition":"old result"}');
    await pending;
    assert.equal(body.innerHTML, 'new lookup');
    assert.equal(next.aiCard, undefined);
});

test('Explain stays manual until enabled and starts automatically only after the full card', async () => {
    for (const automatic of [false, true]) {
        const finish = deferred();
        const calls = [];
        const view = await fixture({explain: async (_options, _word, _sentence, request) => {
            calls.push(request.prompt);
            if (request.prompt === 'card') {
                request.onChunk('', '{"headword":"run","definition":"move');
                return finish.promise;
            }
            return 'Contextual explanation';
        }});
        view.ai.autoExplain = automatic;
        view.ai.customPrompt = 'explanation';
        const body = new Element();
        const generating = view.popup.runCard(body);
        await flush();
        assert.deepEqual(calls, ['card']);
        finish.resolve('{"headword":"run","definition":"move quickly"}');
        await generating;
        await flush();
        assert.deepEqual(calls, automatic ? ['card', 'explanation'] : ['card']);
        if (!automatic) {
            body.querySelector('.explain-btn').listeners.click();
            await flush();
            assert.deepEqual(calls, ['card', 'explanation']);
        }
        assert.equal(view.lookup.explanation, 'Contextual explanation');
        assert.equal(body.querySelector('.explanation').hidden, false);
    }
});

test('explanation reset forces only Explain while card and normal clicks reuse cached answers', async () => {
    const calls = [];
    const view = await fixture({explain: async (_options, _word, _sentence, request) => {
        calls.push(request.prompt);
        return request.prompt === 'card' ? '{"headword":"run","definition":"move quickly"}' : `Explanation ${calls.length}`;
    }});
    view.ai.customPrompt = 'explanation';
    const body = new Element();
    await view.popup.runCard(body);
    const explainButton = body.querySelector('.explain-btn');
    explainButton.listeners.click();
    await flush();
    explainButton.listeners.click();
    await flush();
    assert.deepEqual(calls, ['card', 'explanation']);
    const card = view.lookup.aiCard;
    body.querySelector('.explain-reset').listeners.click();
    await flush();
    assert.deepEqual(calls, ['card', 'explanation', 'explanation']);
    assert.equal(view.lookup.aiCard, card);
    await view.popup.runCard(body);
    assert.deepEqual(calls, ['card', 'explanation', 'explanation']);
});

test('UI mode changes reuse completed AI answers but provider model, prompt and context invalidate them', async () => {
    let requests = 0;
    const view = await fixture({explain: async () => { requests++; return '{"headword":"run","definition":"move"}'; }});
    await view.popup.runCard(new Element());
    view.ai.autoExplain = false;
    view.ai.streamResponse = false;
    view.ai.customPrompt = 'unused explanation prompt';
    view.ai.cancelPendingRequests = false;
    view.ai.openai = {model: 'inactive provider'};
    await view.popup.runCard(new Element());
    assert.equal(requests, 1);
    view.ai.custom.model = 'different model';
    await view.popup.runCard(new Element());
    view.ai.cardPrompt = 'different card prompt';
    await view.popup.runCard(new Element());
    view.lookup.payload.sentence = 'I run a business.';
    await view.popup.runCard(new Element());
    assert.equal(requests, 4);
});

test('Anki waits for the complete explanation when automatic Explain is enabled', async () => {
    const finish = deferred();
    const view = await fixture({explain: async (_options, _word, _sentence, request) => {
        if (request.prompt === 'card') { return '{"headword":"run","definition":"move"}'; }
        request.onChunk('Partial', 'Partial explanation');
        return finish.promise;
    }});
    view.options.enable = true;
    view.ai.autoExplain = true;
    view.ai.customPrompt = 'explanation';
    await view.popup.runCard(new Element());
    await flush();
    const button = view.document.getElementById('btn-anki');
    assert.equal(button.dataset.mode, 'generating');
    await view.popup.onAnkiClick();
    assert.equal(view.calls.notes.length, 0);
    finish.resolve('Complete explanation');
    await flush();
    assert.equal(view.lookup.explanation, 'Complete explanation');
    assert.equal(button.dataset.mode, 'add');
});

test('a superseded card cannot unlock Anki while its forced replacement is generating', async () => {
    const first = deferred(), second = deferred();
    let calls = 0;
    const view = await fixture({explain: async () => (++calls === 1 ? first.promise : second.promise)});
    view.options.enable = true;
    const body = new Element();
    const initial = view.popup.runCard(body);
    await flush();
    const replacement = view.popup.runCard(body, true);
    await flush();
    first.resolve('{"headword":"run","definition":"old"}');
    await initial;
    assert.equal(view.lookup.aiPending, true);
    assert.equal(view.document.getElementById('btn-anki').dataset.mode, 'generating');
    assert.equal(view.lookup.aiCard, undefined);
    second.resolve('{"headword":"run","definition":"new"}');
    await replacement;
    assert.equal(view.lookup.aiCard.definition, 'new');
    assert.equal(view.document.getElementById('btn-anki').dataset.mode, 'add');
});

test('Anki export waits for the complete streamed word card', async () => {
    const finish = deferred();
    const view = await fixture({explain: async (_options, _word, _sentence, request) => {
        request.onChunk('', '{"headword":"run","definition":"move');
        return finish.promise;
    }});
    view.options.enable = true;
    const pending = view.popup.runCard(new Element());
    await flush();
    const button = view.document.getElementById('btn-anki');
    assert.equal(button.dataset.mode, 'generating');
    assert.equal(button.disabled, true);
    await view.popup.onAnkiClick();
    assert.equal(view.calls.notes.length, 0);
    finish.resolve('{"headword":"run","definition":"move quickly"}');
    await pending;
    assert.equal(button.dataset.mode, 'add');
    assert.equal(button.disabled, false);
});

test('enabled Anki with missing format exposes setup and opens its settings', async () => {
    const view = await fixture({ready: false});
    view.options.enable = true;
    await view.popup.refreshAnkiButton();
    const button = view.document.getElementById('btn-anki');
    assert.equal(button.hidden, false);
    assert.equal(button.dataset.mode, 'configure');
    assert(view.document.getElementById('anki-status').textContent.includes('baralho'));
    await view.popup.onAnkiClick();
    assert.equal(view.calls.settings, 1);
    assert.equal(view.calls.events[0][0], 'navigate-settings');
    assert.equal(view.calls.events[0][1].section, 'anki');
});

test('saving Anki configuration updates the already-open lookup button', async () => {
    let configured = false;
    const view = await fixture({ready: () => configured});
    view.options.enable = true;
    await view.popup.refreshAnkiButton();
    assert.equal(view.document.getElementById('btn-anki').dataset.mode, 'configure');
    configured = true;
    await view.calls.listeners.get('settings-changed')();
    assert.equal(view.document.getElementById('btn-anki').dataset.mode, 'add');
    assert.equal(view.document.getElementById('anki-status').hidden, true);
});

test('a delayed duplicate check cannot unlock Anki while the AI card is generating', async () => {
    const duplicates = deferred();
    const view = await fixture({connection: {canAddNotes: () => duplicates.promise}});
    view.options.enable = true;
    view.options.checkForDuplicates = true;
    view.options.duplicateBehavior = 'prevent';
    const checking = view.popup.refreshAnkiButton();
    await flush();
    view.lookup.aiPending = true;
    view.document.getElementById('btn-anki').dataset.mode = 'generating';
    duplicates.resolve([false]);
    await checking;
    assert.equal(view.document.getElementById('btn-anki').dataset.mode, 'generating');
});

test('adding an Anki note retains the clicked word across a later lookup', async () => {
    const glossary = deferred();
    const view = await fixture({entryHtml: () => glossary.promise, connection: {addNote: async (note) => {
        assert.equal(note.fields.Word, 'run');
        return 123;
    }}});
    view.options.enable = true;
    view.lookup.payload.entries = [{dictId: 'test', entry: 1}];
    view.document.getElementById('btn-anki').dataset.mode = 'add';
    const adding = view.popup.onAnkiClick();
    const next = {payload: {word: 'walk'}, keys: new Set()};
    view.change(next);
    view.document.getElementById('btn-anki').dataset.mode = 'add';
    glossary.resolve('<p>run</p>');
    await adding;
    assert.equal(view.lookup.ankiNoteId, 123);
    assert.equal(next.ankiNoteId, undefined);
    assert.equal(view.document.getElementById('btn-anki').dataset.mode, 'add');
});

test('an AnkiConnect that is not running gives no notice while looking up words', async () => {
    const view = await fixture({connection: {canAddNotes: async () => { throw new Error('AnkiConnect connection refused'); }}});
    view.options.enable = true;
    view.options.checkForDuplicates = true;
    await view.popup.refreshAnkiButton();
    const button = view.document.getElementById('btn-anki');
    assert.equal(button.dataset.mode, 'add', 'the button stays, to try again');
    assert.notEqual(view.document.getElementById('anki-status').hidden, false, 'no error notice in the pop-up');
    assert(button.title.includes('connection refused'), 'the reason is in the tooltip');
});

test('Anki add failures show an actionable visible message', async () => {
    const view = await fixture({connection: {addNote: async () => { throw new Error('AnkiConnect connection refused'); }}});
    view.options.enable = true;
    view.document.getElementById('btn-anki').dataset.mode = 'add';
    await view.popup.onAnkiClick();
    const status = view.document.getElementById('anki-status');
    assert.equal(status.hidden, false);
    assert(status.textContent.includes('connection refused'));
    assert.equal(view.document.getElementById('btn-anki').dataset.mode, 'add');
});

test('the pop-up empties itself before Linux hides it, and says so', async () => {
    const view = await fixture();
    view.calls.listeners.get('popup-blank')({payload: {token: 3}});
    assert.deepEqual(view.calls.blanks, [3]);
    view.calls.listeners.get('popup-blank')({payload: null});
    assert.deepEqual(view.calls.blanks, [3]);
});

async function imagesFixture({explain, autoShowImages = false}) {
    const searches = [];
    const requests = [];
    const view = await fixture({
        explain: async (_options, word, text, request) => { requests.push({word, text, prompt: request.prompt, system: request.system}); return explain(); },
        dictionaries: [{id: 'ai-images', kind: 'ai-images', name: 'Images', enabled: true}],
        images: {searchImages: async (query) => {
            searches.push(query);
            return [{src: 'https://p.example/1.jpg', thumb: 'https://t.example/1', title: 'a picture', page: 'https://p.example/page'}];
        }},
    });
    Object.assign(view.ai, {customPrompt: 'explain', contextPrompt: 'translate', autoShowImages, imageQuerySystemPrompt: 'SYS {{wordLanguage}}', imageQueryPrompt: 'USER {{target}}'});
    await view.popup.render({word: 'corre', sentence: 'Ele corre todo dia.', context: 'Um parágrafo longo. Ele corre todo dia.', entries: []});
    const [sectionNode] = view.document.getElementById('sections').children;
    const body = sectionNode.querySelector('.sec-body');
    const settle = async () => { for (let i = 0; i < 20; i++) { await flush(); } };
    return {view, sectionNode, body, searches, requests, settle};
}

test('pictures come on a click, searched with the AI query made by their own prompts', async () => {
    const {sectionNode, body, searches, requests, settle} = await imagesFixture({explain: async () => '"to run"\n'});
    assert(sectionNode.innerHTML.includes('Imagens'));
    assert(body.innerHTML.includes('Mostrar imagens'), 'like Explicar, nothing is fetched before the click');
    await settle();
    assert.equal(searches.length, 0);
    body.querySelector('button').listeners.click();
    await settle();
    assert.equal(requests.length, 1);
    assert.equal(requests[0].prompt, 'USER {{target}}');
    assert(requests[0].system.startsWith('SYS ') && !requests[0].system.includes('{{'), 'the image query sends its own system prompt');
    assert.equal(requests[0].text, 'Ele corre todo dia.', 'the query reads the sentence, not the whole paragraph');
    assert.deepEqual(searches, ['to run']);
    assert(body.innerHTML.includes('https://t.example/1'), 'the strip shows Bing\'s thumbnail');
    assert(body.innerHTML.includes('img-check'), 'each picture has its check mark');
    assert(body.innerHTML.includes('Busca: to run'));
});

test('with automatic pictures on they load at once, and a failed AI query searches the word itself', async () => {
    const {body, searches, settle} = await imagesFixture({autoShowImages: true, explain: async () => { throw new Error('quota'); }});
    await settle();
    assert(!body.innerHTML.includes('Mostrar imagens'));
    assert.deepEqual(searches, ['corre']);
    assert(body.innerHTML.includes('https://t.example/1'));
});

test('meme mode adds " meme" to the AI query, and the switch shows it', async () => {
    const {view, searches, requests, settle} = await imagesFixture({autoShowImages: true, explain: async () => 'to run'});
    await settle();
    assert.deepEqual(searches, ['to run']);
    view.ai.imageMemeMode = true;
    await view.popup.render({word: 'corre', sentence: 'Ele corre todo dia.', entries: []});
    await settle();
    assert.deepEqual(searches, ['to run', 'to run meme']);
    const [sectionNode] = view.document.getElementById('sections').children;
    const shown = sectionNode.querySelector('.sec-body');
    assert(shown.innerHTML.includes('Modo meme') && shown.innerHTML.includes(' checked'), 'the switch shows the mode');
    assert(shown.innerHTML.includes('Busca: to run meme'));
    assert.equal(requests.length, 1, 'the AI query is reused from the cache');
});

test('each AI dictionary sends its requests with its own provider', async () => {
    const seen = [];
    const view = await fixture({
        explain: async (options, _word, _sentence, _request) => { seen.push([options.provider, options[options.provider].model]); return 'ok'; },
    });
    Object.assign(view.ai, {customPrompt: 'explain', contextPrompt: 'translate',
        provider: 'custom', custom: {model: 'word-model'},
        sectionProviders: {'ai-context': {provider: 'gemini', gemini: {model: 'context-model'}, temperature: 0.2, streamResponse: false, cancelPendingRequests: true, systemPrompt: ''}}});
    await view.popup.runContext(new Element());
    await view.popup.runExplain(new Element());
    assert.deepEqual(seen, [['gemini', 'context-model'], ['custom', 'word-model']]);
});

test('the context translation can stay blurred until clicked, like the word translation', async () => {
    const view = await fixture({explain: async () => 'Eu **corro** todo dia.'});
    Object.assign(view.ai, {contextPrompt: 'translate', hideContextTranslation: true});
    const body = new Element();
    await view.popup.runContext(body);
    assert(body.innerHTML.includes('translation hidden-tr'), 'blurred at first');
    assert(body.innerHTML.includes('<mark>corro</mark>'));
    assert(body.innerHTML.includes('speak-sentence'), 'a speaker reads the original sentence aloud');
    view.lookup.contextRevealed = true;
    await view.popup.runContext(body);
    assert(!body.innerHTML.includes('hidden-tr'), 'once shown it stays shown for the lookup');
    view.ai.hideContextTranslation = false;
    view.lookup.contextRevealed = false;
    await view.popup.runContext(body);
    assert(!body.innerHTML.includes('hidden-tr'), 'off by default');
});

test('{audio} gets the word audio from a local database source, as an Anki sound file', async () => {
    const view = await fixture({audioRows: async (command, args) => {
        if (command === 'audio_lookup') { return [{source: 'nhk16', file: 'audio/yomu.ogg', name: 'NHK16 ヨム'}]; }
        if (command === 'audio_data') { return args.source === 'nhk16' ? ['T2dnUw==', 'ogg'] : null; }
        return null;
    }});
    view.settings.audio = {sources: [{type: 'local-db', path: 'C:/audio/android.db'}, {type: 'tts'}]};
    view.options.enable = true;
    view.lookup.payload.word = '読む';
    view.document.getElementById('btn-anki').dataset.mode = 'add';
    await view.popup.onAnkiClick();
    const ctx = view.calls.notes.at(-1);
    const file = await ctx.audio();
    assert.deepEqual(JSON.parse(JSON.stringify(file)), {data: 'T2dnUw==', extension: 'ogg', mime: 'audio/ogg'});
    assert.deepEqual(JSON.parse(JSON.stringify(view.calls.invokes[0])), ['audio_lookup', {path: 'C:/audio/android.db', term: '読む', reading: null, order: null}]);
});

test('a counter beside the bookmark shows how many cards of the word were saved, and opens them', async () => {
    const words = [
        {id: 'a', word: 'run', dictionaryForm: 'run', translation: 'correr', sentence: 'I run daily.', createdAt: 1},
        {id: 'b', word: 'ran', dictionaryForm: 'Run', translation: 'correu', sentence: 'She ran home.', contextTranslation: 'Ela **correu** para casa.', createdAt: 2},
        {id: 'c', word: 'walk', dictionaryForm: 'walk', createdAt: 3},
    ];
    const view = await fixture({words});
    await view.popup.refreshSavedCount(view.lookup);
    const button = view.nodes.get('btn-saved');
    assert.equal(button.hidden, false);
    assert(button.innerHTML.includes('<span class="n">2</span>'), 'the word as found and its dictionary form both count');
    assert(button.title.includes('2 vezes em contextos diferentes'));
    await view.popup.openSavedCards();
    const box = view.nodes.get('card').children.at(-1);
    assert.equal(box.className, 'saved-view');
    const newer = box.innerHTML.indexOf('She <mark>ran</mark> home.');
    assert(newer >= 0 && newer < box.innerHTML.indexOf('I <mark>run</mark> daily.'), 'newest first, the word marked in its sentence');
    assert(box.innerHTML.includes('<mark>correu</mark>'), 'the saved context translation');
    const empty = await fixture({words: [words[2]]});
    await empty.popup.refreshSavedCount(empty.lookup);
    assert.equal(empty.nodes.get('btn-saved').hidden, true, 'no counter for a word never saved');
});

test('a counter beside the Anki button shows the notes of the word in Anki, and opens them in Anki', async () => {
    const browsed = [];
    const connection = {getModelFieldNames: async () => ['Word'], findNoteIds: async () => [[11, 12]], guiBrowseNotes: async (ids) => { browsed.push(ids); }};
    const view = await fixture({connection});
    Object.assign(view.options, {enable: true, checkForDuplicates: true});
    await view.popup.refreshAnkiNotes(view.lookup, view.options);
    const button = view.nodes.get('btn-anki-notes');
    assert.equal(button.hidden, false);
    assert(button.innerHTML.includes('<span class="n">2</span>'));
    assert(button.title.includes('2 notas'));
    await view.popup.openAnkiNotes();
    assert.equal(JSON.stringify(browsed), JSON.stringify([[11, 12]]));
    view.options.checkForDuplicates = false;
    await view.popup.refreshAnkiNotes(view.lookup, view.options);
    assert.equal(button.hidden, true, 'off with the duplicate check');
});

test('the Anki notes counter does not wait for the AI card and searches every form of the word', async () => {
    const queried = [];
    const connection = {getModelFieldNames: async () => ['Word'], findNoteIds: async (notes) => {
        queried.push(notes.map((n) => n.fields.Word));
        return notes.map((n) => (n.fields.Word === 'run' ? [7] : []));
    }};
    const view = await fixture({connection});
    Object.assign(view.options, {enable: true, checkForDuplicates: true});
    view.lookup.payload = {word: 'ran', sentence: 'She ran home.', entries: [{headword: 'run'}]};
    view.lookup.aiPending = true;
    await view.popup.refreshAnkiButton();
    await flush();
    assert.equal(JSON.stringify(queried[0]), JSON.stringify(['ran', 'run']), 'the word as found and its headword, in one request');
    const button = view.nodes.get('btn-anki-notes');
    assert.equal(button.hidden, false, 'shown while the AI card is still generating');
    assert(button.innerHTML.includes('<span class="n">1</span>'));
});

test('a slow, older Anki notes check cannot overwrite a newer one', async () => {
    const slow = deferred();
    let calls = 0;
    const connection = {getModelFieldNames: async () => ['Word'], findNoteIds: async () => { calls++; return calls === 1 ? slow.promise : [[5]]; }};
    const view = await fixture({connection});
    Object.assign(view.options, {enable: true, checkForDuplicates: true});
    const first = view.popup.refreshAnkiNotes(view.lookup, view.options);
    await view.popup.refreshAnkiNotes(view.lookup, view.options);
    slow.resolve([[1, 2, 3]]);
    await first;
    assert(view.nodes.get('btn-anki-notes').innerHTML.includes('<span class="n">1</span>'), 'the newest answer stays');
});

test('{sentence-audio} reads only the sentence, with the Edge voice of the context or the language default', async () => {
    const view = await fixture({audioRows: async (command) => (command === 'edge_tts_speak' ? 'SUQz' : null)});
    view.settings.audio = {sources: [{type: 'tts'}], context: {type: 'edge-tts', voice: 'en-US-AriaNeural'}};
    view.options.enable = true;
    view.lookup.payload = {word: 'run', sentence: 'I run daily.', context: 'Every morning, I run daily. It helps.', entries: []};
    view.document.getElementById('btn-anki').dataset.mode = 'add';
    await view.popup.onAnkiClick();
    const ctx = view.calls.notes.at(-1);
    const file = await ctx.sentenceAudio();
    assert.equal(JSON.stringify(file), JSON.stringify({data: 'SUQz', extension: 'mp3', mime: 'audio/mpeg'}));
    const speak = view.calls.invokes.find(([command]) => command === 'edge_tts_speak');
    assert.equal(JSON.stringify(speak[1]), JSON.stringify({text: 'I run daily.', voice: 'en-US-AriaNeural', language: 'en-US'}), 'the sentence, not the paragraph');
    view.settings.audio.context = {type: 'tts', voice: ''};
    await ctx.sentenceAudio();
    const last = view.calls.invokes.filter(([command]) => command === 'edge_tts_speak').at(-1);
    assert.equal(last[1].voice, '', 'with the system voice, the language default Edge voice');
});

test('the Anki notes counter queries the note type\'s first field, and nothing when it would be empty', async () => {
    const queried = [];
    let model = ['Word', 'Audio'];
    let asked = 0;
    const connection = {
        getModelFieldNames: async () => { asked++; return model; },
        findNoteIds: async (notes) => { queried.push(Object.keys(notes[0].fields)[0]); return [[3]]; },
    };
    const view = await fixture({connection});
    Object.assign(view.options, {enable: true, checkForDuplicates: true, server: 'http://127.0.0.1:8765', cardFormats: [{model: 'Mine'}]});
    await view.popup.refreshAnkiNotes(view.lookup, view.options);
    assert.equal(queried[0], 'Word', 'the note type\'s first field, not the first key of the saved settings');
    await view.popup.refreshAnkiNotes(view.lookup, view.options);
    assert.equal(asked, 1, 'the note type\'s fields are asked once');
    // A note type whose first field Lookitan leaves empty: an empty query would match every note.
    model = ['Expression', 'Word'];
    const other = await fixture({connection});
    Object.assign(other.options, {enable: true, checkForDuplicates: true, server: 'http://127.0.0.1:8765', cardFormats: [{model: 'Other'}]});
    await other.popup.refreshAnkiNotes(other.lookup, other.options);
    assert.equal(queried.length, 2, 'not asked');
    assert.equal(other.nodes.get('btn-anki-notes').hidden, true);
});

test('a saved card opens whole in the pop-up, as the Palavras tab shows it', async () => {
    const words = [{id: 'a', word: 'optimized', dictionaryForm: 'optimize', ipa: 'ˈɑptɪmaɪz', cefr: 'C1', partOfSpeech: ['verb'], labels: ['technical'],
        translation: 'otimizar', definition: 'To make as effective as possible.', explanation: 'Used for **software**.',
        sentence: 'It is optimized for speed.', context: 'A new model. It is optimized for speed. It supports text.',
        contextTranslation: 'É **otimizado** para velocidade.', glossaries: [{name: 'Oxford', html: '<b>optimize</b> v.'}], createdAt: 1}];
    const view = await fixture({words});
    view.lookup.payload.word = 'optimized';
    await view.popup.refreshSavedCount(view.lookup);
    await view.popup.openSavedCards();
    const html = view.nodes.get('card').children.at(-1).innerHTML;
    for (const part of ['ˈɑptɪmaɪz', 'C1', '>verb<', '>technical<', 'otimizar', 'To make as effective as possible.', 'Used for **software**.',
        'A new model. It is <mark>optimized</mark> for speed. It supports text.', '<mark>otimizado</mark>', 'Na tela: optimized',
        '<summary>Oxford</summary>', 'clean(<b>optimize</b> v.)']) {
        assert(html.includes(part), `the saved card shows ${part}`);
    }
});

