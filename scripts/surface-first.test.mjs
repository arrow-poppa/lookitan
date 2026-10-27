import assert from 'node:assert/strict';
import {readFile} from 'node:fs/promises';
import {resolve} from 'node:path';
import test from 'node:test';
import {audioSearchTerm, findAudio, wordAudio, wordAudioFile} from '../src/lib/audio.js';
import {cardHints, parseCard, sameWord, validDisplaySurface} from '../src/lib/ai/word-card.js';
import {cardFromLookup, noteContextFromCard, savedDisplaySurface} from '../src/lib/cards.js';

// Deterministic, with mocked sources: these check which spelling Lookitan asks each source for and
// in which order; they say nothing about which recordings Wiktionary or Lingua Libre really have.
const project = resolve(import.meta.dirname, '..');
/** The OCR languages Lookitan offers (src-tauri/src/ocr/mod.rs). */
const OCR_TAGS = [...(await readFile(resolve(project, 'src-tauri/src/ocr/mod.rs'), 'utf8')).matchAll(/LanguageModel \{ tag: "([^"]+)"/g)].map((m) => m[1]);

/** A form on screen and its dictionary form, for each OCR language. */
const SAMPLES = {
    ar: ['كتبوا', 'كتب'], bs: ['radili', 'raditi'], bg: ['къщи', 'къща'], zh: ['学习', '学习'], hr: ['kuće', 'kuća'], cs: ['domů', 'dům'],
    da: ['husene', 'hus'], nl: ['liep', 'lopen'], en: ['running', 'run'], fi: ['talossa', 'talo'], fr: ['allions', 'aller'], de: ['machte', 'machen'],
    el: ['ανθρώπου', 'άνθρωπος'], hu: ['házak', 'ház'], id: ['makanan', 'makan'], it: ['parlavo', 'parlare'], ja: ['進めて', '進める'],
    ko: ['종류이다', '종류'], la: ['amici', 'amicus'], nb: ['husene', 'hus'], pl: ['domach', 'dom'], pt: ['corações', 'coração'],
    ro: ['țării', 'țară'], ru: ['Шелестят', 'шелестеть'], sr: ['кућама', 'кућа'], sk: ['dĺžky', 'dĺžka'], sl: ['šoli', 'šola'],
    es: ['canciones', 'canción'], sv: ['husen', 'hus'], tr: ['ağaçlar', 'ağaç'], uk: ['країни', 'країна'],
};
const sample = (tag) => (tag === 'sr-Latn-RS' ? ['kućama', 'kuća'] : SAMPLES[tag.split('-')[0]]);

/** A fake source list: recordings for the terms in `has`, and a voice that reads anything. */
function sources(has, {voice = true} = {}) {
    const asked = [];
    const find = async (term) => {
        asked.push(term);
        const found = (has[term] || []).map((name) => ({kind: 'url', name, url: `https://audio.test/${encodeURIComponent(term)}/${name}`}));
        return voice ? [...found, {kind: 'tts', name: 'Voz do sistema'}] : found;
    };
    return {asked, find};
}

test('every OCR language: the word audio is asked for the form on screen first, its dictionary form after', async () => {
    assert.equal(OCR_TAGS.length, 38);
    assert.equal(new Set(OCR_TAGS.map((t) => t.split('-')[0])).size, 31);
    for (const tag of OCR_TAGS) {
        const [surface, lemma] = sample(tag);
        assert(surface && lemma, `${tag}: a sample`);
        const {asked, find} = sources(lemma === surface ? {[surface]: ['own']} : {[surface]: ['own'], [lemma]: ['lemma']});
        const entries = await wordAudio(surface, [lemma], tag, find);
        assert.equal(asked[0], surface, `${tag}: ${surface} first, spelled as read`);
        assert.equal(entries[0].name, 'own', `${tag}: its own recording plays first`);
        assert.equal(entries[0].term, surface);
        if (lemma !== surface) {
            const fallback = entries.find((e) => e.fallback);
            assert(fallback && fallback.name === `lemma · ${lemma}` && fallback.term === lemma, `${tag}: the dictionary form's recording is named as such`);
            assert(entries.indexOf(fallback) > entries.findIndex((e) => e.kind === 'tts'), `${tag}: after the voice reading the word itself`);
        }
        assert.equal(entries.filter((e) => e.kind === 'tts').length, 1, `${tag}: the voice reads only the word on screen`);
        assert.equal(validDisplaySurface({displaySurface: surface}, surface, tag), '', `${tag}: no display form equal to the word`);
    }
});

test('Шелестят: the title may be Шелестя́т, the audio is searched as Шелестят, never шелестеть first', async () => {
    assert.equal(validDisplaySurface({displaySurface: 'Шелестя́т'}, 'Шелестят', 'ru-RU'), 'Шелестя́т');
    assert.equal(validDisplaySurface({displaySurface: 'шелесте́ть'}, 'Шелестят', 'ru-RU'), '', 'the infinitive is no display form of the word');
    const {asked, find} = sources({шелестят: ['Wiktionary · user']}, {voice: false});
    const entries = await wordAudio('Шелестя́т', ['шелестеть', 'Шелестя́т'], 'ru-RU', find);
    assert.deepEqual(asked, ['Шелестят', 'шелестят', 'шелестеть'], 'stress mark off, then the sentence-initial capital, then the infinitive');
    assert.deepEqual(entries.map((e) => e.name), ['Wiktionary · user'], 'the conjugated form\'s recording, named as it is');
    const none = sources({шелестеть: ['Wiktionary · user']}, {voice: false});
    const fallback = await wordAudio('Шелестят', ['шелестеть'], 'ru-RU', none.find);
    assert.deepEqual(fallback.map((e) => [e.name, e.term, e.fallback]), [['Wiktionary · user · шелестеть', 'шелестеть', true]],
        'no recording of Шелестят: the infinitive\'s, labelled with the infinitive');
});

test('the primary audio target is the form on screen in every script', async () => {
    for (const [tag, surface, lemma] of [['ja-JP', '進めて', '進める'], ['ko-KR', '종류이다', '종류'], ['el-GR', 'ανθρώπου', 'άνθρωπος'],
        ['en-US', 'running', 'run'], ['de-DE', 'machte', 'ausmachen'], ['de-DE', 'Macht', 'machen']]) {
        const {asked, find} = sources({}, {voice: false});
        await wordAudio(surface, [lemma], tag, find);
        assert.equal(asked[0], surface, `${tag}: ${surface}, not ${lemma}`);
        assert.equal(asked.at(-1), lemma);
    }
});

test('the user\'s source order is kept, and each recording is listed once', async () => {
    const find = async (term) => (term === 'running'
        ? [{kind: 'db', name: 'Local', path: 'a.db', source: 's', file: 'f'}, {kind: 'url', name: 'Lingua Libre', url: 'u1'}, {kind: 'url', name: 'Wiktionary', url: 'u2'}, {kind: 'tts', name: 'Voz do sistema'}]
        : [{kind: 'url', name: 'Wiktionary', url: 'u2'}, {kind: 'url', name: 'Wiktionary', url: 'u3'}, {kind: 'edge', name: 'Edge', text: term, voice: '', language: 'en'}]);
    const entries = await wordAudio('running', ['run'], 'en-US', find);
    assert.deepEqual(entries.map((e) => e.name), ['Local', 'Lingua Libre', 'Wiktionary', 'Voz do sistema', 'Wiktionary · run'],
        'own in the configured order; a recording found twice once; no voice reading the dictionary form');
});

test('the search spelling drops only the learner\'s stress acute, in Russian, Ukrainian and Bulgarian', () => {
    assert.equal(audioSearchTerm('Шелестя́т', 'ru-RU'), 'Шелестят');
    assert.equal(audioSearchTerm('ёлка', 'ru-RU'), 'ёлка', 'ё kept');
    assert.equal(audioSearchTerm('мо́й', 'ru-RU'), 'мой', 'й kept');
    assert.equal(audioSearchTerm('краї́на', 'uk-UA'), 'країна', 'ї kept');
    assert.equal(audioSearchTerm('ґа́нок', 'uk-UA'), 'ґанок');
    assert.equal(audioSearchTerm('ѝ', 'bg-BG'), 'ѝ', 'a Bulgarian grave letter is spelling');
    assert.equal(audioSearchTerm('ανθρώπου'.normalize('NFD'), 'el-GR'), 'ανθρώπου'.normalize('NFC'), 'Greek tonos kept, NFC');
    assert.equal(audioSearchTerm('ΐ', 'el-GR'), 'ΐ');
    assert.equal(audioSearchTerm('كَتَبَ', 'ar-SA'), 'كَتَبَ'.normalize('NFC'), 'Arabic marks kept');
    for (const [w, tag] of [['café', 'fr-FR'], ['año', 'es-ES'], ['țară', 'ro-RO'], ['kůň', 'cs-CZ'], ['學習', 'zh-TW'], ['勉強', 'ja-JP'], ['종류', 'ko-KR'], ['ağaç', 'tr-TR']]) {
        assert.equal(audioSearchTerm(w, tag), w, `${tag}: ${w} as written`);
    }
    assert.equal(audioSearchTerm('amīcī', 'la'), 'amīcī', 'Latin macrons are not stress marks');
});

test('lowercasing follows the language: Turkish İ becomes i', async () => {
    const {asked, find} = sources({}, {voice: false});
    await wordAudio('İstanbul', [], 'tr-TR', find);
    assert.deepEqual(asked, ['İstanbul', 'istanbul']);
    assert(sameWord('Iğdır', 'ığdır', 'tr-TR'), 'Turkish I is ı');
    assert(sameWord('Шелестя́т', 'шелестят', 'ru-RU') && !sameWord('шелестят', 'шелестеть', 'ru-RU'));
    assert(!sameWord('ёлка', 'елка', 'ru-RU'), 'ё is not е');
});

test('{audio} of a saved word: the word as saved first, from the configured source, with its dictionary form as fallback', async () => {
    const asked = [];
    const io = {invoke: async (command, args) => {
        if (command === 'audio_lookup') { asked.push(args.term); return args.term === '進める' ? [{source: 's', file: 'f.ogg', name: 'Local'}] : []; }
        if (command === 'audio_data') { return ['T2dnUw==', 'ogg']; }
        return null;
    }};
    const options = {sources: [{type: 'local-db', path: 'C:/a.db'}, {type: 'tts'}]};
    const file = await wordAudioFile(options, '進めて', ['進める'], 'ja-JP', io);
    assert.deepEqual(asked, ['進めて', '進める']);
    assert.equal(file.extension, 'ogg');
    assert.equal(await wordAudioFile({sources: [{type: 'tts'}]}, '進めて', [], 'ja-JP', io), null, 'the system voice gives no file');
    assert.deepEqual((await findAudio(options, {term: '進めて', reading: '', language: 'ja', locale: 'ja-JP'}, io)).map((e) => e.kind), ['tts'], 'findAudio itself unchanged');
});

test('saved words keep the surface, its display form, the lemma and an audio target of the surface', async () => {
    const lookup = {payload: {word: 'Шелестят', sentence: 'Шелестят листья.', context: 'Шелестят листья.', contextAt: 0},
        aiCard: parseCard('{"headword":"шелестеть","displaySurface":"Шелестя́т","displayHeadword":"шелесте́ть"}')};
    const saved = {...cardFromLookup(lookup, {ocrLanguage: 'ru-RU'}), createdAt: 0};
    assert.deepEqual([saved.word, saved.displaySurface, saved.dictionaryForm, saved.displayHeadword], ['Шелестят', 'Шелестя́т', 'шелестеть', 'шелесте́ть']);
    assert.equal(savedDisplaySurface({...saved, word: 'Шумят'}), '', 'an edited word drops a stale display form');
    assert.equal(savedDisplaySurface({word: 'x'}), '', 'older words');
    const note = noteContextFromCard(saved, [], {audio: {sources: []}});
    assert.deepEqual([note.word, note.headword, note.displaySurface], ['Шелестят', 'шелестеть', 'Шелестя́т'], '{expression} stays the dictionary form');
    assert.equal(typeof note.audio, 'function');
    assert.equal(await note.audio(), null, 'no sources: no file, no error');
});

test('the AI hints read the lemmas from their own field, the form on screen leading the candidates', () => {
    const hint = cardHints({word: '進めて', context: '計画を進めていった。', contextAt: 3, candidates: ['進めて', '進める', '進め', '進'], lemmas: ['進める'], analyzer: 'sudachi'});
    assert(hint.includes('(Sudachi): "進める"') && !hint.includes('"進め"'), 'the lemma, not the prefixes');
    const older = cardHints({word: '進めて', context: '計画を進めていった。', contextAt: 3, candidates: ['進める', '進めて', '進'], analyzer: 'sudachi'});
    assert(older.includes('"進める"'), 'an older payload, lemmas first, still works');
    assert.equal(cardHints({word: 'run', context: 'I run.', contextAt: 2, candidates: ['run'], lemmas: [], analyzer: 'generic'}), '');
});
