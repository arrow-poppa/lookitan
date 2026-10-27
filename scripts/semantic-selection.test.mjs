import assert from 'node:assert/strict';
import {readFile} from 'node:fs/promises';
import {resolve} from 'node:path';
import test from 'node:test';
import {AIProvider} from '../src/lib/ai/ai-provider.js';
import {normalizeAiOptions} from '../src/lib/ai/defaults.js';
import {clozeRange, locateSelection, occurrence, parseSelection, rangeWithin, selectedSpan, selectionText} from '../src/lib/ai/selection.js';
import {CARD_PROMPT, EXPRESSIONS_PROMPT, OCR_ERRORS_PROMPT, cardHints, cardPrompt, parseCard, parseCardProgress, takesCardNotes} from '../src/lib/ai/word-card.js';
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

test('the expressions note follows its own prompt: emptied, nothing of it is sent; restored, its five examples come back', () => {
    const settings = {ocrLanguage: 'en-US'};
    const off = cardPrompt(CARD_PROMPT, settings, {expressionsPrompt: ''});
    assert(!off.includes('"selection"') && !off.includes('Examples:') && !off.includes('Socrates'), 'no instructions, no examples');
    assert.equal(cardPrompt(CARD_PROMPT, settings, {expressionsPrompt: ' \n  '}), off, 'spaces only: the same');
    const restored = cardPrompt(CARD_PROMPT, settings, {expressionsPrompt: EXPRESSIONS_PROMPT});
    assert.deepEqual(restored.slice(restored.indexOf('Examples:')).match(/^\d\. /gm), ['1. ', '2. ', '3. ', '4. ', '5. ']);
    assert.equal(cardPrompt(CARD_PROMPT, settings, {}), restored, 'missing: the default');
    const mine = cardPrompt(CARD_PROMPT, settings, {expressionsPrompt: 'My unit rules.'});
    assert.equal(mine.split('My unit rules.').length, 2, 'the user\'s own, once');
    assert(!mine.includes('Examples:'));
    assert.equal(cardPrompt("Describe '{{target}}' in '{{sentence}}'.", settings, {}), "Describe '{{target}}' in '{{sentence}}'.");
    const ai = normalizeAiOptions({cardPrompt: 'mine {{translationField}}', cardPromptVersion: 3});
    assert(!('cardExpressions' in ai), 'the switch is gone');
    assert.equal(ai.expressionsPrompt, EXPRESSIONS_PROMPT);
    assert.equal(ai.cardPrompt, 'mine {{translationField}}', 'a saved prompt is not rewritten; the note is added when it is sent');
    assert.equal(cardPrompt(ai.cardPrompt, settings, ai).split('Examples:').length, 2, 'the examples once');
    assert.equal(normalizeAiOptions({expressionsPrompt: ''}).expressionsPrompt, '', 'an emptied prompt stays empty');
    assert.equal(parseCard('{"headword":"turn off","selection":{"parts":["turned","off"]}}').selection.parts.length, 2,
        'a selection a custom prompt asks for is still read');
});

test('the old "Reconhecer expressões" switch, off, becomes an empty expressions prompt; a prompt of the user\'s own is kept aside', () => {
    const settings = {ocrLanguage: 'en-US'};
    const fromDefault = normalizeAiOptions({cardExpressions: false, expressionsPrompt: EXPRESSIONS_PROMPT});
    assert.equal(fromDefault.expressionsPrompt, '', 'off stays off');
    assert(!('cardExpressions' in fromDefault) && !('expressionsPromptBackup' in fromDefault), 'nothing to keep: it was the default');
    assert(!cardPrompt(CARD_PROMPT, settings, fromDefault).includes('Examples:'));
    assert.equal(normalizeAiOptions({cardExpressions: false}).expressionsPrompt, '', 'off with no prompt saved');
    const own = normalizeAiOptions({cardExpressions: false, expressionsPrompt: 'My unit rules.'});
    assert.equal(own.expressionsPrompt, '');
    assert.equal(own.expressionsPromptBackup, 'My unit rules.', 'kept, never sent');
    assert(!cardPrompt(CARD_PROMPT, settings, own).includes('My unit rules.'));
    const on = normalizeAiOptions({cardExpressions: true, expressionsPrompt: 'My unit rules.'});
    assert.equal(on.expressionsPrompt, 'My unit rules.', 'on: as it was');
    assert(!('expressionsPromptBackup' in on));
    assert.deepEqual(normalizeAiOptions(own), own, 'the same after saving and loading again');
});

test('a word repeated in its context tells the AI which one is on screen', () => {
    const payload = {word: 'turn', context: 'turn it on. Then turn it off.', contextAt: 17, candidates: []};
    assert.equal(cardHints(payload), "The word on screen is the second of the 2 times '{{target}}' is in the context.");
    assert.equal(cardHints({...payload, contextAt: 0}), "The word on screen is the first of the 2 times '{{target}}' is in the context.");
    assert.equal(cardHints({word: 'turn', context: 'Then turn it off.', contextAt: 5, candidates: []}), '', 'once: nothing to add');
    const split = Object.create(AIProvider.prototype)._splitPrompts('turn', payload.context, `Rules.\n${cardHints(payload)}`, '', undefined);
    assert.equal(split.systemPrompt, 'Rules.', 'the position goes with the lookup, never in the cached instructions');
});

test('the pictures search the unit found in the text, else the OCR fix, else the word as read', () => {
    const unit = {headword: 'Socrates Awards', selection: {parts: ['Socrates Awards']}};
    const fix = {headword: 'table', ocrFix: 'tabe → table', selection: null};
    const both = {headword: 'turn off', ocrFix: 'tumed → turned', selection: {parts: ['turned', 'off']}};
    const proposed = {headword: 'Nobel Prize', ocrFix: 'Socrales → Socrates', selection: {parts: ['Nobel Prize']}};
    assert.equal(imageWord('Socrates', unit, {}, true), 'Socrates Awards', 'a unit found');
    assert.equal(imageWord('tabe', fix, {}, false), 'table', 'an OCR fix only');
    assert.equal(imageWord('tumed', both, {}, true), 'turn off', 'a unit found wins over the fix; its headword carries the fix');
    assert.equal(imageWord('Socrales', proposed, {}, false), 'Socrates', 'a unit not found is never used; the fix is');
    assert.equal(imageWord('Socrates', unit, {}), 'Socrates', 'not found unless said so');
    assert.equal(imageWord('tumed', both, {imageUseCardWord: false}, true), 'tumed', 'switched off: the word as read');
    assert.equal(imageWord('runs', {headword: 'run', selection: null}, {}, false), 'runs', 'neither: the word as read');
});

test('a custom card prompt that asks for the card JSON gets the notes; one that asks for them itself, or for no JSON, does not', () => {
    const settings = {ocrLanguage: 'en-US'};
    const json = "Word: '{{target}}'\nContext: '{{sentence}}'\nReply with JSON only: {\"headword\": its dictionary form, \"definition\": a short definition}";
    assert(takesCardNotes(json) && takesCardNotes(CARD_PROMPT));
    const sent = cardPrompt(json, settings, {});
    assert(sent.startsWith(json), "the user's prompt comes first, unchanged");
    assert.equal(sent.split('Examples:').length, 2, 'the expressions note once');
    assert(sent.includes('"ocrFix"'), 'and the OCR note');
    assert.equal(cardPrompt(json, settings, {expressionsPrompt: '', cardOcrFix: false}), json, 'both off: as written');
    const free = "Explain '{{target}}' as used in '{{sentence}}', in two sentences.";
    assert(!takesCardNotes(free));
    assert.equal(cardPrompt(free, settings, {}), free, 'no card JSON asked: as written');
    const own = `${json.slice(0, -1)}, "selection": the phrase it belongs to, or null, "ocrFix": what OCR misread}`;
    assert.equal(cardPrompt(own, settings, {}), own, 'a prompt asking for "selection" and "ocrFix" itself gets neither note');
    const ownSelection = `${CARD_PROMPT.slice(0, -1)}, "selection": mine}`;
    const withOcr = cardPrompt(ownSelection, settings, {});
    assert(!withOcr.includes('Examples:') && withOcr.includes('"ocrFix"'), 'each note is skipped on its own');
});

test('occurrences of the word are whole words in spaced scripts, any match in unspaced ones', () => {
    const turn = 'The turnover rose. Then turn it off.';
    assert.equal(occurrence(turn, at(turn, 'turn', 10), 'turn'), null, '"turn" in "turnover" is not the word');
    assert.equal(cardHints({word: 'turn', context: turn, contextAt: at(turn, 'turn', 10), candidates: []}), '');
    const twice = 'Turn it on. Then turn it off.';
    assert.deepEqual(occurrence(twice, at(twice, 'turn'), 'turn'), [2, 2], 'case is ignored');
    const dotted = 'İ go and go.';
    assert.deepEqual(occurrence(dotted, 2, 'go'), [1, 2], 'a letter whose lower case is longer does not shift the count');
    const ar = 'قرأ الكتاب ثم كتاب آخر';
    assert.equal(occurrence(ar, at(ar, 'كتاب', 10), 'كتاب'), null, 'كتاب inside الكتاب is not another occurrence');
    const run = 'جماعةالضفادع جماعة';
    assert.deepEqual(occurrence(run, 0, 'جماعة'), [1, 2], 'the word on screen counts even when cut out of a run');
    const zh = '大学的学生在学习。';
    assert.deepEqual(occurrence(zh, at(zh, '学', 3), '学'), [2, 3], 'Chinese has no word boundaries to test');
    assert.equal(occurrence(turn, 3, 'turn'), null, 'not at the word: nothing');
    assert.equal(occurrence('', 0, 'x'), null);
});

test('the cloze range is the unit when it is one piece, else the word where the OCR found it', () => {
    const text = 'The cat saw the cat.';
    assert.deepEqual(clozeRange(text, 16, 'cat', null), [16, 19]);
    assert.deepEqual(clozeRange(text, 12, 'the', [[12, 19]]), [12, 19]);
    assert.deepEqual(clozeRange(text, 4, 'cat', [[4, 7], [16, 19]]), [4, 7], 'a split unit: the word');
    assert.equal(clozeRange(text, null, 'cat', null), null, 'twice and no anchor');
    assert.equal(clozeRange('', 0, 'cat', null), null);
    const para = 'Go home. Go home. Then rest.';
    assert.deepEqual(rangeWithin(para, 'Go home.', [12, 16]), [3, 7], 'the second sentence holds the second "home"');
    assert.deepEqual(rangeWithin(para, para, [12, 16]), [12, 16]);
    assert.equal(rangeWithin(para, 'Then rest.', [12, 16]), null, 'a sentence without it');
    assert.equal(rangeWithin(para, 'Go home.', null), null);
});
