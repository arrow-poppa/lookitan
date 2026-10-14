import assert from 'node:assert/strict';
import {readFile} from 'node:fs/promises';
import {resolve} from 'node:path';
import test from 'node:test';
import {createHash} from 'node:crypto';
import {CARD_PROMPT} from '../src/lib/ai/word-card.js';
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
}

async function fixture({explain = async () => '', ready = true, connection = {}, entryHtml = async () => '', dictionaries = []} = {}) {
    const nodes = new Map();
    const document = {getElementById: (id) => {
        if (!nodes.has(id)) { nodes.set(id, new Element()); }
        return nodes.get(id);
    }, createElement: () => new Element(), addEventListener() {}};
    const calls = {settings: 0, hides: 0, events: [], notes: [], listeners: new Map()};
    const lookup = {payload: {word: 'run', sentence: 'I run daily.', entries: []}, keys: new Set()};
    const options = {enable: false, checkForDuplicates: false};
    const settings = {hideWordTranslation: false, saveWordsAutomatically: false, ocrLanguage: 'en-US'};
    const ai = {streamResponse: true, provider: 'custom', custom: {model: 'test'}, cardPrompt: 'card'};
    const entry = file('src/popup/main.js');
    const stubs = {
        '@tauri-apps/api/event': {listen: async (name, handler) => { calls.listeners.set(name, handler); return () => {}; }, emit: async (...args) => { calls.events.push(args); }},
        [file('src/lib/api.js')]: {
            api: {openSettings: async () => { calls.settings++; }, hidePopup: async () => { calls.hides++; }, entryHtml, listDictionaries: async () => dictionaries},
            entryUrl: () => '', escapeHtml: (value) => String(value).replace(/[&<>"']/g, (c) =>
                ({'&': '&amp;', '<': '&lt;', '>': '&gt;', '"': '&quot;', "'": '&#39;'})[c]),
            loadAllSettings: async () => ({settings, ai, anki: options}), renderMarkdown: (text) => String(text),
        },
        [file('src/lib/ai/explain.js')]: {explain, isConfigured: () => true},
        [file('src/lib/anki/notes.js')]: {
            createConnection: () => connection,
            formatUsesMarker: () => false,
            glossaryDictionaryNames: () => [],
            isFormatReady: () => typeof ready === 'function' ? ready() : ready,
            buildNote: async (ctx) => { calls.notes.push(ctx); return {fields: {Word: ctx.word}, options: {}}; },
        },
        [file('src/lib/cards.js')]: {cardFromLookup: () => { throw new Error('Unexpected local word save'); }},
        [file('src/lib/icons.js')]: {hydrateIcons() {}, icon: (name) => name},
    };
    const context = createContext({document, window: {addEventListener() {}}, localStorage: {getItem: () => null, setItem() {}},
        AbortController, DOMException, crypto: {subtle: {digest: async (_algorithm, bytes) => createHash('sha256').update(bytes).digest()}},
        TextEncoder, console, Intl, setTimeout, clearTimeout});
    const modules = new Map();
    async function get(id) {
        if (modules.has(id)) { return modules.get(id); }
        const stub = stubs[id];
        const module = stub ? new SyntheticModule(Object.keys(stub), function () {
            for (const [name, value] of Object.entries(stub)) { this.setExport(name, value); }
        }, {context, identifier: id}) : new SourceTextModule(await readFile(id, 'utf8') + (id === entry ?
            '\nexport {runCard, runExplain, refreshAnkiButton, onAnkiClick};\nexport function contextForTest(value) { state = value.lookup; ai = value.ai; anki = value.anki; settings = value.settings; }' : ''),
        {context, identifier: id});
        modules.set(id, module);
        await module.link((specifier) => get(specifier.startsWith('.') ? resolve(id, '..', specifier) : specifier));
        return module;
    }
    const module = await get(entry);
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
