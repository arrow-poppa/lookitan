import assert from 'node:assert/strict';
import test from 'node:test';
import {defaultAiOptions, normalizeAiOptions} from '../src/lib/ai/defaults.js';

test('a saved card prompt gets the synonyms, reading and components once, and components start on', async () => {
    const {CARD_PROMPT, COMPONENTS_FIELD, READING_FIELD, TRANSLATION_FIELD} = await import('../src/lib/ai/word-card.js');
    const old = CARD_PROMPT.replace(/, "synonyms".*\{\{formField\}\}/, '');
    assert(!old.includes(COMPONENTS_FIELD) && old.includes(TRANSLATION_FIELD));
    const migrated = normalizeAiOptions({cardPrompt: old});
    assert.equal(migrated.cardPrompt, CARD_PROMPT);
    assert.equal(migrated.cardPromptVersion, 3);
    assert.equal(migrated.cardComponents, true);
    const taken = normalizeAiOptions({cardPrompt: old, cardPromptVersion: 3, cardComponents: false});
    assert.equal(taken.cardPrompt, old, 'a prompt the user changed after the migration stays as it is');
    assert.equal(taken.cardComponents, false);
    assert(defaultAiOptions().cardPrompt.includes(READING_FIELD));
});

test('a card prompt saved with the components gets the form once, after them', async () => {
    const {CARD_PROMPT, COMPONENTS_FIELD, FORM_FIELD} = await import('../src/lib/ai/word-card.js');
    const old = CARD_PROMPT.replace(FORM_FIELD, '');
    assert.equal(normalizeAiOptions({cardPrompt: old, cardPromptVersion: 2}).cardPrompt, CARD_PROMPT);
    const edited = `My card. ${COMPONENTS_FIELD} and more`;
    assert.equal(normalizeAiOptions({cardPrompt: edited, cardPromptVersion: 2}).cardPrompt, `My card. ${COMPONENTS_FIELD}${FORM_FIELD} and more`);
    assert.equal(normalizeAiOptions({cardPrompt: old, cardPromptVersion: 3}).cardPrompt, old, 'taken out after the migration, it stays out');
});

test('"Corrigir erros de OCR com IA" starts on; a missing prompt is the default, an emptied one stays empty', async () => {
    const {OCR_ERRORS_PROMPT} = await import('../src/lib/ai/word-card.js');
    assert.equal(normalizeAiOptions({}).cardOcrFix, true);
    assert.equal(normalizeAiOptions({cardOcrFix: false}).cardOcrFix, false);
    assert.equal(normalizeAiOptions({cardOcrFix: 'no'}).cardOcrFix, true);
    assert.equal(normalizeAiOptions({}).ocrErrorsPrompt, OCR_ERRORS_PROMPT);
    assert.equal(normalizeAiOptions({ocrErrorsPrompt: '  '}).ocrErrorsPrompt, '  ', 'kept as saved; nothing is sent');
    assert(!('arabicOcrPrompt' in normalizeAiOptions({arabicOcrPrompt: 'old'})));
});

test('the context translator has its own "Corrigir erros de OCR com IA", and the pictures use the card\'s word', async () => {
    const {CONTEXT_PROMPT, contextPrompt} = await import('../src/lib/ai/word-card.js');
    const {imagePrompts, imageWord} = await import('../src/lib/images.js');
    const ai = normalizeAiOptions({imageOcrFix: false, imageOcrPrompt: 'old'});
    assert.equal(ai.contextOcrFix, true);
    assert(!('imageOcrFix' in ai) && !('imageOcrPrompt' in ai), 'the pictures\' own OCR note is gone');
    assert.equal(ai.imageUseCardWord, true);
    const settings = {ocrLanguage: 'zh-TW', translationLanguage: 'pt-BR'};
    const on = contextPrompt(CONTEXT_PROMPT, settings, ai);
    assert(on.includes('read from the screen by OCR') && on.includes('進/盡'));
    assert(on.indexOf('read from the screen by OCR') > on.indexOf("Word: '{{target}}'"), 'a paragraph of its own after the prompt');
    assert(!contextPrompt(CONTEXT_PROMPT, settings, {...ai, contextOcrFix: false}).includes('OCR'));
    assert(!imagePrompts(ai, settings).system.includes('OCR'), 'no OCR note in the image query');
    const card = {ocrFix: 'تابة → لكتابة'};
    assert.equal(imageWord('تابة', card, ai), 'لكتابة', 'the word the card corrected');
    assert.equal(imageWord('تابة', {ocrFix: ''}, ai), 'تابة');
    assert.equal(imageWord('تابة', null, ai), 'تابة', 'no card: the OCR word');
    assert.equal(imageWord('تابة', card, {...ai, imageUseCardWord: false}), 'تابة', 'switched off: as the OCR read it');
});

test('the first default transliteration prompt becomes the one that asks for Latin letters only', async () => {
    const {LEGACY_TRANSLITERATION_PROMPT, TRANSLITERATION_PROMPT} = await import('../src/lib/ai/word-card.js');
    assert(TRANSLITERATION_PROMPT.includes('Latin letters only') && TRANSLITERATION_PROMPT.includes('never the original script'));
    assert.equal(normalizeAiOptions({transliterationPrompt: LEGACY_TRANSLITERATION_PROMPT}).transliterationPrompt, TRANSLITERATION_PROMPT);
    assert.equal(normalizeAiOptions({transliterationPrompt: 'my letters'}).transliterationPrompt, 'my letters', "the user's own prompt stays");
});

test('the card gets the word analyzer\'s hints with the word, after the cached instructions', async () => {
    const {cardHints} = await import('../src/lib/ai/word-card.js');
    const context = 'واجهة بسيطة و حلحه الرسام';
    const at = [...context].indexOf('ح');
    const cut = cardHints({word: 'حل', context, contextAt: at, candidates: ['حل']});
    assert.equal(cut, `Hints for '{{target}}': the OCR read it inside "حلحه".`);
    const ja = cardHints({word: '食べさせられなかった', context: '彼は食べさせられなかった。', contextAt: 2,
        candidates: ['食べる', '食べさせられなかった', '食べさせられなかっ', '食べさせられなか']});
    assert.equal(ja, `Hints for '{{target}}': the word analyzer's dictionary forms: "食べる".`,
        'no run in unspaced text, and the prefixes after the word are not forms of it');
    assert.equal(cardHints({word: '更進', context: '勸君更進一杯酒', contextAt: 2, candidates: ['更進', '更']}), '', 'jieba gives no dictionary form');
    assert.equal(cardHints({word: '盡為', context: '無故人盡為政心閑', contextAt: 3, lineBreak: 1, candidates: ['盡為', '盡']}),
        `Hints for '{{target}}': the word analyzer joined "盡" at the end of one line or column with "為" at the start of the next, so the word may end or begin at that break.`,
        'a word joined across a column break');
    assert.equal(cardHints({word: 'run', context: 'I run, daily.', contextAt: 2, candidates: ['run']}), '', 'nothing to add');
    const {AIProvider} = await import('../src/lib/ai/ai-provider.js');
    const split = Object.create(AIProvider.prototype)._splitPrompts('حل', context, `Instructions.\nContext: '{{sentence}}'\nWord: '{{target}}'\n${cut}`, '', undefined);
    assert.equal(split.systemPrompt, 'Instructions.');
    assert(split.prompt.endsWith(`Word: 'حل'\nHints for 'حل': the OCR read it inside "حلحه".`));
});

test('explanation stays manual for new and existing AI settings without autoExplain', () => {
    assert.equal(defaultAiOptions().autoExplain, false);
    assert.equal(normalizeAiOptions(null).autoExplain, false);
    const previous = normalizeAiOptions({provider: 'gemini', autoGenerateOnLookup: false, streamResponse: false});
    assert.equal(previous.autoExplain, false);
    assert.equal(previous.provider, 'gemini');
    assert.equal(previous.autoGenerateOnLookup, false);
    assert.equal(previous.streamResponse, false);
});

test('automatic explanation persists independently of automatic cards and restores as a boolean', () => {
    for (const autoExplain of [true, false]) {
        const saved = JSON.parse(JSON.stringify({autoExplain, autoGenerateOnLookup: false}));
        const restored = normalizeAiOptions(saved);
        assert.equal(restored.autoExplain, autoExplain);
        assert.equal(restored.autoGenerateOnLookup, false);
    }
    for (const invalid of [null, 'true', 'false', 0, 1, [], {}]) {
        assert.equal(normalizeAiOptions({autoExplain: invalid}).autoExplain, false);
    }
});

test('the context translator and the pictures start from a copy of the word card provider, then keep their own', async () => {
    const {aiOptionsFor} = await import('../src/lib/ai/defaults.js');
    const old = normalizeAiOptions({provider: 'gemini', gemini: {apiKey: 'test-only-key', model: 'gemini-x'}, temperature: 0.3, streamResponse: false, systemPrompt: 'Be brief.'});
    for (const id of ['ai-context', 'ai-images']) {
        const options = aiOptionsFor(old, id);
        assert.equal(options.provider, 'gemini');
        assert.equal(options.gemini.model, 'gemini-x');
        assert.equal(options.gemini.apiKey, 'test-only-key', 'the same key keeps working');
        assert.equal(options.temperature, 0.3);
        assert.equal(options.streamResponse, false);
        assert.equal(options.systemPrompt, 'Be brief.');
    }
    old.sectionProviders['ai-context'].gemini.model = 'other';
    assert.equal(old.gemini.model, 'gemini-x', 'a copy, not the same object');
    assert.equal(old.sectionProviders['ai-images'].gemini.model, 'gemini-x');
    const saved = normalizeAiOptions(JSON.parse(JSON.stringify({provider: 'openai', sectionProviders: {'ai-context': {provider: 'deepseek', deepseek: {model: 'ds'}}}})));
    assert.equal(aiOptionsFor(saved, 'ai-context').provider, 'deepseek');
    assert.equal(aiOptionsFor(saved, 'ai-context').deepseek.model, 'ds');
    assert.equal(aiOptionsFor(saved, 'ai-context').openai.model, 'gpt-4o-mini', 'missing accounts get defaults');
    assert.equal(aiOptionsFor(saved, 'ai-images').provider, 'openai');
    assert.equal(aiOptionsFor(saved, 'ai-word'), saved, 'the word card uses the options as they are');
});

test('the context translator gets its own automatic switch, starting from the old shared one', () => {
    assert.equal(normalizeAiOptions({autoGenerateOnLookup: false}).autoTranslateContext, false);
    assert.equal(normalizeAiOptions({autoGenerateOnLookup: true}).autoTranslateContext, true);
    assert.equal(normalizeAiOptions({autoGenerateOnLookup: true, autoTranslateContext: false}).autoTranslateContext, false);
    assert.equal(normalizeAiOptions({}).imageMemeMode, false);
});

test('the old default explanation and context prompts become the ones that put the lookup text last', async () => {
    const {CONTEXT_PROMPT, EXPLAIN_PROMPT, LEGACY_CONTEXT_PROMPT, LEGACY_EXPLAIN_PROMPT} = await import('../src/lib/ai/word-card.js');
    const migrated = normalizeAiOptions({customPrompt: LEGACY_EXPLAIN_PROMPT, contextPrompt: LEGACY_CONTEXT_PROMPT});
    assert.equal(migrated.customPrompt, EXPLAIN_PROMPT);
    assert.equal(migrated.contextPrompt, CONTEXT_PROMPT);
    const own = normalizeAiOptions({customPrompt: 'Target: {{target}}\n\nSentence: {{sentence}}'});
    assert.equal(own.customPrompt, 'Target: {{target}}\n\nSentence: {{sentence}}', "the user's own prompt stays as written");
});

test('the explanation has its own language, which starts as the translation language', async () => {
    const {EXPLAIN_PROMPT, TRANSLATION_LANGUAGE_EXPLAIN_PROMPT, CONTEXT_PROMPT, fillLanguages, languageName} = await import('../src/lib/ai/word-card.js');
    assert.equal(normalizeAiOptions({customPrompt: TRANSLATION_LANGUAGE_EXPLAIN_PROMPT}).customPrompt, EXPLAIN_PROMPT);
    const [spanish, portuguese] = [languageName('es-419'), languageName('pt-BR')];
    const settings = {translationLanguage: 'pt-BR', explanationLanguage: 'es-419'};
    assert(fillLanguages(EXPLAIN_PROMPT, settings).includes(`Write in ${spanish}.`));
    assert(fillLanguages(CONTEXT_PROMPT, settings).includes(`into ${portuguese}.`));
    // Settings from before the split have no explanation language: the translation's is used.
    assert(fillLanguages(EXPLAIN_PROMPT, {translationLanguage: 'pt-BR'}).includes(`Write in ${portuguese}.`));
});

test('every editable prompt: missing or broken gets the default, emptied or spaces stay as saved, the user\'s own stays', async () => {
    const base = defaultAiOptions();
    const keys = ['customPrompt', 'cardPrompt', 'contextPrompt', 'wordTranslationPrompt', 'readingPrompt', 'componentsPrompt', 'formPrompt',
        'transliterationPrompt', 'ocrErrorsPrompt', 'expressionsPrompt', 'contextOcrPrompt', 'imageQuerySystemPrompt', 'imageQueryPrompt'];
    for (const key of keys) {
        assert.equal(typeof base[key], 'string', key);
        assert.equal(normalizeAiOptions({})[key], base[key], `${key}: missing`);
        for (const broken of [undefined, null, 3, {}]) { assert.equal(normalizeAiOptions({[key]: broken})[key], base[key], `${key}: ${broken}`); }
        assert.equal(normalizeAiOptions({[key]: ''})[key], '', `${key}: emptied`);
        assert.equal(normalizeAiOptions({[key]: ' \n '})[key], ' \n ', `${key}: spaces`);
        assert.equal(normalizeAiOptions({[key]: 'Mine.'})[key], 'Mine.', `${key}: the user's`);
        const saved = JSON.parse(JSON.stringify(normalizeAiOptions({[key]: ''})));
        assert.equal(normalizeAiOptions(saved)[key], '', `${key}: still empty after saving and loading`);
    }
    assert.equal(normalizeAiOptions({systemPrompt: ''}).systemPrompt, '', 'an empty system prompt is no custom system prompt');
});

test('emptied optional prompts leave their keys out and the JSON list well formed', async () => {
    const {CARD_PROMPT, CONTEXT_PROMPT, cardPrompt, contextPrompt} = await import('../src/lib/ai/word-card.js');
    const zh = {ocrLanguage: 'zh-CN'};
    const ar = {ocrLanguage: 'ar-SA'};
    const all = cardPrompt(CARD_PROMPT, zh, {});
    for (const name of ['translation', 'reading', 'components', 'form']) { assert(all.includes(`"${name}": `), name); }
    assert(cardPrompt(CARD_PROMPT, ar, {}).includes('"transliteration": '));
    const none = {wordTranslationPrompt: '', readingPrompt: '', componentsPrompt: ' ', formPrompt: '', transliterationPrompt: '', expressionsPrompt: '', ocrErrorsPrompt: ''};
    for (const [settings, label] of [[zh, 'zh'], [ar, 'ar']]) {
        const sent = cardPrompt(CARD_PROMPT, settings, none);
        for (const name of ['translation', 'reading', 'components', 'form', 'transliteration', 'selection', 'ocrFix']) {
            assert(!sent.includes(`"${name}"`), `${label}: no ${name}`);
        }
        assert(sent.endsWith('; [] when there are none]}'), `${label}: the list closes after the synonyms, which have no prompt of their own`);
        assert(!/\{\{\w+\}\}/.test(sent.replace(/\{\{(target|sentence)\}\}/g, '')), `${label}: every marker filled`);
        assert(!/,\s*[,}]/.test(sent), `${label}: no empty item in the list`);
    }
    assert(cardPrompt(CARD_PROMPT, zh, {...none, cardOcrFix: true, ocrErrorsPrompt: 'Mind OCR: {{ocrErrors}}.'}).endsWith('mixes simplified and traditional forms.'),
        'the OCR note when it has text and its switch is on');
    assert(!cardPrompt(CARD_PROMPT, zh, {...none, cardOcrFix: false, ocrErrorsPrompt: 'Mind OCR.'}).includes('Mind OCR'), 'its switch still turns it off');
    assert(!cardPrompt(CARD_PROMPT, zh, {cardComponents: false}).includes('"components"'), 'and the components switch');
    assert.equal(cardPrompt('', zh, {}), '', 'an emptied card prompt asks nothing, with no notes on their own');
    assert.equal(contextPrompt('  ', zh, {}), '', 'an emptied translator prompt asks nothing, with no OCR note on its own');
    const noNote = contextPrompt(CONTEXT_PROMPT, zh, {contextOcrPrompt: ''});
    assert(!noNote.includes('OCR') && noNote.includes('**this**'), 'an emptied context OCR note is left out; the translation still asked');
});
