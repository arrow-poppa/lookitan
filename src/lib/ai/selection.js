// The word card's "selection": the unit the AI describes when the word on screen is only part of
// it ("Socrates" in "the Socrates Awards", "macht" in "macht das Licht aus"). The AI names the
// unit's pieces as they are written; this finds them in the context around the word, so the
// pop-up can mark them. The AI is never asked for positions, and never asked again: pieces that
// are not found, or that could be found in more than one way, leave the word's own mark.

/** Characters kept of the unit's kind ("phrasal_verb", "proper_noun"…). */
const MAX_TYPE = 32;
const MAX_PARTS = 6;
/** Code points in one piece. */
const MAX_PART = 80;
/** Code points between two pieces of a split unit ("turned the light off"). */
const MAX_GAP = 80;
/** A model sometimes writes a split unit as one piece: "turned ... off". */
const ELLIPSIS = /\s*(?:\.{3}|…)\s*/u;
/** A sentence end between two pieces means they are not one unit. */
const SENTENCE_END = /[.!?。！？]/u;
const WORD_CHAR = /[\p{L}\p{M}\p{N}]/u;
/** Scripts written without spaces: a piece may begin or end next to any of their letters. */
const UNSPACED = /[\p{Script=Han}\p{Script=Hiragana}\p{Script=Katakana}\p{Script=Thai}\p{Script=Lao}\p{Script=Khmer}\p{Script=Myanmar}]/u;

/**
 * The "selection" key of a card reply, or null when it is missing or unusable.
 * @param {unknown} value
 * @returns {{parts: string[], type: string}|null}
 */
export function parseSelection(value) {
    if (typeof value !== 'object' || value === null || Array.isArray(value)) { return null; }
    const given = Array.isArray(value.parts) ? value.parts : (typeof value.surface === 'string' ? [value.surface] : []);
    const parts = given.filter((p) => typeof p === 'string').flatMap((p) => p.split(ELLIPSIS)).map((p) => p.trim()).filter(Boolean);
    if (!parts.length || parts.length > MAX_PARTS || parts.some((p) => [...p].length > MAX_PART)) { return null; }
    const type = typeof value.type === 'string' ? value.type.trim().toLowerCase().slice(0, MAX_TYPE) : '';
    return {parts, type};
}

/** The unit as the card names it: its pieces, with "…" where other words come between them. */
export function selectionText(selection) {
    return selection?.parts?.length ? selection.parts.join(' … ') : '';
}

const lower = (c) => c.toLowerCase();
const isSpace = (c) => /\s/u.test(c);

/** Where `piece` ends when it starts at `i` (a space in it matches any run of spaces), else -1. */
function matchAt(chars, i, piece) {
    let j = i;
    for (let k = 0; k < piece.length; k++) {
        if (isSpace(piece[k])) {
            if (j >= chars.length || !isSpace(chars[j])) { return -1; }
            while (k + 1 < piece.length && isSpace(piece[k + 1])) { k++; }
            while (j < chars.length && isSpace(chars[j])) { j++; }
        } else {
            if (j >= chars.length || lower(chars[j]) !== lower(piece[k])) { return -1; }
            j++;
        }
    }
    return j;
}

/** Whether a piece at [start, end) would cut a word of a spaced script in two. The word on screen
 * may itself be cut out of a longer run (an Arabic run split by the analyzer), so its own edges
 * are not tested. */
function cutsWord(chars, start, end, anchor) {
    const cuts = (a, b) => a !== undefined && b !== undefined && WORD_CHAR.test(a) && WORD_CHAR.test(b) && !UNSPACED.test(a) && !UNSPACED.test(b);
    return (start !== anchor[0] && cuts(chars[start - 1], chars[start])) ||
        (end !== anchor[1] && cuts(chars[end - 1], chars[end]));
}

/** Every place `piece` is in the text, as [start, end). */
function occurrences(chars, piece, anchor) {
    const found = [];
    for (let i = 0; i < chars.length; i++) {
        const end = matchAt(chars, i, piece);
        if (end > i && !cutsWord(chars, i, end, anchor)) { found.push([i, end]); }
    }
    return found;
}

/** The word's own span in the text: at `at` when it is there, else its only occurrence. */
function anchorOf(chars, at, word) {
    const piece = [...word];
    if (!piece.length) { return null; }
    if (Number.isInteger(at) && at >= 0 && matchAt(chars, at, piece) === at + piece.length) { return [at, at + piece.length]; }
    const all = [];
    for (let i = 0; i < chars.length && all.length < 2; i++) {
        if (matchAt(chars, i, piece) === i + piece.length) { all.push([i, i + piece.length]); }
    }
    return all.length === 1 ? all[0] : null;
}

/** Whether the gap between two pieces may lie inside one unit. */
function joinable(chars, end, start) {
    return start >= end && start - end <= MAX_GAP && !SENTENCE_END.test(chars.slice(end, start).join(''));
}

/**
 * Where the card's selection is in the text, as ordered [start, end) code point ranges, or null to
 * keep the word's own mark: no selection, a piece not found, pieces out of order or far apart, a
 * unit that does not touch the word on screen, more than one way to place it, or a unit that is
 * just the word.
 * @param {string} text the context the AI read (`payload.context`, else the sentence)
 * @param {number|null|undefined} at where the word starts in it, in code points (`contextAt`)
 * @param {string} word the word on screen
 * @param {{parts: string[]}|null|undefined} selection
 * @returns {[number, number][]|null}
 */
export function locateSelection(text, at, word, selection) {
    const parts = selection?.parts;
    if (!text || !word || !Array.isArray(parts) || !parts.length) { return null; }
    const chars = [...text];
    const anchor = anchorOf(chars, at, word);
    if (!anchor) { return null; }
    const found = parts.map((p) => occurrences(chars, [...p], anchor));
    const placements = new Map();
    for (let k = 0; k < parts.length; k++) {
        for (const own of found[k]) {
            if (own[0] >= anchor[1] || own[1] <= anchor[0]) { continue; }
            // From the piece on the word, each other piece is the nearest one before or after it.
            const ranges = [own];
            for (let j = k - 1; j >= 0 && ranges.length; j--) {
                const next = ranges[0];
                const before = found[j].filter(([s, e]) => s < next[0] && joinable(chars, e, next[0])).pop();
                if (before) { ranges.unshift(before); } else { ranges.length = 0; }
            }
            for (let j = k + 1; j < parts.length && ranges.length; j++) {
                const prev = ranges[ranges.length - 1];
                const after = found[j].find(([s]) => joinable(chars, prev[1], s));
                if (after) { ranges.push(after); } else { ranges.length = 0; }
            }
            if (ranges.length) { placements.set(ranges.map((r) => r.join('-')).join(','), ranges); }
        }
    }
    if (placements.size !== 1) { return null; }
    const [ranges] = placements.values();
    return ranges.length === 1 && ranges[0][0] === anchor[0] && ranges[0][1] === anchor[1] ? null : ranges;
}

/**
 * The selection's text when it is one piece in the text (for a cloze), else ''.
 * @param {string} text
 * @param {[number, number][]|null} ranges from locateSelection
 */
export function selectedSpan(text, ranges) {
    return ranges?.length === 1 ? [...text].slice(ranges[0][0], ranges[0][1]).join('') : '';
}
