// Transparent overlay over the monitor: one rectangle per recognized word. Clicking a rectangle
// looks that word up without a new OCR pass; clicking elsewhere or Esc closes the overlay. In a
// whole-screen lookup with the pop-up pinned, a click elsewhere first closes only the pop-up
// (the backend decides, see `overlay_dismiss`).

import {invoke} from '@tauri-apps/api/core';
import {listen} from '@tauri-apps/api/event';
import {schemeBase} from '../lib/api.js';

const root = document.getElementById('words');
const frame = document.getElementById('frame');
const hint = document.getElementById('hint');
const dim = document.getElementById('dim');
const block = document.getElementById('block');
/** Margin around the highlighted block, in CSS pixels. */
const BLOCK_PAD = 6;
/** Same darkness as the frozen frame's brightness(0.85). */
const DIM_ALPHA = 0.15;
/** A whole-screen lookup fades in, so the screen darkens smoothly. */
const FADE_IN = {duration: 220, easing: 'ease-out'};
let current = null;
let selected = null;
/** Words already looked up in this overlay; they keep a faint highlight, as in Lookupper. */
let seen = [];

// GTK's integer scale factor can differ from WebKit's fractional CSS pixel scale. Compare
// the actual viewport sizes rather than assuming a toolkit pixel is one browser pixel.
function viewportScale() {
    return {
        x: current?.viewportWidth > 0 ? window.innerWidth / current.viewportWidth : 1,
        y: current?.viewportHeight > 0 ? window.innerHeight / current.viewportHeight : 1,
    };
}

function position(el, r, scale) {
    el.style.left = `${r.x * scale.x}px`;
    el.style.top = `${r.y * scale.y}px`;
    el.style.width = `${Math.max(r.w * scale.x, 4)}px`;
    el.style.height = `${Math.max(r.h * scale.y, 4)}px`;
}

function containsCenter(w, s) {
    const cx = s.x + s.w / 2;
    const cy = s.y + s.h / 2;
    return cx >= w.x && cx <= w.x + w.w && cy >= w.y && cy <= w.y + w.h;
}

function setSpotlight(on, blurred) {
    for (const [el, active] of [[hint, on], [frame, blurred], [root, blurred]]) {
        if (!el) { continue; }
        if (active) { el.classList.add('spotlight'); } else { el.classList.remove('spotlight'); }
    }
}

/**
 * Whole-screen lookup on a live screen: darken it slightly except under the words, like
 * Lookupper. A frozen frame does this with its own filter instead.
 */
function drawDim(on, scale) {
    if (!dim || typeof dim.getContext !== 'function') { return; }
    if (!on) { dim.style.display = 'none'; return; }
    const ratio = window.devicePixelRatio || 1;
    dim.width = Math.round(window.innerWidth * ratio);
    dim.height = Math.round(window.innerHeight * ratio);
    dim.style.display = 'block';
    const ctx = dim.getContext('2d');
    ctx.setTransform(ratio, 0, 0, ratio, 0, 0);
    ctx.clearRect(0, 0, window.innerWidth, window.innerHeight);
    ctx.fillStyle = `rgba(0, 0, 0, ${DIM_ALPHA})`;
    ctx.fillRect(0, 0, window.innerWidth, window.innerHeight);
    const b = blockRect();
    if (b) { ctx.clearRect(b.x * scale.x, b.y * scale.y, b.w * scale.x, b.h * scale.y); }
    for (const w of current.words) {
        ctx.clearRect(w.x * scale.x - 1, w.y * scale.y - 1, w.w * scale.x + 2, w.h * scale.y + 2);
    }
}

/** Shows the part of the captured frame under the word, so it stays sharp over the blur. */
function cutOut(el, w, f, scale) {
    el.style.backgroundImage = `url("${frame.src}")`;
    el.style.backgroundSize = `${f.w * scale.x}px ${f.h * scale.y}px`;
    el.style.backgroundPosition = `${(f.x - w.x) * scale.x}px ${(f.y - w.y) * scale.y}px`;
}

/** The block of text the shortcut was pressed on, with a margin; null when there is none. */
function blockRect() {
    const b = current?.block;
    return b ? {x: b.x - BLOCK_PAD, y: b.y - BLOCK_PAD, w: b.w + 2 * BLOCK_PAD, h: b.h + 2 * BLOCK_PAD} : null;
}

function drawBlock(scale) {
    if (!block) { return; }
    const b = blockRect();
    if (b) { position(block, b, scale); }
    block.style.display = b ? 'block' : 'none';
}

function render() {
    if (!current) { return; }
    const scale = viewportScale();
    if (current.frameRect) {
        position(frame, current.frameRect, scale);
    }
    const blurred = Boolean(current.spotlight && current.frameRect && frame.src);
    setSpotlight(Boolean(current.spotlight), blurred);
    drawDim(Boolean(current.spotlight) && !blurred, scale);
    drawBlock(scale);
    const frag = document.createDocumentFragment();
    for (const w of current.words) {
        const el = document.createElement('div');
        el.className = 'w';
        if (selected && containsCenter(w, selected)) {
            el.classList.add('sel');
        } else if (seen.some((s) => containsCenter(w, s))) {
            el.classList.add('seen');
        }
        position(el, w, scale);
        if (blurred) { cutOut(el, w, current.frameRect, scale); }
        el.title = w.text;
        frag.appendChild(el);
    }
    root.replaceChildren(frag);
}

/**
 * Hidden until the next lookup arrives, so the window never shows the previous one. While a
 * whole-screen lookup is still being read the window is up but empty: it only holds the input.
 */
function setVisible(visible) {
    if (!document.body) { return; }
    if (visible) { document.body.classList.remove('idle'); } else { document.body.classList.add('idle'); }
}

void listen('overlay-closed', () => {
    setVisible(false);
    // Drop the old words, so a click before the next lookup arrives cannot pick one of them.
    current = null;
    root.replaceChildren();
});

void listen('overlay', (event) => {
    const p = event.payload;
    if (!current || current.id !== p.id) {
        setVisible(true);
        if (p.spotlight) { document.body?.animate?.([{opacity: 0}, {opacity: 1}], FADE_IN); }
        selected = null;
        seen = [];
        if (p.frameUrl) {
            frame.src = `${schemeBase()}${p.frameUrl}`;
            frame.style.display = 'block';
        } else {
            frame.removeAttribute('src');
            frame.style.display = 'none';
        }
    }
    if (p.selected) { selected = p.selected; }
    else if (selected && current?.id === p.id) {
        // Keep selection attached to the same word when a move/DPI event changes its rectangle.
        const i = current.words.findIndex((w) => containsCenter(w, selected));
        selected = i >= 0 ? p.words[i] || null : null;
    }
    current = p;
    render();
});

document.addEventListener('mousedown', (ev) => {
    ev.preventDefault();
    const target = ev.target;
    if (target instanceof HTMLElement && target.classList.contains('w')) {
        for (const el of root.querySelectorAll('.w.sel')) { el.classList.remove('sel'); el.classList.add('seen'); }
        if (selected) { seen.push(selected); }
        target.classList.remove('seen');
        target.classList.add('sel');
        const scale = viewportScale();
        void invoke('overlay_pick', {x: ev.clientX / scale.x, y: ev.clientY / scale.y});
    } else {
        void invoke('overlay_dismiss');
    }
});

window.addEventListener('resize', render);

document.addEventListener('keydown', (ev) => {
    if (ev.key === 'Escape') { void invoke('overlay_close'); }
});
