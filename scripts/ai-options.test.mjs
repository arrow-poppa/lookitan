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
