import assert from 'node:assert/strict';
import {readFile} from 'node:fs/promises';
import {runInNewContext} from 'node:vm';
import test from 'node:test';

const script = await readFile(new URL('../src/lib/dictionaries/article.js', import.meta.url), 'utf8');
function fixture() {
    const listeners = new Map();
    const windowListeners = new Map();
    const messages = [];
    const audio = [];
    const fragments = [];
    const parent = {postMessage: (message) => messages.push(JSON.parse(JSON.stringify(message)))};
    class Audio {
        constructor(src) { this.src = src; this.paused = false; audio.push(this); }
        play() { return Promise.resolve(); }
        pause() { this.paused = true; }
    }
    const document = {documentElement: {scrollHeight: 125},
        addEventListener: (name, callback) => listeners.set(name, callback),
        getElementById: (id) => id === 'meaning' ? {scrollIntoView: () => fragments.push(id)} : null,
        getElementsByName: () => []};
    runInNewContext(script, {document, Audio, parent,
        location: {pathname: '/e/d1/7', hash: ''},
        window: {getSelection: () => 'chosen text', addEventListener: (name, callback) => windowListeners.set(name, callback)},
        setTimeout() {}});
    const click = (href) => {
        let prevented = false;
        listeners.get('click')({target: {closest: () => ({getAttribute: () => href})}, preventDefault: () => { prevented = true; }});
        return prevented;
    };
    return {listeners, windowListeners, messages, audio, fragments, parent, click};
}

test('dictionary audio paths survive encoded backslashes and only one sound plays at once', () => {
    const view = fixture();
    assert(view.click('sound://%5Caudio%5Cmy%20word.mp3'));
    assert.equal(view.audio[0].src, '/d/d1/audio/my%20word.mp3');
    view.click('sound://next.ogg');
    assert.equal(view.audio[0].paused, true);
    assert.equal(view.audio[1].paused, false);
    view.windowListeners.get('message')({source: view.parent, data: {lookitan: true, type: 'stop-audio'}});
    assert.equal(view.audio[1].paused, true);
});

test('dictionary cross references preserve the word, anchor and malformed percent escapes', () => {
    const view = fixture();
    view.click('entry://two%20words#meaning');
    assert.deepEqual(view.messages[0], {type: 'lookup', text: 'two words', fragment: 'meaning', lookitan: true});
    view.click('bword://%ZZ');
    assert.equal(view.messages[1].text, '%ZZ');
    view.click('entry://#meaning');
    assert.deepEqual(view.fragments, ['meaning']);
});

test('online audio is played inline while external article links are sent to the host', () => {
    const view = fixture();
    view.click('https://example.test/word.mp3?voice=1');
    assert.equal(view.audio[0].src, 'https://example.test/word.mp3?voice=1');
    view.click('https://example.test/definition');
    assert.deepEqual(view.messages[0], {type: 'open', url: 'https://example.test/definition', lookitan: true});
    view.listeners.get('selectionchange')();
    assert.equal(view.messages[1].text, 'chosen text');
});
