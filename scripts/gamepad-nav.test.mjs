import assert from 'node:assert/strict';
import test from 'node:test';
import {nearest} from '../src/lib/gamepad-nav.js';

// Two lines of three words, 40 px wide, 20 px tall.
const words = [0, 1].flatMap((row) => [0, 1, 2].map((col) => ({x: col * 60, y: row * 40, w: 40, h: 20})));
const centre = (r) => ({x: r.x + r.w / 2, y: r.y + r.h / 2});

test('the D-pad moves to the nearest word that way, along the line first', () => {
    assert.equal(nearest(centre(words[0]), words, 'right'), 1);
    assert.equal(nearest(centre(words[1]), words, 'left'), 0);
    assert.equal(nearest(centre(words[1]), words, 'down'), 4);
    assert.equal(nearest(centre(words[5]), words, 'up'), 2);
    assert.equal(nearest(centre(words[2]), words, 'right'), -1, 'the end of the line stays put');
    assert.equal(nearest(centre(words[0]), words, 'up'), -1);
    // From the end of the first line, down goes to the word below, not the next line's first word.
    assert.equal(nearest(centre(words[2]), words, 'down'), 5);
});
