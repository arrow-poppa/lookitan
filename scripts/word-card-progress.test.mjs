import assert from 'node:assert/strict';
import test from 'node:test';
import {parseCardProgress} from '../src/lib/ai/word-card.js';

test('card fields appear before the JSON response is complete', () => {
    const card = parseCardProgress('{"headword":"run","partOfSpeech":["verb"],"definition":"move qui');
    assert.equal(card.headword, 'run');
    assert.deepEqual(card.partOfSpeech, ['verb']);
    assert.equal(card.definition, 'move qui');
    assert.equal(card.translation, '');
});

test('components keep their part, reading and meaning, and odd items are left out', () => {
    const card = parseCardProgress('{"headword":"期許","synonyms":["期望"],"components":[{"part":"期","reading":"qī","meaning":"period"},"x",{"meaning":"no part"},{"part":"許"}]}');
    assert.deepEqual(card.synonyms, ['期望']);
    assert.deepEqual(card.components, [{part: '期', reading: 'qī', meaning: 'period'}, {part: '許', reading: '', meaning: ''}]);
    assert.deepEqual(parseCardProgress('{"headword":"run"}').components, []);
    const korean = parseCardProgress('{"headword":"학교","components":[{"part":"학","hanja":"學","meaning":"learn"}]}');
    assert.deepEqual(korean.components, [{part: '학', reading: '學', meaning: 'learn'}], 'a Korean part shows its hanja where a reading goes');
});

test('split JSON escapes wait for complete unicode and retain escaped quotes and braces', () => {
    const text = '{"headword":"café","definition":"a \\"quoted\\" {word}';
    assert.equal(parseCardProgress(text).definition, 'a "quoted" {word}');
    assert.equal(parseCardProgress('{"definition":"caf\\u00').definition, 'caf');
    assert.equal(parseCardProgress('{"definition":"caf\\u00e9').definition, 'café');
    assert.equal(parseCardProgress('{"definition":"text\\').definition, 'text');
});

test('unfinished keys, values and lists keep earlier usable fields', () => {
    for (const tail of [',', ',"trans', ',"translation":', ',"labels":[', ',"labels":["inf']) {
        assert.equal(parseCardProgress('{"headword":"run"' + tail).headword, 'run');
    }
    assert.deepEqual(parseCardProgress('{"headword":"run","labels":["inf').labels, ['inf']);
    assert.equal(parseCardProgress('{"head'), null);
    assert.equal(parseCardProgress('normal prose'), null);
});

test('complete fenced response retains the final card and its HTML remains plain text data', () => {
    const card = parseCardProgress('```json\n{"headword":"<img src=x>","definition":"A word.","translation":"palavra"}\n```');
    assert.equal(card.headword, '<img src=x>');
    assert.equal(card.translation, 'palavra');
});
