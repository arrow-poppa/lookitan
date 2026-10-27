import assert from 'node:assert/strict';
import test from 'node:test';
import {normalizeAiOptions} from '../src/lib/ai/defaults.js';
import {CARD_PROMPT, LEGACY_CARD_PROMPT, LEGACY_OCR_ERRORS_PROMPT, LEGACY_WORD_TRANSLATION_PROMPT, OCR_ERRORS_PROMPT, WORD_TRANSLATION_PROMPT,
    cardPrompt, ocrErrorsFor, parseCard, parseCardProgress, translationText} from '../src/lib/ai/word-card.js';
import {cardFromLookup, noteContextFromCard, toCsv, toQuizlet} from '../src/lib/cards.js';

const reply = (alternatives) => parseCard(JSON.stringify({headword: 'finish', translation: 'terminou', translationAlternatives: alternatives}));

test('the card keeps one to three translations, the main one first', () => {
    assert.deepEqual(reply(undefined).translationAlternatives, [], 'old replies have none');
    assert.equal(translationText(reply(undefined)), 'terminou', 'one: as before');
    assert.equal(translationText(reply(['concluiu'])), 'terminou, concluiu');
    assert.equal(translationText(reply(['concluiu', 'acabou'])), 'terminou, concluiu, acabou');
    assert.deepEqual(reply(['concluiu', 'acabou', 'finalizou']).translationAlternatives, ['concluiu', 'acabou'], 'at most two besides the main one');
    assert.deepEqual(reply([]).translationAlternatives, []);
    assert.equal(typeof reply(['concluiu']).translation, 'string', 'translation stays a string');
});

test('alternatives that are malformed, repeated or too long are dropped or cut', () => {
    for (const bad of ['concluiu', 3, null, {a: 1}]) { assert.deepEqual(reply(bad).translationAlternatives, [], JSON.stringify(bad)); }
    assert.deepEqual(reply([3, null, {t: 'x'}, ['y'], '', '  ', 'concluiu']).translationAlternatives, ['concluiu'], 'strings only');
    assert.deepEqual(reply(['Terminou', ' terminou ', 'con  cluiu', 'Con cluiu']).translationAlternatives, ['con cluiu'],
        'never the main one again, nor twice, case and spacing aside');
    const long = reply(['a'.repeat(200)]).translationAlternatives[0];
    assert.equal(long.length, 80);
    assert.equal(parseCard(JSON.stringify({translation: `x${' '.repeat(5)}y\n`})).translation, 'x y');
    const ar = parseCard(JSON.stringify({translation: 'أطفأ', translationAlternatives: ['أغلق', '𠮷野']}));
    assert.equal(translationText(ar), 'أطفأ, أغلق, 𠮷野', 'right-to-left and astral text kept as written');
    assert.equal(translationText({translation: '', translationAlternatives: ['b']}), 'b');
    assert.equal(translationText(null), '');
});

test('alternatives arriving while streaming are read as far as they came', () => {
    const partial = parseCardProgress('{"headword":"finish","translation":"terminou","translationAlternatives":["conclu');
    assert.equal(translationText(partial), 'terminou, conclu');
});

test('the prompt asks for alternatives with the translation, only once, and not when the translation is off', () => {
    const settings = {ocrLanguage: 'en-US', translationLanguage: 'pt-BR'};
    const prompt = cardPrompt(CARD_PROMPT, settings, {});
    assert(prompt.includes('"translationAlternatives": [up to two other natural translations into Brazilian Portuguese'));
    assert(prompt.indexOf('"translation"') < prompt.indexOf('"translationAlternatives"'));
    assert.equal(prompt.split('"translationAlternatives"').length, 2);
    assert(!cardPrompt(CARD_PROMPT, {...settings, showWordTranslation: false}, {}).includes('translation'), 'off: no translation key at all');
    const mine = cardPrompt(CARD_PROMPT, settings, {wordTranslationPrompt: 'one word in {{translationLanguage}}'});
    assert(mine.includes('"translation": one word in Brazilian Portuguese, "translationAlternatives"'), 'the user\'s translation prompt is kept');
    const own = `${CARD_PROMPT.slice(0, -1)}, "translationAlternatives": my own}`;
    assert.equal(cardPrompt(own, settings, {}).split('"translationAlternatives"').length, 2, 'a prompt asking for them itself is not asked twice');
});

test('every key of the default card describes the unit; the form stays the word on screen', () => {
    const prompt = cardPrompt(CARD_PROMPT, {ocrLanguage: 'en-US'}, {});
    for (const part of ['"headword": the dictionary form of what you describe', '"ipa": IPA transcription of the headword',
        'synonyms of the headword in this sense', 'of what you describe, as it is meant in this context']) {
        assert(prompt.includes(part), part);
    }
    assert(prompt.includes('"form": [the grammatical features of the word as written in the context'));
    assert(!prompt.includes('of the word in this sense') && !prompt.includes('"headword": the dictionary form of the word'));
});

test('prompts still on an earlier default move to the new one; edited ones stay', () => {
    const old = normalizeAiOptions({cardPrompt: LEGACY_CARD_PROMPT, wordTranslationPrompt: LEGACY_WORD_TRANSLATION_PROMPT, ocrErrorsPrompt: LEGACY_OCR_ERRORS_PROMPT, cardPromptVersion: 3});
    assert.equal(old.cardPrompt, CARD_PROMPT);
    assert.equal(old.wordTranslationPrompt, WORD_TRANSLATION_PROMPT);
    assert.equal(old.ocrErrorsPrompt, OCR_ERRORS_PROMPT);
    const edited = normalizeAiOptions({cardPrompt: `${LEGACY_CARD_PROMPT} Be brief.`, wordTranslationPrompt: 'mine', ocrErrorsPrompt: 'my OCR note', cardPromptVersion: 3});
    assert.deepEqual([edited.cardPrompt, edited.wordTranslationPrompt, edited.ocrErrorsPrompt], [`${LEGACY_CARD_PROMPT} Be brief.`, 'mine', 'my OCR note']);
});

test('saved words, their Anki notes and the exports keep every translation; old cards still work', () => {
    const lookup = {payload: {word: 'finished', sentence: 'She finished her homework.', context: 'She finished her homework.', contextAt: 4},
        aiCard: reply(['concluiu']), contextMarks: null};
    const card = {...cardFromLookup(lookup, {ocrLanguage: 'en-US'}), createdAt: 0};
    assert.equal(card.translation, 'terminou');
    assert.deepEqual(card.translationAlternatives, ['concluiu']);
    assert.deepEqual(noteContextFromCard(card).aiCard.translationAlternatives, ['concluiu']);
    assert(toCsv([card]).includes('"terminou, concluiu"'), 'the CSV translation column has both');
    assert(toQuizlet([card]).startsWith('finished\tterminou, concluiu'), 'and Quizlet');
    const old = {word: 'run', translation: 'correr', createdAt: 0};
    assert(toCsv([old]).split('\r\n')[1].includes(',correr,'), 'a card saved before has only its translation');
    assert.deepEqual(noteContextFromCard(old).aiCard.translationAlternatives, []);
});

test('the OCR note corrects only clear misreadings, and the scripts\' examples are only misread characters', () => {
    for (const part of ['only when the context and its spelling clearly point to one specific misreading',
        'Keep rare or archaic words, names, slang, dialectal or informal spellings, inflected forms and technical terms',
        'A longer unit the word belongs to, a word cut at another place and the parts of a word are not misreadings',
        'When unsure, describe the word as given and leave "ocrFix" out']) {
        assert(OCR_ERRORS_PROMPT.includes(part), part);
    }
    for (const tag of ['zh-CN', 'ja-JP', 'ko-KR']) {
        assert(!/cut the word|spaces/.test(ocrErrorsFor(tag)), `${tag}: no word boundaries among the misreadings`);
    }
    assert(ocrErrorsFor('en-US').includes('rn/m'), 'the Latin examples stay');
    const prompt = cardPrompt(CARD_PROMPT, {ocrLanguage: 'zh-CN'}, {});
    assert(prompt.indexOf('"selection"') < prompt.indexOf('"ocrFix": "<the word as given>'), 'units first, then the OCR note');
});
