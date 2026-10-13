import assert from 'node:assert/strict';
import {readFile} from 'node:fs/promises';
import {resolve} from 'node:path';
import test from 'node:test';
import {createContext, SourceTextModule, SyntheticModule} from 'node:vm';

class Element {
    constructor() {
        this.style = {};
        this.children = [];
        this.className = '';
        this.classList = {
            contains: (name) => this.className.split(' ').includes(name),
            add: (name) => { this.className += ` ${name}`; },
            remove: (name) => { this.className = this.className.split(' ').filter((s) => s !== name).join(' '); },
        };
    }
    append(child) { this.children.push(child); }
    appendChild(child) { this.append(child); }
    replaceChildren(fragment) { this.children = fragment.children; }
    removeAttribute(name) { delete this[name]; }
    querySelectorAll() { return this.children.filter((child) => child.classList.contains('sel')); }
}

async function fixture() {
    const root = new Element(), frame = new Element(), handlers = {}, requests = [];
    let emit;
    const context = createContext({HTMLElement: Element, document: {
        getElementById: (id) => id === 'words' ? root : frame,
        createElement: () => new Element(), createDocumentFragment: () => new Element(),
        addEventListener: (name, callback) => { handlers[name] = callback; },
    }});
    const stubs = {
        '@tauri-apps/api/core': {invoke: async (command, args) => { requests.push({command, args}); }},
        '@tauri-apps/api/event': {listen: async (_name, callback) => { emit = (payload) => callback({payload}); }},
        '../lib/api.js': {schemeBase: () => 'lkt://localhost'},
    };
    const module = new SourceTextModule(await readFile(resolve(import.meta.dirname, '../src/overlay/main.js'), 'utf8'), {context});
    await module.link((name) => new SyntheticModule(Object.keys(stubs[name]), function () {
        for (const [key, value] of Object.entries(stubs[name])) { this.setExport(key, value); }
    }, {context}));
    await module.evaluate();
    return {root, frame, handlers, requests, emit: (payload) => emit(payload)};
}

test('moving or scaling the overlay redraws borders and keeps the selected word', async () => {
    const view = await fixture();
    const word = {text: 'target', x: 10, y: 20, w: 50, h: 18};
    view.emit({id: 1, words: [word], selected: word});
    assert(view.root.children[0].classList.contains('sel'));
    view.emit({id: 1, words: [{...word, x: 100, y: 200, w: 100, h: 36}]});
    assert.equal(view.root.children[0].style.left, '100px');
    assert.equal(view.root.children[0].style.top, '200px');
    assert(view.root.children[0].classList.contains('sel'));
    view.emit({id: 2, words: [word]});
    assert(!view.root.children[0].classList.contains('sel'));
});

test('frozen region images retain their own position and size instead of stretching to the monitor', async () => {
    const view = await fixture();
    view.emit({id: 1, words: [], frameUrl: '/c/1/frame.png', frameRect: {x: 220, y: 140, w: 900, h: 260}});
    assert.equal(view.frame.src, 'lkt://localhost/c/1/frame.png');
    assert.equal(view.frame.style.left, '220px');
    assert.equal(view.frame.style.width, '900px');
    view.emit({id: 1, words: [], frameUrl: '/c/1/frame.png', frameRect: {x: 440, y: 280, w: 1800, h: 520}});
    assert.equal(view.frame.style.top, '280px');
    assert.equal(view.frame.style.height, '520px');
    view.emit({id: 2, words: []});
    assert.equal(view.frame.style.display, 'none');
    assert.equal(view.frame.src, undefined);
});

test('word clicks send the exact viewport coordinates used by the backend inverse map', async () => {
    const view = await fixture();
    view.emit({id: 1, words: [{text: 'target', x: 115, y: 87, w: 115, h: 23}]});
    view.handlers.mousedown({preventDefault() {}, target: view.root.children[0], clientX: 172.5, clientY: 98.5});
    assert.equal(view.requests[0].command, 'overlay_pick');
    assert.equal(view.requests[0].args.x, 172.5);
    assert.equal(view.requests[0].args.y, 98.5);
});
