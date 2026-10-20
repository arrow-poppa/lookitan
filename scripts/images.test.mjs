import assert from 'node:assert/strict';
import {readFile} from 'node:fs/promises';
import {resolve} from 'node:path';
import test from 'node:test';
import {createContext, SourceTextModule, SyntheticModule} from 'node:vm';
import {defaultAiOptions, normalizeAiOptions} from '../src/lib/ai/defaults.js';
import {languageName} from '../src/lib/ai/word-card.js';
import {IMAGE_COUNT, IMAGE_QUERY_PROMPT, IMAGE_QUERY_SYSTEM_PROMPT, bingUrl, cleanQuery, imagePrompts, parseBingImages, searchImages} from '../src/lib/images.js';

const project = resolve(import.meta.dirname, '..');

/** One Bing Images result as `/images/async` writes it: JSON in an HTML-escaped `m` attribute. */
function result(d) {
    const json = JSON.stringify(d).replace(/&/g, '&amp;').replace(/"/g, '&quot;');
    return `<div class="imgpt"><a class="iusc" style="height:180px;width:310px" m="${json}" href="/images/search"><img src="x"></a></div>`;
}

test('Bing results give the picture, its thumbnail, title and page, each picture once', () => {
    const html = [
        result({murl: 'https://a.example/cat.jpg?w=1&h=2', turl: 'https://ts1.mm.bing.net/th?id=OIP.a&pid=15.1', t: 'A "funny" cat', purl: 'https://a.example/page'}),
        result({murl: 'https://a.example/cat.jpg?w=1&h=2', turl: 'https://ts1.mm.bing.net/th?id=OIP.dup', t: 'again'}),
        result({murl: 'data:image/png;base64,AAAA', turl: 'https://ts1.mm.bing.net/th?id=OIP.data'}),
        result({murl: 'https://b.example/dog.png', t: 'no thumbnail'}),
    ].join('');
    assert.deepEqual(parseBingImages(html), [
        {src: 'https://a.example/cat.jpg?w=1&h=2', thumb: 'https://ts1.mm.bing.net/th?id=OIP.a&pid=15.1', title: 'A "funny" cat', page: 'https://a.example/page'},
        {src: 'https://b.example/dog.png', thumb: 'https://b.example/dog.png', title: 'no thumbnail', page: ''},
    ]);
});

test('a page without result attributes is read the way Migaku reads it, from murl alone', () => {
    const html = 'x murl&quot;:&quot;https://c.example/a.jpg&quot;,&quot;turl y murl&quot;:&quot;https://c.example/b.jpg&quot;';
    assert.deepEqual(parseBingImages(html).map((i) => i.src), ['https://c.example/a.jpg', 'https://c.example/b.jpg']);
    assert.deepEqual(parseBingImages('<html>nothing</html>'), []);
});

test('the search asks Bing with SafeSearch strict and keeps the first ten pictures', async () => {
    const html = Array.from({length: 14}, (_v, i) => result({murl: `https://p.example/${i}.jpg`, turl: `https://ts.example/${i}`})).join('');
    let asked;
    const items = await searchImages('to run & jump', {fetch: async (url, init) => {
        asked = {url, init};
        return {ok: true, status: 200, text: async () => html};
    }});
    assert.equal(asked.url, bingUrl('to run & jump'));
    assert(asked.url.startsWith('https://www.bing.com/images/async?q=to%20run%20%26%20jump&'));
    assert(asked.url.includes('adlt=strict'));
    assert.equal(asked.init.method, 'GET');
    assert.equal(items.length, IMAGE_COUNT);
    assert.equal(items[0].src, 'https://p.example/0.jpg');
    await assert.rejects(searchImages('x', {fetch: async () => ({ok: false, status: 429, text: async () => ''})}), /429/);    let calls = 0;
    const retried = await searchImages('x', {fetch: async () => ({ok: true, status: 200, text: async () => (++calls === 1 ? '<html></html>' : html)})});
    assert.equal(calls, 2, 'an empty answer is asked once more');
    assert.equal(retried.length, IMAGE_COUNT);
    calls = 0;
    assert.deepEqual(await searchImages('x', {fetch: async () => { calls++; return {ok: true, status: 200, text: async () => ''}; }}), []);
    assert.equal(calls, 2, 'but only once');
});

test('the AI answer becomes a clean query, and an unusable one falls back to the word', () => {
    assert.equal(cleanQuery('"to run"\n', 'correr'), 'to run');
    assert.equal(cleanQuery('Query: funny cat.', 'x'), 'funny cat');
    assert.equal(cleanQuery('\n\n  serendipity  \nbecause…', 'x'), 'serendipity');
    assert.equal(cleanQuery('', 'gato'), 'gato');
    assert.equal(cleanQuery('a'.repeat(200), 'gato'), 'gato');
});

test('the image query has its own system and user prompts, with the word language filled in', () => {
    const settings = {ocrLanguage: 'ja-JP', translationLanguage: 'pt-BR', definitionLanguage: 'en-US'};
    const defaults = imagePrompts(defaultAiOptions(), settings);
    assert.equal(defaults.system, IMAGE_QUERY_SYSTEM_PROMPT);
    assert.equal(defaults.user, IMAGE_QUERY_PROMPT.replace('{{wordLanguage}}', languageName('ja-JP')));
    assert(defaults.user.includes('{{target}}') && defaults.user.includes('{{sentence}}'), 'the provider fills the word and the sentence');
    const own = imagePrompts({imageQuerySystemPrompt: 'Answer in {{translationLanguage}}.', imageQueryPrompt: '[{{target}}] {{wordLanguage}}'}, settings);
    assert.deepEqual(own, {system: `Answer in ${languageName('pt-BR')}.`, user: `[{{target}}] ${languageName('ja-JP')}`});
});

test('image options default to on-click pictures with an AI query, and empty prompts restore the defaults', () => {
    const base = defaultAiOptions();
    assert.equal(base.autoShowImages, false);
    assert.equal(base.imageQueryUseAi, true);
    const old = normalizeAiOptions({provider: 'gemini'});
    assert.equal(old.autoShowImages, false, 'settings saved before the images dictionary show pictures on click');
    assert.equal(old.imageQuerySystemPrompt, IMAGE_QUERY_SYSTEM_PROMPT);
    const saved = normalizeAiOptions({autoShowImages: true, imageQueryUseAi: false, imageQueryPrompt: '  ', imageQuerySystemPrompt: 'mine'});
    assert.equal(saved.autoShowImages, true);
    assert.equal(saved.imageQueryUseAi, false);
    assert.equal(saved.imageQueryPrompt, IMAGE_QUERY_PROMPT);
    assert.equal(saved.imageQuerySystemPrompt, 'mine');
    assert.equal(normalizeAiOptions({autoShowImages: 'yes'}).autoShowImages, false);
});

test('{image} uploads the checked pictures to Anki, in order, only when a field uses it', async () => {
    const context = createContext({AbortController, setTimeout, clearTimeout});
    const modules = new Map();
    async function load(id) {
        if (!modules.has(id)) {
            modules.set(id, (async () => (id === '@tauri-apps/plugin-http' ? new SyntheticModule(['fetch'], function () {
                this.setExport('fetch', async () => { throw new Error('note construction must not use HTTP'); });
            }, {context, identifier: id}) : new SourceTextModule(await readFile(id, 'utf8'), {context, identifier: id})))());
        }
        return modules.get(id);
    }
    const module = await load(resolve(project, 'src/lib/anki/notes.js'));
    await module.link((specifier, owner) => load(specifier.startsWith('.') ? resolve(owner.identifier, '..', specifier) : specifier));
    await module.evaluate();
    const notes = module.namespace;
    const options = notes.defaultAnkiOptions();
    options.cardFormats = [{deck: 'D', model: 'Basic', fields: {Front: {value: '{expression}'}, Picture: {value: '{image}'}}}];
    const stored = [];
    const conn = {storeMediaFile: async (name, data) => { stored.push({name, data}); return name; }};
    let asked = 0;
    const images = async () => { asked++; return [{data: 'AAA', extension: 'jpg'}, {data: 'BBB', extension: 'png'}]; };
    const built = await notes.buildNote({word: 'cat', images}, options, conn);
    assert.equal(stored.length, 2);
    assert.match(stored[0].name, /^lookitan_image_1_.+\.jpg$/);
    assert.match(stored[1].name, /^lookitan_image_2_.+\.png$/);
    assert.deepEqual(stored.map((s) => s.data), ['AAA', 'BBB']);
    assert.equal(built.fields.Picture, `<img src="${stored[0].name}"><img src="${stored[1].name}">`);
    const check = await notes.buildNote({word: 'cat', images}, options, null);
    assert.equal(check.fields.Picture, '', 'a duplicate check builds the note without downloading pictures');
    options.cardFormats[0].fields = {Front: {value: '{expression}'}};
    await notes.buildNote({word: 'cat', images}, options, conn);
    assert.equal(asked, 1, 'pictures are fetched only for a field with {image}');
    assert(notes.MARKERS.includes('image'));
    assert.equal(notes.suggestMarker('Picture', 3), '{image}');
    assert.equal(notes.suggestMarker('Screenshot', 3), '{screenshot}');
});

test('meme mode puts " meme" at the end of the search, as Migaku does', async () => {
    const {withMemeMode} = await import('../src/lib/images.js');
    assert.equal(withMemeMode('to run', true), 'to run meme');
    assert.equal(withMemeMode('to run', false), 'to run');
});

test('word audio sources: Yomitan URL templates, the JSON list, order and defaults', async () => {
    const {defaultAudioOptions, fillAudioUrl, findAudio, normalizeAudioOptions, parseAudioList} = await import('../src/lib/audio.js');
    assert.deepEqual(defaultAudioOptions().sources.map((s) => s.type), ['tts'], 'the system voice until something else is set');
    assert.equal(fillAudioUrl('http://x/?t={term}&r={reading}&l={language}', {term: '読む', reading: 'よむ', language: 'ja'}),
        `http://x/?t=${encodeURIComponent('読む')}&r=${encodeURIComponent('よむ')}&l=ja`);
    assert.equal(fillAudioUrl('{reading}', {term: 'run'}), 'run', 'no reading: the term');
    assert.deepEqual(parseAudioList({type: 'audioSourceList', audioSources: [{name: 'NHK16', url: 'http://127.0.0.1:5050/nhk16/a.ogg'}, {url: 'file:///etc/passwd'}, {url: 'https://b/c.mp3'}]}),
        [{name: 'NHK16', url: 'http://127.0.0.1:5050/nhk16/a.ogg'}, {name: '', url: 'https://b/c.mp3'}]);
    assert.deepEqual(parseAudioList({type: 'other'}), []);
    assert.deepEqual(normalizeAudioOptions({sources: [{type: 'bogus'}, {type: 'custom', url: 'u', enabled: false}]}).sources,
        [{type: 'custom', enabled: false, path: '', url: 'u', subsources: []}]);
    const options = {sources: [
        {type: 'custom-json', url: 'http://127.0.0.1:5050/?term={term}'},
        {type: 'local-db', path: 'a.db'},
        {type: 'custom', url: 'https://audio.example/{term}.mp3', enabled: false},
        {type: 'custom', url: 'https://audio.example/{term}.mp3'},
        {type: 'tts'},
    ]};
    const io = {
        invoke: async (command) => { if (command === 'audio_lookup') { throw new Error('no such file'); } return null; },
        fetch: async (url) => ({ok: true, json: async () => ({type: 'audioSourceList', audioSources: [{name: 'Forvo', url: `${url}&file=1`}]})}),
    };
    const found = await findAudio(options, {term: 'run', reading: '', language: 'en'}, io);
    assert.deepEqual(found.map((f) => [f.kind, f.name]), [['url', 'Forvo'], ['url', 'audio.example'], ['tts', 'Voz do sistema']],
        'in source order; a failing database and a disabled source are skipped');
});

test('a local database keeps its own source order: kept in place, new ones added, gone ones dropped', async () => {
    const {findAudio, mergeSubsources, normalizeAudioOptions} = await import('../src/lib/audio.js');
    assert.deepEqual(mergeSubsources([{name: 'forvo', enabled: true}, {name: 'gone', enabled: true}, {name: 'nhk16', enabled: false}], ['nhk16', 'forvo', 'jpod']),
        [{name: 'forvo', enabled: true}, {name: 'nhk16', enabled: false}, {name: 'jpod', enabled: true}]);
    assert.equal(normalizeAudioOptions({others: 'buttons'}).others, 'buttons');
    assert.equal(normalizeAudioOptions({others: 'x'}).others, 'menu');
    const calls = [];
    const io = {invoke: async (command, args) => { calls.push(args); return []; }};
    await findAudio({sources: [{type: 'local-db', path: 'a.db', subsources: [{name: 'forvo', enabled: true}, {name: 'nhk16', enabled: false}, {name: 'jpod', enabled: true}]}]}, {term: 'x'}, io);
    assert.deepEqual(calls[0].order, ['forvo', 'jpod'], 'the enabled sources, in the user order');
    await findAudio({sources: [{type: 'local-db', path: 'a.db'}]}, {term: 'x'}, io);
    assert.equal(calls[1].order, null, 'not set yet: all sources, default order');
    await findAudio({sources: [{type: 'local-db', path: 'a.db', subsources: [{name: 'forvo', enabled: false}]}]}, {term: 'x'}, io);
    assert.equal(calls.length, 2, 'every source switched off: the database is not asked');
});
