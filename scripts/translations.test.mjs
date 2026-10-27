import assert from 'node:assert/strict';
import test from 'node:test';
import {normalizeAiOptions} from '../src/lib/ai/defaults.js';
import {CARD_PROMPT, LEGACY_CARD_PROMPT, LEGACY_OCR_ERRORS_PROMPT, LEGACY_WORD_TRANSLATION_PROMPT, OCR_ERRORS_PROMPT, SINGLE_WORD_TRANSLATION_PROMPT,
    WORD_TRANSLATION_PROMPT, cardPrompt, ocrErrorsFor, parseCard} from '../src/lib/ai/word-card.js';
import {cardFromLookup, toCsv, toQuizlet} from '../src/lib/cards.js';

test('the translation prompt asks for the best translation and up to two others of the same meaning, in the one key', () => {
    const settings = {ocrLanguage: 'en-US', translationLanguage: 'pt-BR'};
    const prompt = cardPrompt(CARD_PROMPT, settings, {});
    assert(prompt.includes(`"translation": ${WORD_TRANSLATION_PROMPT.replace('{{translationLanguage}}', 'Brazilian Portuguese')}`));
    assert(WORD_TRANSLATION_PROMPT.includes('separated by commas, up to two other natural translations with exactly this meaning'));
    assert(WORD_TRANSLATION_PROMPT.includes('never other senses'));
    assert(!prompt.includes('translationAlternatives'), 'no other key');
    assert(!cardPrompt(CARD_PROMPT, {...settings, showWordTranslation: false}, {}).includes('translation'), 'off: no translation asked');
    assert(cardPrompt(CARD_PROMPT, settings, {wordTranslationPrompt: 'one word in {{translationLanguage}}'}).includes('"translation": one word in Brazilian Portuguese,'),
        'the user\'s own translation prompt is sent as written');
});

test('a translation with alternatives is one string, kept as it came everywhere', () => {
    const card = parseCard('{"headword":"finish","translation":"terminou, concluiu","definition":"to end"}');
    assert.equal(card.translation, 'terminou, concluiu');
    assert.equal(Object.hasOwn(card, 'translationAlternatives'), false);
    const lookup = {payload: {word: 'finished', sentence: 'She finished her homework.', context: 'She finished her homework.', contextAt: 4}, aiCard: card};
    const saved = {...cardFromLookup(lookup, {ocrLanguage: 'en-US'}), createdAt: 0};
    assert.equal(saved.translation, 'terminou, concluiu');
    assert(toCsv([saved]).includes('"terminou, concluiu"'));
    assert(toQuizlet([saved]).startsWith('finished\tterminou, concluiu'));
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
    for (const translation of [LEGACY_WORD_TRANSLATION_PROMPT, SINGLE_WORD_TRANSLATION_PROMPT]) {
        const old = normalizeAiOptions({cardPrompt: LEGACY_CARD_PROMPT, wordTranslationPrompt: translation, ocrErrorsPrompt: LEGACY_OCR_ERRORS_PROMPT, cardPromptVersion: 3});
        assert.equal(old.cardPrompt, CARD_PROMPT);
        assert.equal(old.wordTranslationPrompt, WORD_TRANSLATION_PROMPT, translation);
        assert.equal(old.ocrErrorsPrompt, OCR_ERRORS_PROMPT);
    }
    const edited = normalizeAiOptions({cardPrompt: `${LEGACY_CARD_PROMPT} Be brief.`, wordTranslationPrompt: 'mine', ocrErrorsPrompt: 'my OCR note', cardPromptVersion: 3});
    assert.deepEqual([edited.cardPrompt, edited.wordTranslationPrompt, edited.ocrErrorsPrompt], [`${LEGACY_CARD_PROMPT} Be brief.`, 'mine', 'my OCR note']);
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
