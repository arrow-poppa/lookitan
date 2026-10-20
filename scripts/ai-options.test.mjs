import assert from 'node:assert/strict';
import test from 'node:test';
import {defaultAiOptions, normalizeAiOptions} from '../src/lib/ai/defaults.js';

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
