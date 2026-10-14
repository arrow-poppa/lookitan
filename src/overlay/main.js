// Transparent overlay over the monitor: one rectangle per recognized word. Clicking a rectangle
// looks that word up without a new OCR pass; clicking elsewhere or Esc closes the overlay.

import {invoke} from '@tauri-apps/api/core';
import {listen} from '@tauri-apps/api/event';
import {schemeBase} from '../lib/api.js';

const root = document.getElementById('words');
const frame = document.getElementById('frame');
let current = null;
let selected = null;

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

function render() {
    if (!current) { return; }
    const scale = viewportScale();
    if (current.frameRect) {
        position(frame, current.frameRect, scale);
    }
    const frag = document.createDocumentFragment();
    for (const w of current.words) {
        const el = document.createElement('div');
        el.className = 'w';
        if (selected && containsCenter(w, selected)) {
            el.classList.add('sel');
        }
        position(el, w, scale);
        el.title = w.text;
        frag.appendChild(el);
    }
    root.replaceChildren(frag);
}

void listen('overlay', (event) => {
    const p = event.payload;
    if (!current || current.id !== p.id) {
        selected = null;
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
        for (const el of root.querySelectorAll('.w.sel')) { el.classList.remove('sel'); }
        target.classList.add('sel');
        const scale = viewportScale();
        void invoke('overlay_pick', {x: ev.clientX / scale.x, y: ev.clientY / scale.y});
    } else {
        void invoke('overlay_close');
    }
});

window.addEventListener('resize', render);

document.addEventListener('keydown', (ev) => {
    if (ev.key === 'Escape') { void invoke('overlay_close'); }
});
