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
