// Transparent overlay over the monitor: one rectangle per recognized word. Clicking a rectangle
// looks that word up without a new OCR pass; clicking elsewhere or Esc closes the overlay.

import {invoke} from '@tauri-apps/api/core';
import {listen} from '@tauri-apps/api/event';
import {schemeBase} from '../lib/api.js';

const root = document.getElementById('words');
const frame = document.getElementById('frame');
let current = null;
let selected = null;

function containsCenter(w, s) {
    const cx = s.x + s.w / 2;
    const cy = s.y + s.h / 2;
    return cx >= w.x && cx <= w.x + w.w && cy >= w.y && cy <= w.y + w.h;
}

function render() {
    if (!current) { return; }
    const frag = document.createDocumentFragment();
    for (const w of current.words) {
        const el = document.createElement('div');
        el.className = 'w';
        if (selected && containsCenter(w, selected)) {
            el.classList.add('sel');
        }
        el.style.left = `${w.x}px`;
        el.style.top = `${w.y}px`;
        el.style.width = `${Math.max(w.w, 4)}px`;
        el.style.height = `${Math.max(w.h, 4)}px`;
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
    current = p;
    render();
});

document.addEventListener('mousedown', (ev) => {
    ev.preventDefault();
    const target = ev.target;
    if (target instanceof HTMLElement && target.classList.contains('w')) {
        for (const el of root.querySelectorAll('.w.sel')) { el.classList.remove('sel'); }
        target.classList.add('sel');
        void invoke('overlay_pick', {x: ev.clientX, y: ev.clientY});
    } else {
        void invoke('overlay_close');
    }
});

document.addEventListener('keydown', (ev) => {
    if (ev.key === 'Escape') { void invoke('overlay_close'); }
});
