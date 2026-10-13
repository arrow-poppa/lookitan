import assert from 'node:assert/strict';
import test from 'node:test';
import {AiResponseCache} from '../src/lib/ai/response-cache.js';

function storage() {
    const items = new Map();
    return {getItem: (key) => items.get(key) || null, setItem: (key, value) => items.set(key, value), removeItem: (key) => items.delete(key)};
}

test('completed answers survive popup/app recreation and expire after 24 hours', async () => {
    const disk = storage();
    let now = 1000, requests = 0;
    const cache = new AiResponseCache(undefined, {storage: disk, now: () => now});
    assert.equal(await cache.run('key', async () => { requests++; return 'answer'; }), 'answer');
    now += 2 * 60 * 1000;
    const reopened = new AiResponseCache(undefined, {storage: disk, now: () => now});
    assert.equal(await reopened.run('key', async () => { requests++; return 'unexpected'; }), 'answer');
    assert.equal(requests, 1);
    now += 24 * 60 * 60 * 1000;
    assert.equal(reopened.peek('key'), null);
    assert.equal(new AiResponseCache(undefined, {storage: disk, now: () => now}).peek('key'), null);
});

test('pending requests join and replay partial text but only final successful answers persist', async () => {
    const disk = storage();
    const cache = new AiResponseCache(undefined, {storage: disk});
    let complete, emit, calls = 0;
    const first = cache.run('key', async (_signal, onChunk) => {
        calls++; emit = onChunk;
        return new Promise((done) => { complete = done; });
    });
    emit('part', 'part');
    const chunks = [];
    const second = cache.run('key', async () => { calls++; return 'wrong'; }, (_delta, full) => chunks.push(full));
    assert.equal(first, second);
    assert.equal(calls, 1);
    assert.deepEqual(chunks, ['part']);
    assert.equal(new AiResponseCache(undefined, {storage: disk}).peek('key'), null);
    complete('complete');
    await first;
    assert.equal(new AiResponseCache(undefined, {storage: disk}).peek('key'), 'complete');
    await assert.rejects(cache.run('failed', async () => { throw new Error('fail'); }), /fail/);
    await cache.run('empty', async () => '');
    const restored = new AiResponseCache(undefined, {storage: disk});
    assert.equal(restored.peek('failed'), null);
    assert.equal(restored.peek('empty'), null);
});

test('bounded storage evicts oldest entries and force regeneration removes only its answer', async () => {
    const disk = storage();
    const cache = new AiResponseCache(undefined, {storage: disk, maxEntries: 2});
    for (const key of ['one', 'two', 'three']) { await cache.run(key, async () => key); }
    assert.equal(cache.peek('one'), null);
    await assert.rejects(cache.run('two', async () => { throw new Error('regeneration failed'); }, undefined, {force: true}));
    const restored = new AiResponseCache(undefined, {storage: disk});
    assert.equal(restored.peek('two'), null);
    assert.equal(restored.peek('three'), 'three');
    cache.clear();
    assert.equal(new AiResponseCache(undefined, {storage: disk}).peek('three'), null);
});

test('corrupt, denied and oversized persistent storage never breaks requests', async () => {
    const disk = {getItem: () => '{invalid', setItem: () => { throw new Error('quota exceeded'); }};
    const cache = new AiResponseCache(undefined, {storage: disk});
    assert.equal(await cache.run('key', async () => 'answer'), 'answer');
    assert.equal(cache.peek('key'), 'answer');
    const bounded = new AiResponseCache(undefined, {maxChars: 10});
    assert.equal(await bounded.run('key', async () => 'answer exceeding storage budget'), 'answer exceeding storage budget');
    assert.equal(bounded.peek('key'), null);
});
