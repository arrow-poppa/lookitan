import assert from 'node:assert/strict';
import {readFile} from 'node:fs/promises';
import {resolve} from 'node:path';
import test from 'node:test';
import {AIProvider} from '../src/lib/ai/ai-provider.js';
import {normalizeAiOptions} from '../src/lib/ai/defaults.js';
import {locateSelection} from '../src/lib/ai/selection.js';
import {CARD_PROMPT, CONTEXT_CARD_PROMPT, DISPLAY_FIELD, EXPRESSIONS_PROMPT, cardPrompt, headwordDisplayInstruction, parseCard, parseCardProgress,
    validDisplayHeadword} from '../src/lib/ai/word-card.js';
import {cardFromLookup, noteContextFromCard, savedDisplayHeadword, toCsv} from '../src/lib/cards.js';

// Deterministic: these check what Lookitan asks and what it accepts from a reply written by hand;
// they say nothing about how often a real model marks stress correctly.
const project = resolve(import.meta.dirname, '..');
/** The OCR languages Lookitan offers (src-tauri/src/ocr/mod.rs). */
const OCR_TAGS = [...(await readFile(resolve(project, 'src-tauri/src/ocr/mod.rs'), 'utf8')).matchAll(/LanguageModel \{ tag: "([^"]+)"/g)].map((m) => m[1]);
const show = (headword, displayHeadword, tag) => validDisplayHeadword({headword, displayHeadword}, tag);
const nfd = (s) => s.normalize('NFD');

test('the display form is asked only where learners\' dictionaries mark what the spelling leaves out', () => {
    assert(OCR_TAGS.length >= 38 && OCR_TAGS.includes('sr-Cyrl-RS') && OCR_TAGS.includes('la'), 'the real OCR list');
    const asked = OCR_TAGS.filter((tag) => headwordDisplayInstruction(tag));
    assert.deepEqual(asked.sort(), ['bg-BG', 'la', 'ru-RU', 'uk-UA']);
    for (const tag of ['ru-RU', 'uk-UA', 'bg-BG']) {
        const text = headwordDisplayInstruction(tag);
        assert(text.startsWith(', "displaySurface": the word exactly as written in the context, letter for letter,') && text.includes('"displayHeadword": ') &&
            text.includes('acute (U+0301)') && text.includes('not the stress of the form in the text') && text.includes('"" for either when you are not sure'), tag);
    }
    assert(headwordDisplayInstruction('ru-RU').includes('none on ё') && !headwordDisplayInstruction('uk-UA').includes('ё'), 'ё is Russian');
    assert(headwordDisplayInstruction('la').includes('macrons on its long vowels') && !headwordDisplayInstruction('la').includes('stress'),
        'Latin macrons are length, not stress');
    for (const tag of ['el-GR', 'ar-SA', 'zh-CN', 'zh-TW', 'ja-JP', 'ko-KR', 'cs-CZ', 'hu-HU', 'sr-Cyrl-RS', 'sr-Latn-RS', 'tr-TR', 'grc']) {
        assert.equal(headwordDisplayInstruction(tag), '', `${tag}: nothing asked`);
    }
});

test('the card prompt: one key in four languages, the headword\'s own spelling in all, cached per language', () => {
    for (const tag of OCR_TAGS) {
        const prompt = cardPrompt(CARD_PROMPT, {ocrLanguage: tag}, {});
        assert(!prompt.includes(DISPLAY_FIELD), `${tag}: marker filled`);
        assert(prompt.includes('spelled as dictionaries of the language spell it, with the letters and marks of that spelling and no added stress or length marks'), tag);
        assert.equal(prompt.split('"displayHeadword"').length, headwordDisplayInstruction(tag) ? 2 : 1, `${tag}: the key at most once`);
    }
    const ru = cardPrompt(CARD_PROMPT, {ocrLanguage: 'ru-RU'}, {});
    assert(ru.indexOf('"displayHeadword"') < ru.indexOf('"ipa"'), 'right after the headword, so the title is ready early');
    assert.equal(EXPRESSIONS_PROMPT.match(/^\d\. Context/gm).length, 5, 'the five examples unchanged');
    const split = (word, context) => Object.create(AIProvider.prototype)._splitPrompts(word, context, ru, '', undefined);
    const [a, b] = [split('дома́', 'Мы дома́.'), split('книги', 'Это книги.')];
    assert.equal(a.systemPrompt, b.systemPrompt, 'the same cached prefix for every Russian lookup');
    assert(a.systemPrompt.includes('"displayHeadword"') && !a.prompt.includes('displayHeadword'));
    const custom = 'Word: {{target}}\nGive "headword" and "displayHeadword" with stress.';
    assert.equal(cardPrompt(custom, {ocrLanguage: 'ru-RU'}, {}).split('displayHeadword').length, 2, 'a custom prompt that asks for it is not given ours too');
    assert(!cardPrompt('Word: {{target}}\nGive "headword".', {ocrLanguage: 'ru-RU'}, {}).includes('displayHeadword'), 'nor one without the marker');
});

test('the previous default moves to the new one; edited and emptied prompts stay', () => {
    assert.equal(normalizeAiOptions({cardPrompt: CONTEXT_CARD_PROMPT, cardPromptVersion: 3}).cardPrompt, CARD_PROMPT);
    const edited = `${CONTEXT_CARD_PROMPT} Be brief.`;
    assert.equal(normalizeAiOptions({cardPrompt: edited, cardPromptVersion: 3}).cardPrompt, edited, 'no marker forced into an edited prompt');
    assert.equal(normalizeAiOptions({cardPrompt: '', cardPromptVersion: 3}).cardPrompt, '');
    assert.equal(normalizeAiOptions({}).cardPrompt, CARD_PROMPT, '"Restaurar padrão" and a missing setting give it');
    assert.equal(cardPrompt('', {ocrLanguage: 'ru-RU'}, {}), '', 'an emptied card prompt asks nothing');
});

test('Russian: the lemma\'s own stress, never the surface\'s; ё and й intact; anything else falls back', () => {
    assert.equal(show('дом', 'до́м', 'ru-RU'), 'до́м');
    assert.equal(show('дом', 'дома́', 'ru-RU'), '', 'the surface дома́ is not the lemma');
    assert.equal(show('дом', 'до́м'.normalize('NFC'), 'ru-RU'), 'до́м', 'U+0301 has no precomposed Cyrillic form');
    assert.equal(show('рука', 'рука́', 'ru-RU'), 'рука́');
    for (const [display, why] of [['д́ом', 'on a consonant'], ['до́́м', 'two marks'], ['до́н', 'a changed letter'], ['до̀м', 'a grave'],
        ['дóм', 'a Latin o'], ['до́ма', 'an added letter'], ['', 'empty'], ['дом', 'no mark']]) {
        assert.equal(show('дом', display, 'ru-RU'), '', why);
    }
    assert.equal(show('ёлка', 'ё́лка', 'ru-RU'), '', 'ё is always stressed: no acute on it');
    assert.equal(show('ёлка', 'елка', 'ru-RU'), '', 'ё is not stripped');
    assert.equal(show('мой', 'мо́й', 'ru-RU'), 'мо́й', 'й kept');
    assert.equal(show('мой', 'мои́', 'ru-RU'), '', 'й is no и with a mark');
    assert.equal(show('бить баклуши', 'би́ть баклу́ши', 'ru-RU'), 'би́ть баклу́ши', 'one stress per word');
    assert.equal(show('до́м', 'до́м', 'ru-RU'), '', 'a stressed headword is no lookup form: no display either');
    assert.equal(show('Москва', 'Москва́', 'ru-RU'), 'Москва́');
});

test('Ukrainian and Bulgarian: their own letters and stress', () => {
    assert.equal(show('країна', 'краї́на', 'uk-UA'), 'краї́на', 'stress on ї');
    assert.equal(show('ґанок', 'ґа́нок', 'uk-UA'), 'ґа́нок');
    assert.equal(show('єдність', 'є́дність', 'uk-UA'), 'є́дність');
    assert.equal(show('країна', 'страна́', 'uk-UA'), '', 'no Russian word in its place');
    assert.equal(show('кіт', 'ки́т', 'uk-UA'), '', 'і is not и');
    assert.equal(show('країна', '', 'uk-UA'), '', 'unknown stress: the headword');
    assert.equal(show('къща', 'къ́ща', 'bg-BG'), 'къ́ща');
    assert.equal(show('къща', 'кꙑща', 'bg-BG'), '');
    assert.equal(show('нея', 'не́я', 'bg-BG'), 'не́я');
    assert.equal(show('объект', 'объ́ект', 'ru-RU'), '', 'Russian ъ is a sign, not a vowel');
    assert.equal(show('кит', 'ки́т', 'uk-UA'), 'ки́т', 'Ukrainian и');
});

test('Greek: tonos and dialytika are spelling, kept in the headword; no display form, no polytonic', () => {
    const card = parseCard(`{"headword":"${nfd('άνθρωπος')}","displayHeadword":"ἄνθρωπος"}`);
    assert.equal(card.headword, 'άνθρωπος'.normalize('NFC'), 'NFC, tonos kept');
    assert.equal(validDisplayHeadword(card, 'el-GR'), '', 'not shown: no monotonic-to-polytonic change');
    assert.equal(parseCard(`{"headword":"${nfd('ΐ')}"}`).headword, 'ΐ', 'dialytika and tonos on one letter');
    assert.equal(show('καφές', 'καφές', 'el-GR'), '');
    assert(!cardPrompt(CARD_PROMPT, {ocrLanguage: 'el-GR'}, {}).includes('"displayHeadword"'));
});

test('Ancient Greek Unicode survives: breathing, circumflex and iota subscript, precomposed or not', () => {
    const word = 'ἀρχῇ';
    for (const form of [word.normalize('NFC'), word.normalize('NFD')]) {
        const card = parseCard(JSON.stringify({headword: 'ἀρχή', displayHeadword: form}));
        assert.equal(card.displayHeadword, word.normalize('NFC'));
        const marks = [...card.displayHeadword.normalize('NFD')].filter((c) => /\p{M}/u.test(c));
        assert.deepEqual(marks, ['̓', '͂', 'ͅ'], 'smooth breathing, circumflex, iota subscript');
    }
    assert.equal(parseCard('{"headword":"ἀρχή"}').headword.normalize('NFD').includes('̓'), true, 'the lemma keeps its breathing');
    assert.notEqual(parseCard('{"headword":"ἀρχή"}').headword, 'αρχη', 'never stripped');
    assert.equal(show('ἀρχή', 'ἀρχή', 'grc'), '', 'no Ancient Greek option yet: nothing claimed');
});

test('Latin: macrons for long vowels only when given, and only on vowels', () => {
    assert.equal(show('amicus', 'amīcus', 'la'), 'amīcus');
    assert.equal(show('amicus', 'amīcus', 'la'), 'amīcus', 'decomposed: NFC');
    assert.equal(show('amicus', 'amícus', 'la'), '', 'an acute is no length mark');
    assert.equal(show('amicus', 'am̄icus', 'la'), '', 'on a consonant');
    assert.equal(show('amicus', 'amīca', 'la'), '', 'another word');
});

test('Latin-script, CJK, Korean and Arabic headwords keep their spelling; no display form is taken', () => {
    const kept = {
        'pt-BR': ['coração', 'público'], 'es-ES': ['canción', 'año'], 'fr-FR': ['école', 'garçon'], 'it-IT': ['perché', 'città'],
        'ro-RO': ['română', 'țară', 'școală'], 'cs-CZ': ['práce', 'kůň'], 'sk-SK': ['dĺžka', 'ľad'], 'hu-HU': ['tűz', 'kő'],
        'pl-PL': ['źródło', 'łódź'], 'hr-HR': ['čađa'], 'sr-Latn-RS': ['đak'], 'sr-Cyrl-RS': ['ђак', 'љубав'], 'sl-SI': ['šola'],
        'de-DE': ['Straße', 'Mädchen'], 'sv-SE': ['år', 'öl'], 'da-DK': ['ø', 'æble'], 'nb-NO': ['blåbær'], 'fi-FI': ['äiti'],
        'tr-TR': ['ışık', 'İstanbul', 'ağaç'], 'id-ID': ['rumah'], 'nl-NL': ['geïnteresseerd'], 'en-US': ['café'],
        'zh-CN': ['学习'], 'zh-TW': ['學習'], 'ja-JP': ['勉強'], 'ko-KR': ['종류'], 'ar-SA': ['كِتَاب', 'كتاب'],
    };
    for (const [tag, words] of Object.entries(kept)) {
        for (const w of words) {
            const card = parseCard(JSON.stringify({headword: nfd(w), displayHeadword: `${w}́`}));
            assert.equal(card.headword, w.normalize('NFC'), `${tag} ${w}: kept as written`);
            assert.equal(validDisplayHeadword(card, tag), '', `${tag} ${w}: no stress mark added`);
        }
    }
    assert.equal(parseCard('{"headword":"İstanbul"}').headword.toLowerCase() === 'i̇stanbul', true, 'no case folding of the headword');
    const zh = parseCard('{"headword":"学习","reading":"xuéxí"}');
    assert.deepEqual([zh.headword, zh.reading, zh.displayHeadword], ['学习', 'xuéxí', ''], 'the pinyin stays the reading');
    const ar = parseCard(JSON.stringify({headword: 'كَتَبَ'}));
    assert.equal([...ar.headword.normalize('NFD')].filter((c) => /\p{M}/u.test(c)).length, 3, 'its harakat kept, none invented');
});

test('streaming: a half-written display form is never shown', () => {
    const full = '{"headword":"дом","displayHeadword":"до́м","definition":"house"}';
    for (const cut of [full.indexOf('до́м') + 1, full.indexOf('до́м') + 2, full.indexOf('до́м') + 3]) {
        const card = parseCardProgress(full.slice(0, cut));
        assert(['', 'до́м'].includes(validDisplayHeadword(card, 'ru-RU')), `cut at ${cut}`);
    }
    assert.equal(validDisplayHeadword(parseCardProgress(full.slice(0, full.indexOf('до́м') + 2)), 'ru-RU'), '', '"до" is not shown');
    assert.equal(parseCard('{"headword":"дом"}').displayHeadword, '', 'older replies have none');
});

test('saved words keep the display form apart from the dictionary form; lookups, cloze and selection are untouched', () => {
    const context = 'Мы были дома́ весь день.';
    const lookup = {payload: {word: 'дома́', sentence: context, context, contextAt: 8}, aiCard: parseCard('{"headword":"дом","displayHeadword":"до́м"}')};
    const saved = {...cardFromLookup(lookup, {ocrLanguage: 'ru-RU'}), createdAt: 0};
    assert.deepEqual([saved.word, saved.dictionaryForm, saved.displayHeadword], ['дома́', 'дом', 'до́м'], 'the word on screen as read');
    assert.equal(savedDisplayHeadword(saved), 'до́м');
    assert.equal(savedDisplayHeadword({...saved, dictionaryForm: 'дама'}), '', 'an edited dictionary form drops a stale display form');
    assert.equal(savedDisplayHeadword({word: 'x', dictionaryForm: 'x'}), '', 'older words');
    const note = noteContextFromCard(saved);
    assert.deepEqual([note.headword, note.displayHeadword], ['дом', 'до́м']);
    assert.deepEqual(note.clozeAt, [8, 13], 'the cloze is the word in the context, not the display form');
    assert.equal(locateSelection(context, 8, 'дома́', {parts: ['до́м']}), null, 'a display form is no selection');
    const csv = toCsv([saved]).split('\r\n');
    assert(csv[0].endsWith(',Forma de dicionário exibida,Palavra exibida') && csv[1].endsWith(',до́м,'), 'new last columns; the others stay where they were');
    assert(csv[0].startsWith('Palavra,Forma de dicionário,'));
    assert.equal(cardFromLookup({payload: {word: 'house'}, aiCard: parseCard('{"headword":"house","displayHeadword":"hóuse"}')}, {ocrLanguage: 'en-US'}).displayHeadword, '');
});
