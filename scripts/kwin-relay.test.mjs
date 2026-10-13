import assert from 'node:assert/strict';
import {readFile} from 'node:fs/promises';
import {runInNewContext} from 'node:vm';
import test from 'node:test';

const script = await readFile(new URL('../src-tauri/resources/kwin/lookitan-cursor.js', import.meta.url), 'utf8');
function fixture() {
    let now = 0, tick, wantFast = false, fail = false;
    const calls = [];
    class Timer {
        timeout = {connect: (callback) => { tick = callback; }};
        start() {}
    }
    const workspace = {cursorPos: {x: -100, y: 100}, screenAt: () => ({name: 'left', devicePixelRatio: 1,
        geometry: {x: -1920, y: 0, width: 1920, height: 1080}}), activeWindow: null, windowActivated: {connect() {}}};
    runInNewContext(script, {QTimer: Timer, workspace, Date: {now: () => now},
        callDBus: (...args) => { calls.push(JSON.parse(args[4])); if (!fail) { args[5](wantFast); } }});
    return {calls, tick: (time) => { now = time; tick(); }, fast: (value) => { wantFast = value; }, fail: (value) => { fail = value; }};
}

test('a stationary cursor keeps a heartbeat and preserves negative monitor coordinates', () => {
    const relay = fixture();
    relay.tick(100);
    relay.tick(200);
    assert.equal(relay.calls.length, 1);
    relay.tick(1100);
    assert.equal(relay.calls.length, 2);
    assert.equal(relay.calls[1].x, -100);
    assert.equal(relay.calls[1].sx, -1920);
});

test('an unavailable sink is retried even without mouse movement', () => {
    const relay = fixture();
    relay.fail(true);
    relay.tick(100);
    relay.tick(200);
    assert.equal(relay.calls.length, 2);
    relay.fail(false);
    relay.tick(300);
    relay.tick(400);
    assert.equal(relay.calls.length, 3);
});

test('fast tracking reports a stationary pointer and returns to the idle heartbeat', () => {
    const relay = fixture();
    relay.fast(true);
    relay.tick(100);
    relay.tick(116);
    assert.equal(relay.calls.length, 2);
    relay.fast(false);
    relay.tick(132);
    relay.tick(232);
    assert.equal(relay.calls.length, 3);
    relay.tick(1132);
    assert.equal(relay.calls.length, 4);
});
