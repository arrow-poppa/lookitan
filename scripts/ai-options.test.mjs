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
