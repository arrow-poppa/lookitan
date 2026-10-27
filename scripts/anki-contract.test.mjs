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

test('{sentence} and the cloze hold the sentence by default and the whole paragraph when chosen', async () => {
    const {options, notes} = await clientFixture(async () => { throw new Error('note construction must not use HTTP'); });
    options.cardFormats = [{deck: 'D', model: 'Basic', fields: {
        Front: {value: '{sentence}', overwriteMode: 'coalesce'},
        Back: {value: '{cloze-prefix}[{cloze-body}]{cloze-suffix}', overwriteMode: 'coalesce'},
    }}];
    const ctx = {word: 'run', sentence: 'I run daily.', context: 'We train. I run daily.'};
    assert.equal(notes.normalizeAnkiOptions({}).contextMode, 'sentence');
    let built = await notes.buildNote(ctx, options, null);
    assert.deepEqual({...built.fields}, {Front: 'I run daily.', Back: 'I [run] daily.'});
    options.contextMode = 'paragraph';
    built = await notes.buildNote(ctx, options, null);
    assert.deepEqual({...built.fields}, {Front: 'We train. I run daily.', Back: 'We train. I [run] daily.'});
    built = await notes.buildNote({word: 'run', sentence: 'I run daily.'}, options, null);
    assert.equal(built.fields.Front, 'I run daily.', 'a lookup without a paragraph keeps the sentence');
});

test('{translation} and the AI card carry every translation, the main one first', async () => {
    const {options, notes} = await clientFixture(async () => { throw new Error('note construction must not use HTTP'); });
    options.cardFormats = [{deck: 'D', model: 'Basic', fields: {
        T: {value: '{translation}'}, Card: {value: '{ai-card}'}, Plain: {value: '{ai-card-plain}'},
    }}];
    const ctx = {word: 'finished', headword: 'finish', sentence: 'She finished.',
        aiCard: {definition: 'to end', translation: 'terminou', translationAlternatives: ['concluiu', '<i>x</i>']}};
    const built = await notes.buildNote(ctx, options, null);
    assert.equal(built.fields.T, 'terminou, concluiu, &lt;i&gt;x&lt;/i&gt;');
    assert(built.fields.Card.includes('terminou, concluiu, &lt;i&gt;x&lt;/i&gt;') && built.fields.Plain.includes('terminou, concluiu'));
    const old = await notes.buildNote({...ctx, aiCard: {definition: 'to end', translation: 'terminou'}}, options, null);
    assert.equal(old.fields.T, 'terminou', 'a card without alternatives, as before');
});

test('the cloze holds the longer unit the AI card marked, when the sentence has it', async () => {
    const {options, notes} = await clientFixture(async () => { throw new Error('note construction must not use HTTP'); });
    options.cardFormats = [{deck: 'D', model: 'Basic', fields: {
        Front: {value: '{cloze-prefix}[{cloze-body}]{cloze-suffix}', overwriteMode: 'coalesce'},
        Back: {value: '{expression}', overwriteMode: 'coalesce'},
    }}];
    const sentence = 'He got nominated to the Socrates Awards.';
    let built = await notes.buildNote({word: 'Socrates', headword: 'Socrates Awards', clozeWord: 'Socrates Awards', sentence}, options, null);
    assert.deepEqual({...built.fields}, {Front: 'He got nominated to the [Socrates Awards].', Back: 'Socrates Awards'});
    built = await notes.buildNote({word: 'Socrates', clozeWord: 'Nobel Prize', sentence}, options, null);
    assert.equal(built.fields.Front, 'He got nominated to the [Socrates] Awards.', 'not in the sentence: the word');
    built = await notes.buildNote({word: 'turned', headword: 'turn off', clozeWord: '', sentence: 'She turned the light off.'}, options, null);
    assert.equal(built.fields.Front, 'She [turned] the light off.', 'a split unit keeps the word');
});

test('the cloze is the occurrence looked up, in the sentence or the paragraph, and falls back when it cannot be placed', async () => {
    const {options, notes} = await clientFixture(async () => { throw new Error('note construction must not use HTTP'); });
    options.cardFormats = [{deck: 'D', model: 'Basic', fields: {Front: {value: '{cloze-prefix}[{cloze-body}]{cloze-suffix}', overwriteMode: 'coalesce'}}}];
    const front = async (ctx) => (await notes.buildNote(ctx, options, null)).fields.Front;
    const cat = 'The cat saw the cat.';
    assert.equal(await front({word: 'cat', sentence: cat, context: cat, clozeAt: [16, 19]}), 'The cat saw the [cat].', 'the second cat');
    assert.equal(await front({word: 'cat', sentence: cat}), 'The [cat] saw the cat.', 'no anchor: the first, as before');
    const para = 'Go home. Go home. Then rest.';
    assert.equal(await front({word: 'home', sentence: 'Go home.', context: para, clozeAt: [12, 16]}), 'Go [home].', 'the sentence that holds it');
    options.contextMode = 'paragraph';
    assert.equal(await front({word: 'home', sentence: 'Go home.', context: para, clozeAt: [12, 16]}), 'Go home. Go [home]. Then rest.', 'the paragraph');
    options.contextMode = 'sentence';
    const nyc = 'New York is big. I love New York.';
    assert.equal(await front({word: 'York', clozeWord: 'New York', sentence: 'I love New York.', context: nyc, clozeAt: [24, 32]}), 'I love [New York].', 'a unit');
    assert.equal(await front({word: 'York', clozeWord: 'New York', sentence: nyc, context: nyc, clozeAt: [24, 32]}), 'New York is big. I love [New York].',
        'a repeated unit, the one looked up');
    assert.equal(await front({word: 'run', sentence: 'I run daily.', context: 'Other text.', clozeAt: [0, 5]}), 'I [run] daily.', 'the range is not in the sentence: the word');
    assert.equal(await front({word: '𠮷', sentence: '𠮷野 and 𠮷', context: '𠮷野 and 𠮷', clozeAt: [7, 8]}), '𠮷野 and [𠮷]', 'code points');
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

test('the AI card carries the reading, synonyms and components, which also have markers of their own', async () => {
    const {options, notes} = await clientFixture(async () => { throw new Error('note construction must not use HTTP'); }, {DOMParser: FakeDOMParser});
    options.cardFormats = [{deck: 'D', model: 'Basic', fields: {
        A: {value: '{ai-card}'}, B: {value: '{ai-card-plain}'}, C: {value: '{reading}'}, D: {value: '{ai-synonyms}'},
        E: {value: '{ai-components}'}, F: {value: '{ai-components-plain}'}, G: {value: '{ai-reading}'}}}];
    const aiCard = {definition: 'expectation', ipa: '', reading: 'qīxǔ', synonyms: ['期望', '期待'],
        components: [{part: '期', reading: 'qī', meaning: 'period of time'}, {part: '許', reading: 'xǔ', meaning: 'to allow'}]};
    const f = (await notes.buildNote({word: '期許', sentence: 's', aiCard}, options, null)).fields;
    assert(f.A.includes('<b>期許</b> qīxǔ') && f.A.includes('<i>期望, 期待</i>') && f.A.includes('<div><b>期</b> qī — period of time</div>'));
    assert.equal(f.B, '期許 qīxǔ<br>expectation<br>期望, 期待<br>期 (qī): period of time<br>許 (xǔ): to allow');
    assert.equal(f.C, 'qīxǔ', '{reading} is the pinyin or kana when there is one');
    assert.equal(f.D, '期望, 期待');
    assert.equal(f.E, '<div><b>期</b> qī — period of time</div><div><b>許</b> xǔ — to allow</div>');
    assert.equal(f.F, '期 (qī): period of time<br>許 (xǔ): to allow');
    assert.equal(f.G, 'qīxǔ');
    const plain = (await notes.buildNote({word: 'run', sentence: 's', aiCard: {definition: 'move fast', ipa: 'rʌn'}}, options, null)).fields;
    assert.equal(plain.C, 'rʌn', 'without a reading, {reading} is still the IPA');
    assert.equal(plain.E, '');
});

test('the form on screen and its transliteration go in the AI card, the tags and markers of their own', async () => {
    const {options, notes} = await clientFixture(async () => { throw new Error('note construction must not use HTTP'); }, {DOMParser: FakeDOMParser});
    options.cardFormats = [{deck: 'D', model: 'Basic', fields: {
        A: {value: '{ai-card}'}, B: {value: '{ai-card-plain}'}, C: {value: '{ai-form}'}, D: {value: '{ai-transliteration}'}, E: {value: '{tags}'},
        F: {value: '{ai-ocr-fix}'}}}];
    const aiCard = {definition: 'to lay eggs', ipa: 'baːdˤa', transliteration: 'fa-bāḍat', form: ['past tense', 'feminine'], partOfSpeech: ['verb'], labels: ['formal'],
        ocrFix: 'فاضت → فباضت'};
    const f = (await notes.buildNote({word: 'فباضت', headword: 'باض', sentence: 's', aiCard}, options, null)).fields;
    assert(f.A.includes('<div dir="auto"><b>باض</b> <i>fa-bāḍat</i> /baːdˤa/</div>') && f.A.includes('<div dir="auto"><i>past tense, feminine, verb, formal</i></div>'));
    assert(f.A.includes('<div dir="auto">to lay eggs</div>'), 'the English definition keeps its own direction under an Arabic headword');
    assert(f.A.includes('<div dir="ltr"><i>OCR: <bdi>فاضت</bdi> → <bdi>فباضت</bdi></i></div>'), 'the OCR fix is exported, read left to right');
    assert(f.B.startsWith('<bdi>باض fa-bāḍat /baːdˤa/</bdi><br><bdi><bdi>OCR: <bdi>فاضت</bdi> → <bdi>فباضت</bdi></bdi></bdi><br><bdi>(past tense, feminine, verb, formal)</bdi>'));
    assert(f.B.includes('<bdi>to lay eggs</bdi>'));
    assert.equal(f.F, '<bdi>فاضت</bdi> → <bdi>فباضت</bdi>');
    assert.equal(f.C, 'past tense, feminine');
    assert.equal(f.D, 'fa-bāḍat');
    assert.equal(f.E, 'past tense, feminine, formal');
});

test('{ai-card} is the AI part alone by default, and also carries the dictionaries when chosen', async () => {
    const {options, notes} = await clientFixture(async () => { throw new Error('note construction must not use HTTP'); }, {DOMParser: FakeDOMParser});
    options.cardFormats = [{deck: 'D', model: 'Basic', fields: {Back: {value: '{ai-card}'}, Plain: {value: '{ai-card-plain}'}}}];
    const ctx = {word: 'run', sentence: 'I run daily.', context: 'We train. I run daily.', aiCard: {definition: 'move fast'},
        glossaries: [{name: 'Oxford', html: '<b>to move</b>'}], dictionaryOrder: ['IA', 'Oxford']};
    assert.equal(notes.normalizeAnkiOptions({}).aiCardMode, 'card');
    assert.equal(notes.normalizeAnkiOptions({aiCardMode: 'odd'}).aiCardMode, 'card');
    let built = await notes.buildNote(ctx, options, null);
    assert(!built.fields.Back.includes('to move') && !built.fields.Plain.includes('to move'), 'only the AI part by default');
    options.aiCardMode = 'dictionaries';
    built = await notes.buildNote(ctx, options, null);
    assert(built.fields.Back.includes('<div dir="auto">move fast</div><hr><div style="text-align: left;"><i>(Oxford)</i> <b>to move</b></div>'), built.fields.Back);
    assert(built.fields.Plain.endsWith('<br>(Oxford) to move'), built.fields.Plain);
    assert(!built.fields.Back.includes('train') && !built.fields.Back.includes('daily'), 'the context stays out either way');
    built = await notes.buildNote({...ctx, aiCard: {}}, options, null);
    assert(built.fields.Back.includes('to move'), 'without an AI card the dictionaries are still there');
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
        J: {value: '{ai-card}'},
        K: {value: '{ai-card-plain}'},
        L: {value: '{sentence}'},
    };
    const note = await notes.buildNote({
        word: 'run',
        glossaries: [{name: 'WordReference', html: 'correr'}, {name: oxford, html: '<b>to move</b>'}],
        sentence: 'I run every day.',
        aiCard: {definition: 'move fast', partOfSpeech: ['verb'], labels: ['informal'], ipa: 'rʌn', cefr: 'A1', translation: 'correr'},
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
    assert(f.J.startsWith('<div style="text-align: start;"><div dir="auto"><b>run</b> /rʌn/</div><div dir="auto"><i>A1, verb, informal</i></div><div dir="auto">correr</div><div dir="auto">move fast</div>'), 'the whole AI card');
    assert(f.J.includes('<strong>Run</strong>'), 'the AI card carries the explanation when it exists');
    assert(!f.J.includes('every day') && !f.K.includes('every day'), 'the context stays out of the AI card; it goes in {sentence}');
    assert.equal(f.K, 'run /rʌn/<br>(A1, verb, informal)<br>correr<br>move fast<br>Run means to move fast.<br>• one example');
    assert.equal(f.L, 'I run every day.');
    assert(notes.formatUsesMarker(options, 'ai-explanation'));
    assert.deepEqual([...notes.dictionaryMarkers(['WordReference']).slice(0, 2)], ['single-glossary-wordreference', 'single-glossary-wordreference-brief']);
    assert.deepEqual([...notes.glossaryDictionaryNames([
        {kind: 'ai-word', enabled: true, name: 'AI word in context'}, {kind: 'ai-context', enabled: true, name: 'x'},
        {kind: 'mdict', enabled: false, name: 'Off'}, {kind: 'mdict', enabled: true, name: oxford},
    ])], ['IA', oxford]);
});
