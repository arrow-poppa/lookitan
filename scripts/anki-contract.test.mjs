import assert from 'node:assert/strict';
import {readFile} from 'node:fs/promises';
import {resolve} from 'node:path';
import test from 'node:test';
import {createContext, SourceTextModule, SyntheticModule} from 'node:vm';

const project = resolve(import.meta.dirname, '..');
const flush = () => new Promise((done) => setImmediate(done));

async function clientFixture(fetch, timerOverrides = {}) {
    const context = createContext({AbortController, setTimeout, clearTimeout, ...timerOverrides});
    const modules = new Map();
    async function load(id) {
        if (modules.has(id)) { return modules.get(id); }
        const pending = (async () => id === '@tauri-apps/plugin-http' ? new SyntheticModule(['fetch'], function () {
            this.setExport('fetch', fetch);
        }, {context, identifier: id}) : new SourceTextModule(await readFile(id, 'utf8'), {context, identifier: id}))();
        modules.set(id, pending);
        return pending;
    }
    const module = await load(resolve(project, 'src/lib/anki/notes.js'));
    await module.link((specifier, owner) => load(specifier.startsWith('.') ? resolve(owner.identifier, '..', specifier) : specifier));
    await module.evaluate();
    const options = module.namespace.defaultAnkiOptions();
    options.enable = true;
    const client = module.namespace.createConnection(options);
    return {client, options, notes: module.namespace};
}

function response(value) { return {ok: true, status: 200, text: async () => JSON.stringify(value)}; }

test('Anki legacy RPC preserves version, authentication, model fields and note payloads', async () => {
    const requests = [];
    const replies = {version: 6, deckNames: ['Study'], modelNames: ['Basic'], modelFieldNames: ['Front', 'Back'], addNote: 1234};
    const {client} = await clientFixture(async (url, init) => {
        const body = JSON.parse(init.body);
        requests.push({url, body});
        return response(replies[body.action]);
    });
    client.apiKey = 'test-key';
    assert.equal(await client.getVersion(), 6);
    assert.deepEqual([...await client.getDeckNames()], ['Study']);
    assert.deepEqual([...await client.getModelNames()], ['Basic']);
    assert.deepEqual([...await client.getModelFieldNames('Basic')], ['Front', 'Back']);
    const note = {deckName: 'Study', modelName: 'Basic', fields: {Front: 'hello', Back: 'olá'}, tags: ['lookitan']};
    assert.equal(await client.addNote(note), 1234);
    assert(requests.every(({url, body}) => url === 'http://127.0.0.1:8765' && body.version === 2 && body.key === 'test-key'));
    assert.deepEqual(requests.find(({body}) => body.action === 'modelFieldNames').body.params, {modelName: 'Basic'});
    assert.deepEqual(requests.find(({body}) => body.action === 'addNote').body.params, {note});
});

function forbidden() { return {ok: false, status: 403, text: async () => ''}; }

test('a 403 from AnkiConnect asks for permission once and retries the request', async () => {
    const actions = [];
    let trusted = false;
    const {client} = await clientFixture(async (_url, init) => {
        const {action} = JSON.parse(init.body);
        actions.push(action);
        if (action === 'requestPermission') { trusted = true; return response({permission: 'granted', requireApikey: false, version: 6}); }
        if (!trusted) { return forbidden(); }
        return response({version: 6, deckNames: ['Study']}[action]);
    });
    const [decks, again] = await Promise.all([client.getDeckNames(), client.getDeckNames()]);
    assert.deepEqual([...decks], ['Study']);
    assert.deepEqual([...again], ['Study']);
    assert.equal(actions.filter((a) => a === 'requestPermission').length, 1, 'concurrent requests share one Anki prompt');
});

test('a denied AnkiConnect permission explains what to do in Anki', async () => {
    const {client} = await clientFixture(async (_url, init) => JSON.parse(init.body).action === 'requestPermission' ?
        response({permission: 'denied'}) : forbidden());
    await assert.rejects(client.getDeckNames(), /recusou o acesso do Lookitan/);
});

test('a 403 after permission was granted is reported instead of looping', async () => {
    let permissions = 0;
    const {client} = await clientFixture(async (_url, init) => {
        if (JSON.parse(init.body).action === 'requestPermission') { permissions++; return response({permission: 'granted'}); }
        return forbidden();
    });
    await assert.rejects(client.getDeckNames(), /bloqueou o Lookitan \(403\)/);
    assert.equal(permissions, 1);
});

test('Anki wrapped success replies do not treat error:null as failure', async () => {
    const replies = {version: 6, deckNames: ['Study'], addNote: null};
    const {client} = await clientFixture(async (_url, init) => response({result: replies[JSON.parse(init.body).action], error: null}));
    assert.equal(await client.getVersion(), 6);
    assert.deepEqual([...await client.getDeckNames()], ['Study']);
    assert.equal(await client.addNote({fields: {Front: 'hello'}}), null);
});

test('Anki API errors remain actionable and preserve unsupported-action detection', async () => {
    const {client} = await clientFixture(async () => response({result: null, error: 'unsupported action'}));
    await assert.rejects(client.getVersion(), (error) => error.message === 'Anki error: unsupported action' && client.isErrorUnsupportedAction(error));
});

test('Anki connection failures preserve the native cause instead of a generic message', async () => {
    const {client} = await clientFixture(async () => { throw new Error('URL denied by HTTP scope'); });
    await assert.rejects(client.getVersion(), /URL denied by HTTP scope/);
});

test('Anki timeout aborts a pending request and releases its timer', async () => {
    const timers = [];
    const cleared = [];
    let observedSignal;
    const {client} = await clientFixture((_url, init) => {
        observedSignal = init.signal;
        return new Promise((_resolve, reject) => init.signal.addEventListener('abort', () => reject(new Error('Request cancelled'))));
    }, {
        setTimeout: (callback, milliseconds) => { timers.push({callback, milliseconds}); return timers.length; },
        clearTimeout: (timer) => { cleared.push(timer); },
    });
    const request = client.getVersion();
    const rejection = assert.rejects(request, /Tempo de conexão com o Anki esgotado/);
    await flush();
    assert.equal(timers[0].milliseconds, 5000);
    timers[0].callback();
    await rejection;
    assert.equal(observedSignal.aborted, true);
    assert.deepEqual(cleared, [1]);
});

test('Anki note construction fills markers and respects duplicate scope without sending a note', async () => {
    const {options, notes} = await clientFixture(async () => { throw new Error('note construction must not use HTTP'); });
    options.cardFormats = [{deck: 'Languages::English', model: 'Basic', fields: {
        Front: {value: '{expression}', overwriteMode: 'coalesce'},
        Back: {value: '{translation}<br>{sentence}', overwriteMode: 'coalesce'},
    }}];
    options.duplicateBehavior = 'prevent';
    options.duplicateScope = 'deck-root';
    const built = await notes.buildNote({word: 'hello', sentence: 'Say hello & smile.', aiCard: {translation: 'olá'}}, options, null);
    assert.equal(built.deckName, 'Languages::English');
    assert.equal(built.modelName, 'Basic');
    assert.equal(built.fields.Front, 'hello');
    assert.equal(built.fields.Back, 'olá<br>Say hello &amp; smile.');
    assert.equal(built.options.allowDuplicate, false);
    assert.equal(built.options.duplicateScope, 'deck');
    assert.equal(built.options.duplicateScopeOptions.deckName, 'Languages');
    assert.equal(built.options.duplicateScopeOptions.checkChildren, true);
});

// Enough of DOMParser for cleanGlossaryHtml and htmlToText on simple, script-free entries.
class FakeDOMParser {
    parseFromString(html) {
        return {
            querySelectorAll: () => [],
            body: {firstElementChild: {innerHTML: html.replace(/^<div>|<\/div>$/g, '')}, textContent: html.replace(/<[^>]+>/g, '')},
        };
    }
}

test('{ai-card} exports the whole AI card without the context, which stays in {sentence}', async () => {
    const {notes, options} = await clientFixture(async () => response(null), {DOMParser: FakeDOMParser});
    options.cardFormats[0].deck = 'Study';
    options.cardFormats[0].model = 'Basic';
    options.cardFormats[0].fields = {A: {value: '{ai-card}'}, B: {value: '{ai-card-plain}'}, C: {value: '{sentence}'}};
    const ctx = {
        word: 'ran',
        headword: 'run',
        sentence: 'She ran home.',
        aiCard: {definition: 'move fast', translation: 'correr', ipa: 'rʌn', cefr: 'A1', partOfSpeech: ['verb'], labels: ['informal']},
        aiExplanation: '**Run** means to move fast.',
    };
    const f = (await notes.buildNote(ctx, options, null)).fields;
    for (const part of ['<b>run</b>', '/rʌn/', '[A1]', '<i>verb, informal</i>', '<b>correr</b>', 'move fast', '<strong>Run</strong>']) {
        assert(f.A.includes(part), `{ai-card} has ${part}`);
    }
    assert(!f.A.includes('She ran home'), 'the context is left to {sentence}');
    assert.equal(f.B, 'run /rʌn/ [A1]<br>verb, informal<br>correr<br>move fast<br>Run means to move fast.');
    assert.equal(f.C, 'She ran home.');
    const empty = (await notes.buildNote({word: 'ran', sentence: 'She ran home.'}, options, null)).fields;
    assert.equal(empty.A, '', 'no AI card, no field');
});

test('Yomitan glossary markers: per dictionary, first, no-dictionary, popup selection and AI explanation', async () => {
    const {notes, options} = await clientFixture(async () => response(null), {DOMParser: FakeDOMParser});
    const oxford = "Oxford Advanced Learner's Dictionary";
    options.cardFormats[0].deck = 'Study';
    options.cardFormats[0].model = 'Basic';
    options.cardFormats[0].fields = {
        A: {value: `{single-glossary-${notes.getKebabCase(oxford)}}`},
        B: {value: '{single-glossary-wordreference-brief}'},
        C: {value: '{glossary-first}'},
        D: {value: '{popup-selection-text}'},
        E: {value: '{ai-explanation}'},
        F: {value: '{single-glossary-not-installed}'},
        G: {value: '{glossary-no-dictionary}'},
        H: {value: '{ai-explanation-plain}'},
        I: {value: '{single-glossary-ia-plain}'},
    };
    const note = await notes.buildNote({
        word: 'run',
        glossaries: [{name: 'WordReference', html: 'correr'}, {name: oxford, html: '<b>to move</b>'}],
        aiCard: {definition: 'move fast', partOfSpeech: ['verb'], labels: ['informal']},
        aiExplanation: '**Run** means to move fast.\n- one example',
        popupSelection: 'move fast',
        dictionaryOrder: ['IA', oxford, 'WordReference'],
    }, options, null);
    const f = note.fields;
    assert.equal(notes.getKebabCase(oxford), 'oxford-advanced-learners-dictionary');
    assert.equal(f.A, `<div style="text-align: left;"><i>(${oxford})</i> <b>to move</b></div>`);
    assert.equal(f.B, '<div style="text-align: left;">correr</div>');
    assert.equal(f.C, '<div style="text-align: left;"><i>(verb, informal, IA)</i> move fast</div>', 'first follows the dictionary order');
    assert.equal(f.D, 'move fast');
    assert(f.E.includes('<strong>Run</strong>') && f.E.includes('<li>one example</li>'), 'the explanation is rendered, not raw Markdown');
    assert.equal(f.F, '', 'a dictionary without an entry leaves its marker empty');
    assert.equal(f.G, '<div style="text-align: left;"><ol><li><i>(verb, informal)</i> move fast</li><li><b>to move</b></li><li>correr</li></ol></div>');
    assert.equal(f.H, 'Run means to move fast.<br>• one example');
    assert.equal(f.I, '(IA) move fast');
    assert(notes.formatUsesMarker(options, 'ai-explanation'));
    assert.deepEqual([...notes.dictionaryMarkers(['WordReference']).slice(0, 2)], ['single-glossary-wordreference', 'single-glossary-wordreference-brief']);
    assert.deepEqual([...notes.glossaryDictionaryNames([
        {kind: 'ai-word', enabled: true, name: 'AI word in context'}, {kind: 'ai-context', enabled: true, name: 'x'},
        {kind: 'mdict', enabled: false, name: 'Off'}, {kind: 'mdict', enabled: true, name: oxford},
    ])], ['IA', oxford]);
});
