import assert from 'node:assert/strict';
import {readFile} from 'node:fs/promises';
import {resolve} from 'node:path';
import test from 'node:test';
import {AIProvider} from '../src/lib/ai/ai-provider.js';
import {normalizeAiOptions} from '../src/lib/ai/defaults.js';
import {locateSelection} from '../src/lib/ai/selection.js';
import {CARD_PROMPT, CONTEXT_CARD_PROMPT, CONTEXT_RULE, DISPLAY_CARD_PROMPT, EXPRESSIONS_PROMPT, HEADWORD_SYNONYMS_KEY, LEGACY_CARD_PROMPT, SYNONYMS_KEY, UNIT_CARD_PROMPT,
    cardHints, cardPrompt, parseCard, parseCardProgress} from '../src/lib/ai/word-card.js';
import {cardFromLookup, noteContextFromCard} from '../src/lib/cards.js';

// Deterministic: these check what Lookitan asks and how it keeps the replies, written by hand in
// the fixture; they cannot show that a real model infers the meanings.
const {cases} = JSON.parse(await readFile(resolve(import.meta.dirname, 'fixtures/contextual-meaning.json'), 'utf8'));
/** Where `word` starts in `text`, in code points, as words.rs gives `context_at`. */
const at = (text, word) => [...text.slice(0, text.indexOf(word))].length;
const sent = (settings = {ocrLanguage: 'ko-KR'}, ai = {}) => cardPrompt(CARD_PROMPT, settings, ai);

test('the default card puts the meaning in context first, and its synonyms follow that meaning', () => {
    const prompt = sent();
    assert(prompt.includes(CONTEXT_RULE), 'the context rule');
    for (const part of ['its meaning here, not every dictionary sense', 'give that meaning in the translation, definition and synonyms',
        'that it is inferred', 'keep the headword, form and components of what is written', 'Hedge an uncertain inference',
        'are not anomalies', 'is not an OCR error']) {
        assert(CONTEXT_RULE.includes(part), part);
    }
    assert(prompt.includes(SYNONYMS_KEY) && SYNONYMS_KEY.includes('the intended one, when you inferred it') &&
        SYNONYMS_KEY.includes('not of other senses of the headword'));
    assert(!prompt.includes(HEADWORD_SYNONYMS_KEY) && !prompt.includes('synonyms of the headword in this sense'), 'the old rule is not sent with it');
    assert.equal(prompt.split('"synonyms"').length, 2, 'one synonyms key');
    assert.equal(prompt.split('not every dictionary sense').length, 2, 'the rule once');
    assert.equal(EXPRESSIONS_PROMPT.match(/^\d\. Context/gm).length, 5, 'still five examples');
    assert(prompt.includes(EXPRESSIONS_PROMPT), 'the expressions note unchanged');
    for (const c of cases) { assert(!prompt.includes(c.context), `${c.id}: fixtures are not in the prompt`); }
    assert(!/[가-힣]/.test(prompt.replace(/\(e\.g\. 을\/올, 는\/눈, 이\/어\)/, '')), 'no Korean example beyond the OCR note\'s');
});

test('the definition explains the word, not the context, and the synonyms stay in the text\'s language', () => {
    const prompt = sent({ocrLanguage: 'ru-RU', definitionLanguage: 'pt-BR'});
    assert(CONTEXT_RULE.includes('Explain what the word means, not the context: do not retell or explain the sentence.'));
    assert(prompt.includes('"definition": a one or two sentence definition in Brazilian Portuguese of its meaning here (what the word means, not what the context says)'));
    assert(prompt.includes('"synonyms": [up to three natural synonyms or near-synonyms in the language of the word and its context, never in the definition or translation language,'));
    assert(!DISPLAY_CARD_PROMPT.includes('not the context') && DISPLAY_CARD_PROMPT.includes('in the same language with the meaning'), 'the previous default kept as it was, for migration');
});

test('the rule and the synonyms key are fixed text, cached in the system message; the lookup goes after', () => {
    const payload = {word: '종류이다', context: cases[0].context, contextAt: at(cases[0].context, '종류이다'), candidates: ['종류', '종류이다'], analyzer: 'kiwi'};
    const template = `${sent()}\n${cardHints(payload)}`;
    const split = Object.create(AIProvider.prototype)._splitPrompts(payload.word, payload.context, template, '', undefined);
    assert(split.systemPrompt.includes(CONTEXT_RULE) && split.systemPrompt.includes(SYNONYMS_KEY));
    assert(!split.systemPrompt.includes('종류') && !split.prompt.includes(CONTEXT_RULE));
    assert(split.prompt.includes('종류이다') && split.prompt.includes('Kiwi'));
    const other = Object.create(AIProvider.prototype)._splitPrompts('run', 'I run.', `${sent({ocrLanguage: 'ko-KR'})}`, '', undefined);
    assert.equal(other.systemPrompt, split.systemPrompt, 'the same prefix for every lookup in the language');
});

test('earlier default card prompts move to the new one; edited and emptied ones stay', () => {
    for (const old of [LEGACY_CARD_PROMPT, UNIT_CARD_PROMPT, CONTEXT_CARD_PROMPT, DISPLAY_CARD_PROMPT, `  ${UNIT_CARD_PROMPT}\n`]) {
        assert.equal(normalizeAiOptions({cardPrompt: old, cardPromptVersion: 3}).cardPrompt, CARD_PROMPT);
    }
    const edited = `${UNIT_CARD_PROMPT} Be brief.`;
    assert.equal(normalizeAiOptions({cardPrompt: edited, cardPromptVersion: 3}).cardPrompt, edited, 'an edited prompt keeps its old synonyms');
    assert.equal(normalizeAiOptions({cardPrompt: '', cardPromptVersion: 3}).cardPrompt, '', 'emptied stays empty');
    assert.equal(normalizeAiOptions({}).cardPrompt, CARD_PROMPT, 'missing: the default, which "Restaurar padrão" also gives');
    assert.equal(sent({ocrLanguage: 'ko-KR'}, {}), cardPrompt(normalizeAiOptions({cardPrompt: UNIT_CARD_PROMPT, cardPromptVersion: 3}).cardPrompt, {ocrLanguage: 'ko-KR'}, {}));
    const custom = 'Word: {{target}}\nGive "headword" and "synonyms" only.';
    assert(cardPrompt(custom, {}, {}).startsWith(custom) && !cardPrompt(custom, {}, {}).includes(CONTEXT_RULE), 'a custom prompt gets no rule of ours');
});

test('the fixture replies parse and agree with their context', () => {
    assert.equal(cases.filter((c) => c.kind === 'anomaly').length >= 1 && cases.filter((c) => c.kind === 'negative').length >= 5, true);
    for (const c of cases) {
        const card = parseCard(JSON.stringify(c.reply));
        assert(card, c.id);
        assert(c.context.includes(c.word), `${c.id}: the word is in its context`);
        assert.equal(card.headword, c.expect.headword, `${c.id}: the headword of what is written`);
        assert.deepEqual([...card.synonyms], c.expect.synonyms, `${c.id}: synonyms kept as given`);
        for (const s of c.expect.avoidSynonyms || []) { assert(!card.synonyms.includes(s), `${c.id}: no synonym of another sense (${s})`); }
        assert(!card.synonyms.includes(card.headword), `${c.id}: a synonym is not the headword`);
        assert.equal(card.ocrFix, '', `${c.id}: a meaning is no OCR correction`);
        const marks = locateSelection(c.context, at(c.context, c.word), c.word, card.selection);
        if (c.expect.selection) {
            assert(marks, `${c.id}: the unit is found in the text`);
            assert.deepEqual(marks.map(([s, e]) => [...c.context].slice(s, e).join('')), c.expect.selection, c.id);
        } else {
            assert.equal(card.selection, null, `${c.id}: no unit beyond the word`);
        }
        for (const intended of c.expect.notInText || []) {
            assert(!c.context.includes(intended), `${c.id}: the intended word is not in the text`);
            assert.equal(locateSelection(c.context, at(c.context, c.word), c.word, {parts: [intended]}), null,
                `${c.id}: a selection of a word the text does not have is not marked`);
        }
    }
});

test('an inferred meaning: the card keeps the written lemma and form, and its definition says it is inferred', () => {
    const c = cases.find((x) => x.id === 'ko-anomaly-kind');
    const card = parseCard(JSON.stringify(c.reply));
    assert.equal(card.headword, '종류');
    assert(card.form[0].includes('이다') && card.components.at(-1).part === '이다', 'the copula of what is written');
    assert(/inferred/.test(card.definition) && /literally/.test(card.definition), 'the discrepancy explained');
    assert(card.translation.includes('gentil'));
    const ordinary = parseCard(JSON.stringify(cases.find((x) => x.id === 'ko-ordinary-type').reply));
    assert.deepEqual([...ordinary.synonyms], ['유형', '부류'], 'the same lemma in an ordinary sentence keeps its own synonyms');
    const partial = JSON.stringify(c.reply).slice(0, JSON.stringify(c.reply).indexOf('상냥하다') + 2);
    assert.deepEqual([...parseCardProgress(partial).synonyms], ['다정하다', '상냥'], 'the synonyms show while they stream');
});

test('contextual synonyms are saved and reach Anki as they came', () => {
    const c = cases[0];
    const lookup = {payload: {word: c.word, sentence: c.context, context: c.context, contextAt: at(c.context, c.word)}, aiCard: parseCard(JSON.stringify(c.reply))};
    const saved = {...cardFromLookup(lookup, {ocrLanguage: c.language}), createdAt: 0};
    assert.equal(saved.word, '종류이다', 'the word on screen is kept as read');
    assert.deepEqual([...saved.synonyms], c.expect.synonyms);
    assert.deepEqual([...noteContextFromCard(saved).aiCard.synonyms], c.expect.synonyms);
});

test('the hints say what each analyzer found, with no confidence invented', () => {
    const korean = '사랑은 종류이다.';
    const kiwi = cardHints({word: '종류이다', context: korean, contextAt: 4, candidates: ['종류', '종류이다'], analyzer: 'kiwi'});
    assert.equal(kiwi, `Hints for '{{target}}': morphological lemma candidates (Kiwi): "종류", of the form as written, not its meaning in this context nor an OCR correction.`);
    assert(cardHints({word: '학생이었다', context: '그는 학생이었다.', contextAt: 3, candidates: ['학생', '학생이었다'], analyzer: 'kiwi'}).includes('(Kiwi): "학생"'));
    const mecab = cardHints({word: '학생이었다', context: '그는 학생이었다.', contextAt: 3, candidates: ['학생', '학생이었다'], analyzer: 'mecab-ko-dic'});
    assert(mecab.includes('(mecab-ko-dic): "학생"') && !mecab.includes('Kiwi'), 'the analyzer actually used');
    const sudachi = cardHints({word: '進めて', context: '計画を進めていった。', contextAt: 3, candidates: ['進める', '進めて', '進'], analyzer: 'sudachi'});
    assert(sudachi.includes('(Sudachi): "進める"') && !sudachi.includes('"進"'), 'the prefixes after the word are not lemmas');

    const zh = '我喜欢学习中文。';
    const jieba = cardHints({word: '学习', context: zh, contextAt: 3, candidates: ['学习', '学'], analyzer: 'jieba'});
    assert.equal(jieba, `Hints for '{{target}}': jieba segmented it as a word, a guess at word boundaries (not a lemma nor an OCR correction).`);
    assert(!jieba.includes('CC-CEDICT') && !jieba.includes('lemma candidates'), 'no dictionary claimed without a lookup');
    const entry = (dictName, matched) => ({dictName, headword: matched, matched, dictId: 'd', entry: 0});
    assert(cardHints({word: '学习', context: zh, contextAt: 3, candidates: ['学习'], analyzer: 'jieba', entries: [entry('CC-CEDICT', '学习')]})
        .endsWith('; CC-CEDICT has an entry for it.'), 'a CC-CEDICT entry the lookup found');
    assert(!cardHints({word: '学习', context: zh, contextAt: 3, analyzer: 'jieba', entries: [entry('My dictionary', '学习'), entry('CC-CEDICT', '学')]})
        .includes('CC-CEDICT'), 'another dictionary, or an entry of a shorter form, is no CC-CEDICT match');
    const traditional = cardHints({word: '學習', context: '我喜歡學習中文。', contextAt: 3, analyzer: 'jieba', entries: [entry('CC-CEDICT', '學習')]});
    assert(traditional.includes('CC-CEDICT') && !traditional.includes('学'), 'traditional characters stay as read');

    const run = 'جماعةالضفادع في البركة';
    const arabic = cardHints({word: 'الضفادع', context: run, contextAt: 5, candidates: ['الضفادع'], analyzer: 'arabic-heuristic'});
    assert(arabic.includes('inside "جماعةالضفادع"') && arabic.includes('estimated heuristically and may be wrong'));
    assert.equal(cardHints({word: 'وبكتابهم', context: 'وبكتابهم عرفوا.', contextAt: 0, candidates: ['وبكتابهم'], analyzer: 'arabic-heuristic'}), '',
        'a word the splitter left whole: its spaces made it, nothing to warn');

    const generic = cardHints({word: '強しています', context: '日本語を勉強しています。', contextAt: 5, candidates: ['強しています', '強'], analyzer: 'generic'});
    assert(generic.includes('no word analyzer was available') && !/Sudachi|jieba|Kiwi/.test(generic), 'no analyzer claimed');
    assert.equal(cardHints({word: 'run', context: 'I run daily.', contextAt: 2, candidates: ['run'], analyzer: 'generic'}), '', 'spaces made it: no hint');
    assert.equal(cardHints({word: '食べた', context: '食べた。', contextAt: 0, candidates: ['食べる', '食べた']}).includes("the word analyzer's dictionary forms"), true,
        'without provenance (typed text, older builds), the forms as before');

    const twice = cardHints({word: '学习', context: '学习，再学习。', contextAt: 4, candidates: ['学习'], analyzer: 'jieba'});
    assert(twice.startsWith("The word on screen is the '{{target}}' in '学习。'") && twice.includes('jieba'), 'repeated words keep their place');
    for (const h of [kiwi, mecab, sudachi, jieba, arabic, generic, twice]) {
        assert(!/confiden|probab|\d+\s*%|score/i.test(h), `no invented confidence: ${h}`);
        assert(h.includes("'{{target}}'"), 'the hints go with the word, after the cached instructions');
    }
});
