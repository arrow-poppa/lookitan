import assert from 'node:assert/strict';
import {readFile} from 'node:fs/promises';
import {resolve} from 'node:path';
import test from 'node:test';
import {AIProvider} from '../src/lib/ai/ai-provider.js';
import {normalizeAiOptions} from '../src/lib/ai/defaults.js';
import {locateSelection, parseSelection, selectedSpan, selectionText} from '../src/lib/ai/selection.js';
import {CARD_PROMPT, EXPRESSIONS_PROMPT, OCR_ERRORS_PROMPT, cardHints, cardPrompt, parseCard, parseCardProgress} from '../src/lib/ai/word-card.js';
import {imageWord} from '../src/lib/images.js';

const fixtures = JSON.parse(await readFile(resolve(import.meta.dirname, 'fixtures/semantic-selection.json'), 'utf8'));
/** Where `word` starts in `text`, in code points, as words.rs gives `context_at`. */
const at = (text, word, from = 0) => [...text.slice(0, text.indexOf(word, from))].length;
const span = (text, ranges) => ranges.map(([s, e]) => [...text].slice(s, e).join(''));

test('the card reply carries the unit the word belongs to, or none', () => {
    const card = parseCard('{"headword":"turn off","definition":"stop","selection":{"parts":["turned","off"],"type":"phrasal_verb"}}');
    assert.deepEqual(card.selection, {parts: ['turned', 'off'], type: 'phrasal_verb'});
    assert.equal(selectionText(card.selection), 'turned … off');
    assert.equal(parseCard('{"headword":"run"}').selection, null, 'old replies have no selection');
    for (const bad of [null, 'Socrates Awards', [], {}, {parts: []}, {parts: ['', '  ']}, {parts: [1, 2]}, {parts: Array(7).fill('a')}, {parts: ['x'.repeat(81)]}]) {
        assert.equal(parseSelection(bad), null, JSON.stringify(bad));
    }
    assert.deepEqual(parseSelection({parts: ['turned ... off']}).parts, ['turned', 'off'], 'a split unit written as one piece');
    assert.deepEqual(parseSelection({surface: 'Socrates Awards', type: 'Proper_Noun'}), {parts: ['Socrates Awards'], type: 'proper_noun'});
});

test('a selection still arriving is parsed but never complete', () => {
    const partial = parseCardProgress('{"headword":"Socrates Awards","definition":"an award","selection":{"parts":["Socr');
    assert.equal(partial.headword, 'Socrates Awards');
    assert.deepEqual(partial.selection.parts, ['Socr'], 'what came so far; the pop-up marks it only once the card is done');
});

test('the unit is found around the word on screen', () => {
    const text = 'He got nominated to the Socrates Awards.';
    assert.deepEqual(span(text, locateSelection(text, at(text, 'Socrates'), 'Socrates', {parts: ['Socrates Awards']})), ['Socrates Awards']);
    const light = 'She turned the light off.';
    assert.deepEqual(locateSelection(light, 4, 'turned', {parts: ['turned', 'off']}), [[4, 10], [21, 24]]);
    assert.deepEqual(span(light, locateSelection(light, 4, 'turned', parseSelection({parts: ['turned ... off']}))), ['turned', 'off']);
    const zh = '我喜欢学习中文。';
    assert.deepEqual(span(zh, locateSelection(zh, 3, '学', {parts: ['学习']})), ['学习'], 'no spaces needed in Chinese');
    assert.deepEqual(locateSelection('Er MACHT das Licht AUS.', 3, 'MACHT', {parts: ['macht', 'aus']}), [[3, 8], [19, 22]], 'case is ignored');
    assert.deepEqual(span('Social  media is loud.', locateSelection('Social  media is loud.', 0, 'Social', {parts: ['Social media']})), ['Social  media'],
        'a space matches the run of spaces the OCR read');
});

test('a unit that is just the word, or cannot be placed for sure, keeps the word marked', () => {
    const text = 'Esse filme é massa!';
    assert.equal(locateSelection(text, 13, 'massa', {parts: ['massa']}), null, 'the word itself');
    assert.equal(locateSelection(text, 13, 'massa', {parts: ['muito massa']}), null, 'not in the text');
    assert.equal(locateSelection('She turned the light off.', 4, 'turned', {parts: ['off', 'turned']}), null, 'out of order');
    assert.equal(locateSelection('He sat down. Then he turned off.', 3, 'sat', {parts: ['sat', 'off']}), null, 'not across a sentence end');
    assert.equal(locateSelection('He got the Socrates Awards.', 11, 'Socrates', {parts: ['the Nobel']}), null, 'must touch the word');
    assert.equal(locateSelection('The Awardsy went out.', 4, 'Awardsy', {parts: ['Awards']}), null, 'never half a word');
    assert.equal(locateSelection('', 0, 'x', {parts: ['x y']}), null);
    assert.equal(locateSelection('a b', 0, 'a', null), null);
    assert.equal(locateSelection('a b', 0, 'a', {parts: []}), null);
});

test('a repeated word is placed by where the OCR found it, and not at all when that is unknown', () => {
    const text = 'Turn it on. Then turn it off.';
    const second = at(text, 'turn', 5);
    assert.deepEqual(span(text, locateSelection(text, second, 'turn', {parts: ['turn', 'off']})), ['turn', 'off']);
    assert.equal(locateSelection(text, 0, 'Turn', {parts: ['Turn', 'off']}), null, 'the first one has no "off" in its sentence');
    assert.equal(locateSelection('the cat and the cat', null, 'cat', {parts: ['the cat']}), null, 'two places, no anchor');
    assert.deepEqual(locateSelection('a red car', null, 'red', {parts: ['red car']}), [[2, 9]], 'one place, no anchor needed');
    assert.deepEqual(locateSelection('ab ab', 0, 'ab', {parts: ['ab', 'ab']}), [[0, 2], [3, 5]], 'the word can only be the first piece');
    assert.equal(locateSelection('go go go', 3, 'go', {parts: ['go', 'go']}), null, 'the word could be either piece');
});

test('Arabic marks, right-to-left text and characters outside the BMP keep their places', () => {
    const ar = 'قرأ الطالبُ وبكتابهم الجديد';
    const word = 'وبكتابهم';
    assert.equal(locateSelection(ar, at(ar, word), word, {parts: [word]}), null, 'the written word stays as it is');
    assert.deepEqual(span(ar, locateSelection(ar, at(ar, 'الطالبُ'), 'الطالبُ', {parts: ['قرأ الطالبُ']})), ['قرأ الطالبُ'], 'the mark stays with its letter');
    assert.equal(locateSelection(ar, at(ar, 'الطالبُ'), 'الطالبُ', {parts: ['الطالب']}), null, 'a piece without the mark would cut it off');
    const wide = '𠮷野家で食べた。𠮷野家';
    const ranges = locateSelection(wide, 1, '野家', {parts: ['𠮷野家']});
    assert.deepEqual(ranges, [[0, 3]], 'code points, not UTF-16 units');
    assert.equal(selectedSpan(wide, ranges), '𠮷野家');
    assert.equal(selectedSpan(wide, [[0, 1], [2, 3]]), '', 'a split unit has no single span');
});

test('every unit in the multilingual fixtures is found where its word is', () => {
    for (const c of fixtures.cases) {
        const ranges = locateSelection(c.context, at(c.context, c.word), c.word, c.parts ? {parts: c.parts} : {parts: [c.word]});
        if (c.parts) { assert.deepEqual(span(c.context, ranges || []), c.parts, `${c.language}: ${c.context}`); }
        else { assert.equal(ranges, null, `${c.language}: ${c.context} keeps the word`); }
    }
    for (const c of fixtures.negative) {
        assert.equal(locateSelection(c.context, at(c.context, c.word), c.word, {parts: [c.word]}), null, c.context);
    }
});

test('the card prompt teaches exactly the five production examples, in the cached instructions', () => {
    const settings = {ocrLanguage: 'en-US', definitionLanguage: 'en-US', translationLanguage: 'pt-BR'};
    const prompt = cardPrompt(CARD_PROMPT, settings, {});
    const examples = prompt.slice(prompt.indexOf('Examples:'));
    assert.deepEqual(examples.match(/^\d\. /gm), ['1. ', '2. ', '3. ', '4. ', '5. ']);
    const production = fixtures.cases.filter((c) => c.production);
    assert.equal(production.length, 5);
    for (const c of fixtures.cases) {
        assert.equal(prompt.includes(c.context), Boolean(c.production), `only the five production contexts are sent: ${c.context}`);
    }
    for (const c of fixtures.negative) { assert(!prompt.includes(c.context), c.context); }
    assert(prompt.indexOf(EXPRESSIONS_PROMPT.slice(0, 40)) < prompt.indexOf(OCR_ERRORS_PROMPT.slice(0, 40)), 'before the OCR note');
    const split = Object.create(AIProvider.prototype)._splitPrompts('Socrates', 'He got nominated to the Socrates Awards.', prompt, '', undefined);
    assert(split.systemPrompt.includes('Examples:') && split.systemPrompt.includes('"selection"'), 'the examples are in the system message');
    assert(!split.systemPrompt.includes("Word: 'Socrates'"));
    assert.equal(split.prompt, "Context: 'He got nominated to the Socrates Awards.'\nWord: 'Socrates'", 'only the lookup in the user message');
    const other = Object.create(AIProvider.prototype)._splitPrompts('massa', 'Esse filme é massa!', prompt, '', undefined);
    assert.equal(other.systemPrompt, split.systemPrompt, 'the same instructions for every lookup');
});

test('the expressions note follows its switch and its own prompt, and leaves a prompt without markers alone', () => {
    const settings = {ocrLanguage: 'en-US'};
    assert(!cardPrompt(CARD_PROMPT, settings, {cardExpressions: false}).includes('"selection"'));
    assert(cardPrompt(CARD_PROMPT, settings, {expressionsPrompt: 'My unit rules.'}).includes('\n\nMy unit rules.'));
    assert.equal(cardPrompt("Describe '{{target}}' in '{{sentence}}'.", settings, {}), "Describe '{{target}}' in '{{sentence}}'.");
    const ai = normalizeAiOptions({cardPrompt: 'mine {{translationField}}', cardPromptVersion: 3});
    assert.equal(ai.cardExpressions, true);
    assert.equal(ai.expressionsPrompt, EXPRESSIONS_PROMPT);
    assert.equal(ai.cardPrompt, 'mine {{translationField}}', 'a saved prompt is not rewritten; the note is added when it is sent');
    assert.equal(cardPrompt(ai.cardPrompt, settings, ai).split('Examples:').length, 2, 'the examples once');
    assert.equal(normalizeAiOptions({cardExpressions: false}).cardExpressions, false);
});

test('a word repeated in its context tells the AI which one is on screen', () => {
    const payload = {word: 'turn', context: 'turn it on. Then turn it off.', contextAt: 17, candidates: []};
    assert.equal(cardHints(payload), "The word on screen is the second of the 2 times '{{target}}' is in the context.");
    assert.equal(cardHints({...payload, contextAt: 0}), "The word on screen is the first of the 2 times '{{target}}' is in the context.");
    assert.equal(cardHints({word: 'turn', context: 'Then turn it off.', contextAt: 5, candidates: []}), '', 'once: nothing to add');
    const split = Object.create(AIProvider.prototype)._splitPrompts('turn', payload.context, `Rules.\n${cardHints(payload)}`, '', undefined);
    assert.equal(split.systemPrompt, 'Rules.', 'the position goes with the lookup, never in the cached instructions');
});

test('the pictures search the unit the card described', () => {
    const card = {headword: 'Socrates Awards', selection: {parts: ['Socrates Awards']}};
    assert.equal(imageWord('Socrates', card, {}), 'Socrates Awards');
    assert.equal(imageWord('Socrates', card, {imageUseCardWord: false}), 'Socrates');
    assert.equal(imageWord('runs', {headword: 'run', selection: null}, {}), 'runs', 'no unit: the word as read');
    assert.equal(imageWord('tabe', {headword: 'x', ocrFix: 'tabe → table', selection: {parts: ['x']}}, {}), 'table', 'the OCR fix first');
});
