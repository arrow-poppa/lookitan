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
    const window = {innerWidth: 1920, innerHeight: 1080, addEventListener: (name, callback) => { handlers[`window:${name}`] = callback; }};
    const context = createContext({window, HTMLElement: Element, document: {
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
    return {root, frame, window, handlers, requests, emit: (payload) => emit(payload)};
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

test('WebKit fractional DPI corrects both borders and clicks when GTK reports scale one', async () => {
    const view = await fixture();
    const word = {text: 'target', x: 115, y: 115, w: 115, h: 23};
    view.emit({id: 1, viewportWidth: 2208, viewportHeight: 1242, words: [word], selected: word,
        frameUrl: '/c/1/frame.png', frameRect: {x: 115, y: 115, w: 1035, h: 299}});
    const rect = view.root.children[0];
    const close = (actual, expected) => assert(Math.abs(Number.parseFloat(actual) - expected) < 0.001);
    close(rect.style.left, 100);
    close(rect.style.top, 100);
    close(rect.style.width, 100);
    close(rect.style.height, 20);
    close(view.frame.style.width, 900);
    close(view.frame.style.height, 260);
    view.handlers.mousedown({preventDefault() {}, target: rect, clientX: 150, clientY: 110});
    close(view.requests[0].args.x, 172.5);
    close(view.requests[0].args.y, 126.5);
    assert(rect.classList.contains('sel'));
});

test('a browser viewport resize updates borders before another OCR or backend event', async () => {
    const view = await fixture();
    view.emit({id: 1, viewportWidth: 1920, viewportHeight: 1080,
        words: [{text: 'target', x: 100, y: 100, w: 100, h: 20}]});
    assert.equal(view.root.children[0].style.left, '100px');
    view.window.innerWidth = 960;
    view.window.innerHeight = 540;
    view.handlers['window:resize']();
    assert.equal(view.root.children[0].style.left, '50px');
    assert.equal(view.root.children[0].style.width, '50px');
    assert.equal(view.root.children[0].style.height, '10px');
    view.handlers.mousedown({preventDefault() {}, target: view.root.children[0], clientX: 75, clientY: 55});
    assert.equal(view.requests[0].args.x, 150);
    assert.equal(view.requests[0].args.y, 110);
});

test('a whole-screen lookup keeps the screen as it is and only highlights words, like Lookupper', async () => {
    const view = await fixture();
    const first = {text: 'but', x: 10, y: 20, w: 30, h: 18};
    const second = {text: 'being', x: 60, y: 20, w: 50, h: 18};
    view.emit({id: 1, words: [first, second], spotlight: true, viewportWidth: 1920, viewportHeight: 1080});
    assert(view.frame.classList.contains('spotlight'), 'the hint pill is hidden');
    assert.equal(view.root.children[0].style.backgroundImage, undefined, 'no blurred frame or cut-outs');
    assert(!view.root.children[0].classList.contains('sel'));

    // Look up one word, then another: the first keeps a faint highlight, the new one a strong one.
    view.emit({id: 1, words: [first, second], selected: first, spotlight: true, viewportWidth: 1920, viewportHeight: 1080});
    view.handlers.mousedown({preventDefault() {}, target: view.root.children[1], clientX: 70, clientY: 25});
    view.emit({id: 1, words: [first, second], selected: second, spotlight: true, viewportWidth: 1920, viewportHeight: 1080});
    assert(view.root.children[0].classList.contains('seen'));
    assert(view.root.children[1].classList.contains('sel'));

    // A new lookup starts clean.
    view.emit({id: 2, words: [first, second], spotlight: true, viewportWidth: 1920, viewportHeight: 1080});
    assert(!view.root.children[0].classList.contains('seen'));
});
