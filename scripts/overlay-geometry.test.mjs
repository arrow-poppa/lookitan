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
    replaceChildren(fragment) { this.children = fragment ? fragment.children : []; }
    removeAttribute(name) { delete this[name]; }
    querySelectorAll() { return this.children.filter((child) => child.classList.contains('sel')); }
}

async function fixture(userAgent = 'Mozilla/5.0 (X11; Linux x86_64)') {
    const root = new Element(), frame = new Element(), body = new Element(), others = {}, handlers = {}, requests = [], listeners = {};
    body.animations = [];
    body.animate = (keyframes, options) => { body.animations.push({keyframes, options}); };
    const window = {innerWidth: 1920, innerHeight: 1080, addEventListener: (name, callback) => { handlers[`window:${name}`] = callback; }};
    const context = createContext({window, navigator: {userAgent}, HTMLElement: Element, requestAnimationFrame: (callback) => callback(), document: {
        body,
        getElementById: (id) => id === 'words' ? root : id === 'frame' ? frame : (others[id] ||= new Element()),
        createElement: () => new Element(), createDocumentFragment: () => new Element(),
        addEventListener: (name, callback) => { handlers[name] = callback; },
    }});
    const stubs = {
        '@tauri-apps/api/core': {invoke: async (command, args) => { requests.push({command, args}); }},
        '@tauri-apps/api/event': {listen: async (name, callback) => { listeners[name] = callback; }},
        '../lib/api.js': {schemeBase: () => 'lkt://localhost'},
    };
    const module = new SourceTextModule(await readFile(resolve(import.meta.dirname, '../src/overlay/main.js'), 'utf8'), {context});
    await module.link((name) => new SyntheticModule(Object.keys(stubs[name]), function () {
        for (const [key, value] of Object.entries(stubs[name])) { this.setExport(key, value); }
    }, {context}));
    await module.evaluate();
    return {root, frame, body, dim: others.dim, block: others.block, window, handlers, requests, document: {getElementById: (id) => (others[id] ||= new Element())},
        emit: (payload) => listeners.overlay({payload}), close: (payload = null) => listeners['overlay-closed']({payload})};
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
    assert(!view.frame.classList.contains('spotlight'), 'no frame yet, so nothing is blurred');
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

test('with the frame loaded, a whole-screen lookup blurs it lightly and keeps words as sharp cut-outs', async () => {
    const view = await fixture();
    const frameRect = {text: '', x: 0, y: 0, w: 1920, h: 1080};
    view.emit({id: 1, words: [{text: 'word', x: 10, y: 20, w: 50, h: 18}], frameUrl: '/c/1/frame.png', frameRect,
        spotlight: true, viewportWidth: 1920, viewportHeight: 1080});
    assert(view.root.classList.contains('spotlight'));
    const word = view.root.children[0].children[0];
    assert.equal(word.style.backgroundImage, 'url("lkt://localhost/c/1/frame.png")');
    assert.equal(word.style.backgroundSize, '1920px 1080px');
    assert.equal(word.style.backgroundPosition, '-10px -20px');
    // A lookup on a word (popup shown) with a frozen frame keeps the frame sharp and plain boxes.
    view.emit({id: 2, words: [{text: 'word', x: 10, y: 20, w: 50, h: 18}], frameUrl: '/c/2/frame.png', frameRect,
        spotlight: false, viewportWidth: 1920, viewportHeight: 1080});
    assert(!view.root.classList.contains('spotlight'));
    assert.equal(view.root.children[0].children[0].style.backgroundImage, undefined);
});

test('a word takes clicks over its whole hit area while its highlight stays on the ink', async () => {
    const view = await fixture();
    view.emit({id: 1, words: [{text: 'word', x: 110, y: 104, w: 40, h: 12, hit: [104, 100, 52, 20]}],
        viewportWidth: 1920, viewportHeight: 1080});
    const area = view.root.children[0];
    const ink = area.children[0];
    assert.deepEqual([area.style.left, area.style.top, area.style.width, area.style.height], ['104px', '100px', '52px', '20px']);
    assert.deepEqual([ink.style.left, ink.style.top, ink.style.width, ink.style.height], ['6px', '4px', '40px', '12px']);
    view.handlers.mousedown({preventDefault() {}, target: area, clientX: 105, clientY: 101});
    assert.equal(view.requests.at(-1).command, 'overlay_pick', 'a click beside the ink still looks the word up');
});

test('a live whole-screen lookup dims the screen slightly except under the words', async () => {
    const view = await fixture();
    const ops = [];
    view.dim.getContext = () => ({
        setTransform() {}, set fillStyle(v) { ops.push(['style', v]); },
        clearRect: (...a) => ops.push(['clear', ...a]), fillRect: (...a) => ops.push(['fill', ...a]),
    });
    const word = {text: 'but', x: 10, y: 20, w: 30, h: 18};
    view.emit({id: 1, words: [word], spotlight: true, viewportWidth: 1920, viewportHeight: 1080});
    assert.equal(view.dim.style.display, 'block');
    assert.deepEqual(ops.find((op) => op[0] === 'style'), ['style', 'rgba(0, 0, 0, 0.15)']);
    assert.deepEqual(ops.find((op) => op[0] === 'fill'), ['fill', 0, 0, 1920, 1080]);
    assert.deepEqual(ops.at(-1), ['clear', 9, 19, 32, 20], 'the word keeps its normal brightness');

    // A frozen frame dims through its own filter; a lookup on a word does not dim.
    view.emit({id: 2, words: [word], frameUrl: '/c/2/frame.png', frameRect: {text: '', x: 0, y: 0, w: 1920, h: 1080},
        spotlight: true, viewportWidth: 1920, viewportHeight: 1080});
    assert.equal(view.dim.style.display, 'none');
    view.emit({id: 3, words: [word], spotlight: false, viewportWidth: 1920, viewportHeight: 1080});
    assert.equal(view.dim.style.display, 'none');
});

test('a Windows whole-screen lookup fades in, and a closed overlay never shows the previous lookup', async () => {
    const view = await fixture('Mozilla/5.0 (Windows NT 10.0; Win64; x64)');
    const word = {text: 'target', x: 10, y: 20, w: 50, h: 18};
    view.emit({id: 1, words: [word], selected: word});
    assert.equal(view.body.animations.length, 0, 'a lookup with a pop-up does not fade the screen');
    view.close();
    assert(view.body.classList.contains('idle'));
    assert.equal(view.root.children.length, 0, 'the old words are gone and cannot be clicked');
    view.handlers.mousedown({preventDefault() {}, target: null, clientX: 20, clientY: 25});
    assert.equal(view.requests.at(-1).command, 'overlay_dismiss', 'a click while the next lookup is read cancels it');
    view.emit({id: 2, words: [word], spotlight: true});
    assert(!view.body.classList.contains('idle'));
    assert.equal(view.body.animations.length, 1);
    assert.deepEqual(JSON.parse(JSON.stringify(view.body.animations[0].keyframes)), [{opacity: 0}, {opacity: 1}]);
    view.emit({id: 2, words: [word], selected: word, spotlight: true});
    assert.equal(view.body.animations.length, 1, 'clicking a word does not fade again');
});

test('Linux whole-screen lookups appear without an opacity animation, including after reopening', async () => {
    const view = await fixture();
    const word = {text: 'target', x: 10, y: 20, w: 50, h: 18};
    view.emit({id: 1, words: [word], spotlight: true});
    assert.equal(view.body.animations.length, 0);
    assert.equal(view.root.children.length, 1);
    assert(!view.body.classList.contains('idle'));
    view.close({token: 7});
    assert(view.body.classList.contains('idle'));
    assert.equal(view.root.children.length, 0);
    view.emit({id: 2, words: [word], selected: word, spotlight: true});
    assert(!view.body.classList.contains('idle'));
    assert(view.root.children[0].classList.contains('sel'), 'the selected word still receives its highlight');
    assert.equal(view.body.animations.length, 0, 'a repeated lookup must not restart the WebKitGTK fade');
});

test('an empty click asks the backend to dismiss (pop-up first when pinned) and Esc closes the overlay', async () => {
    const view = await fixture();
    const word = {text: 'target', x: 10, y: 20, w: 50, h: 18};
    view.emit({id: 1, words: [word], spotlight: true});
    view.handlers.mousedown({preventDefault() {}, target: view.root.children[0], clientX: 20, clientY: 25});
    view.handlers.mousedown({preventDefault() {}, target: null, clientX: 500, clientY: 500});
    view.handlers.keydown({key: 'Escape'});
    assert.deepEqual(view.requests.map((r) => r.command), ['overlay_pick', 'overlay_dismiss', 'overlay_close']);
});

test('a block of text pressed off its words is outlined, and only while the lookup has one', async () => {
    const view = await fixture();
    const word = {text: 'target', x: 110, y: 120, w: 50, h: 18};
    view.emit({id: 1, words: [word], spotlight: true, block: {x: 100, y: 110, w: 300, h: 60}});
    assert.equal(view.block.style.display, 'block');
    assert.equal(view.block.style.left, '94px');
    assert.equal(view.block.style.top, '104px');
    assert.equal(view.block.style.width, '312px');
    assert.equal(view.block.style.height, '72px');
    view.emit({id: 2, words: [word]});
    assert.equal(view.block.style.display, 'none');
});

test('a closed overlay confirms its empty frame so Linux hides it only then (no flash on the next lookup)', async () => {
    const view = await fixture();
    view.emit({id: 1, words: [{text: 'old', x: 10, y: 20, w: 50, h: 18}], spotlight: true});
    view.close({token: 7});
    assert(view.body.classList.contains('idle'));
    assert.equal(view.root.children.length, 0);
    assert.equal(view.requests.at(-1).command, 'webview_blank');
    assert.equal(view.requests.at(-1).args.token, 7);
    // Windows hides at once and sends no token: nothing to confirm.
    const count = view.requests.length;
    view.close();
    assert.equal(view.requests.length, count);
});

test('a whole-screen lookup says when Google Lens failed, and a word lookup leaves it to the pop-up', async () => {
    const view = await fixture();
    const notice = view.document.getElementById('lens-notice');
    const word = {text: 'target', x: 10, y: 20, w: 50, h: 18};
    view.emit({id: 1, words: [word], notice: 'O Google Lens falhou e o OCR local (PP-OCRv6) leu no lugar: HTTP 403'});
    assert.equal(notice.style.display, 'block');
    assert(notice.textContent.includes('Google Lens falhou'));
    view.handlers.mousedown({preventDefault() {}, target: view.root.children[0], clientX: 20, clientY: 25});
    assert.equal(notice.style.display, 'none', 'the pop-up opens and says it');
    view.emit({id: 2, words: [word], selected: word, notice: 'O Google Lens falhou'});
    assert.equal(notice.style.display, 'none', 'a word lookup has the pop-up');
    view.emit({id: 3, words: [word], notice: 'O Google Lens falhou'});
    view.close();
    assert.equal(notice.style.display, 'none');
    view.emit({id: 4, words: [word]});
    assert.equal(notice.style.display, 'none', 'Lens worked');
});

test('the overlay shows no hint pill over the screen, in any lookup', async () => {
    const html = await readFile(resolve(import.meta.dirname, '../overlay.html'), 'utf8');
    assert(!html.includes('id="hint"'));
    assert(!html.includes('Esc para fechar'));
    const script = await readFile(resolve(import.meta.dirname, '../src/overlay/main.js'), 'utf8');
    assert(!script.includes("getElementById('hint')"));
});
