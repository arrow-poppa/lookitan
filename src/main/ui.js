// Small DOM helpers for the settings-style cards used across the main window.

import {escapeHtml} from '../lib/api.js';
import {icon} from '../lib/icons.js';

export function el(tag, attrs = {}, ...children) {
    const node = document.createElement(tag);
    for (const [k, v] of Object.entries(attrs)) {
        if (v === undefined || v === null || v === false) { continue; }
        if (k === 'class') { node.className = v; } else if (k === 'html') { node.innerHTML = v; } else if (k.startsWith('on')) { node.addEventListener(k.slice(2), v); } else { node.setAttribute(k, v === true ? '' : v); }
    }
    for (const c of children.flat()) {
        if (c === null || c === undefined || c === false) { continue; }
        node.append(c instanceof Node ? c : document.createTextNode(String(c)));
    }
    return node;
}

/** A settings card: icon, title, optional description and a control on the right. */
export function cardRow({icon: iconName, title, desc, control, wide = false}) {
    return el('div', {class: 'card-row'},
        el('span', {class: 'ico', html: iconName ? icon(iconName) : ''}),
        el('div', {class: 'txt'}, el('div', {class: 't'}, title), desc ? el('div', {class: 'd'}, desc) : null),
        el('div', {class: wide ? 'ctl wide' : 'ctl'}, control));
}

/**
 * A chevron button that collapses a section, like Lookupper's settings groups. `body()` returns
 * the elements to hide; `onChange(collapsed)` can restyle the header. The state is remembered per
 * `key` in this window only; until then the section starts as `startCollapsed` says.
 */
export function collapser(key, body, onChange = () => {}, startCollapsed = false) {
    const storageKey = `lookitan.collapsed.${key}`;
    let collapsed = startCollapsed;
    try {
        const stored = localStorage.getItem(storageKey);
        if (stored !== null) { collapsed = stored === '1'; }
    } catch (_e) { /* storage unavailable */ }
    const button = el('button', {class: 'ghost icon collapse', type: 'button'});
    const apply = () => {
        for (const node of body()) { if (node) { node.hidden = collapsed; } }
        button.innerHTML = icon(collapsed ? 'chevronDown' : 'chevronUp', 'sm');
        button.title = collapsed ? 'Expandir' : 'Recolher';
        button.setAttribute('aria-label', button.title);
        button.setAttribute('aria-expanded', String(!collapsed));
        onChange(collapsed);
    };
    button.addEventListener('click', () => {
        collapsed = !collapsed;
        try { localStorage.setItem(storageKey, collapsed ? '1' : '0'); } catch (_e) { /* storage unavailable */ }
        apply();
    });
    return {button, apply};
}

export function toggle(checked, onChange, {on = 'Ligado', off = 'Desligado'} = {}) {
    const input = el('input', {type: 'checkbox'});
    input.checked = Boolean(checked);
    const label = el('span', {class: 'label'}, input.checked ? on : off);
    input.addEventListener('change', () => {
        label.textContent = input.checked ? on : off;
        onChange(input.checked);
    });
    return el('label', {class: 'switch'}, label, input, el('span', {class: 'track'}));
}

export function select(options, value, onChange) {
    const s = el('select');
    for (const o of options) {
        const opt = el('option', {value: o.value}, o.label);
        if (String(o.value) === String(value)) { opt.selected = true; }
        s.append(opt);
    }
    s.addEventListener('change', () => onChange(s.value));
    return s;
}

export function textInput(value, onChange, {type = 'text', placeholder = ''} = {}) {
    const i = el('input', {type, placeholder, autocomplete: 'off', spellcheck: 'false'});
    i.value = value ?? '';
    i.addEventListener('input', () => onChange(i.value));
    return i;
}

export function textArea(value, onChange, rows = 4) {
    const t = el('textarea', {rows: String(rows), spellcheck: 'false'});
    t.value = value ?? '';
    t.addEventListener('input', () => onChange(t.value));
    return t;
}

export function segmented(options, value, onChange) {
    const wrap = el('div', {class: 'segmented'});
    for (const o of options) {
        const b = el('button', {type: 'button', class: String(o.value) === String(value) ? 'on' : '', html: `${o.icon ? icon(o.icon, 'sm') : ''}${escapeHtml(o.label)}`});
        b.addEventListener('click', () => {
            for (const x of wrap.children) { x.classList.toggle('on', x === b); }
            onChange(o.value);
        });
        wrap.append(b);
    }
    return wrap;
}

export function heading(text) {
    return el('h3', {}, text);
}

/** Languages offered for definitions, translations and explanations. */
/**
 * Languages for definitions and translations: the 53 Lookupper translates into, with its regional
 * variants (English, Spanish, Portuguese, Serbian and Chinese), named in English.
 */
export const LANGUAGES = [
    {tag: 'ar-SA', name: 'Arabic'},
    {tag: 'hy-AM', name: 'Armenian'},
    {tag: 'az-Latn-AZ', name: 'Azerbaijani'},
    {tag: 'bn', name: 'Bangla'},
    {tag: 'be-BY', name: 'Belarusian'},
    {tag: 'bs-Latn-BA', name: 'Bosnian'},
    {tag: 'bg-BG', name: 'Bulgarian'},
    {tag: 'ca', name: 'Catalan'},
    {tag: 'zh-Hans-CN', name: 'Chinese (Simplified)'},
    {tag: 'zh-Hant-TW', name: 'Chinese (Traditional)'},
    {tag: 'hr', name: 'Croatian'},
    {tag: 'cs-CZ', name: 'Czech'},
    {tag: 'da-DK', name: 'Danish'},
    {tag: 'nl-NL', name: 'Dutch'},
    {tag: 'en-GB', name: 'English (United Kingdom)'},
    {tag: 'en-US', name: 'English (United States)'},
    {tag: 'et-EE', name: 'Estonian'},
    {tag: 'fil-PH', name: 'Filipino'},
    {tag: 'fi-FI', name: 'Finnish'},
    {tag: 'fr-FR', name: 'French'},
    {tag: 'ka-GE', name: 'Georgian'},
    {tag: 'de-DE', name: 'German'},
    {tag: 'el-GR', name: 'Greek'},
    {tag: 'he-IL', name: 'Hebrew'},
    {tag: 'hi-IN', name: 'Hindi'},
    {tag: 'hu-HU', name: 'Hungarian'},
    {tag: 'id-ID', name: 'Indonesian'},
    {tag: 'it-IT', name: 'Italian'},
    {tag: 'ja-JP', name: 'Japanese'},
    {tag: 'kk-KZ', name: 'Kazakh'},
    {tag: 'ko-KR', name: 'Korean'},
    {tag: 'lv-LV', name: 'Latvian'},
    {tag: 'lt-LT', name: 'Lithuanian'},
    {tag: 'ms', name: 'Malay'},
    {tag: 'mr-IN', name: 'Marathi'},
    {tag: 'nb-NO', name: 'Norwegian Bokmål'},
    {tag: 'fa', name: 'Persian'},
    {tag: 'pl-PL', name: 'Polish'},
    {tag: 'pt-BR', name: 'Portuguese (Brazil)'},
    {tag: 'pt-PT', name: 'Portuguese (Portugal)'},
    {tag: 'ro-RO', name: 'Romanian'},
    {tag: 'ru-RU', name: 'Russian'},
    {tag: 'sr-Cyrl-RS', name: 'Serbian (Cyrillic)'},
    {tag: 'sr-Latn-RS', name: 'Serbian (Latin)'},
    {tag: 'sk-SK', name: 'Slovak'},
    {tag: 'sl-SI', name: 'Slovenian'},
    {tag: 'es-419', name: 'Spanish (Latin America)'},
    {tag: 'es-ES', name: 'Spanish (Spain)'},
    {tag: 'sw-KE', name: 'Swahili'},
    {tag: 'sv-SE', name: 'Swedish'},
    {tag: 'ta', name: 'Tamil'},
    {tag: 'te-IN', name: 'Telugu'},
    {tag: 'th-TH', name: 'Thai'},
    {tag: 'tr-TR', name: 'Turkish'},
    {tag: 'uk-UA', name: 'Ukrainian'},
    {tag: 'ur', name: 'Urdu'},
    {tag: 'uz-Latn-UZ', name: 'Uzbek'},
    {tag: 'vi-VN', name: 'Vietnamese'},
];

export const LANGUAGE_TAGS = LANGUAGES.map((l) => l.tag);

export function languageLabel(tag) {
    const known = LANGUAGES.find((l) => l.tag === tag);
    if (known) { return known.name; }
    try {
        return new Intl.DisplayNames(['en'], {type: 'language'}).of(tag) || tag;
    } catch (_e) {
        return tag;
    }
}

export function languageOptions() {
    return LANGUAGE_TAGS.map((t) => ({value: t, label: languageLabel(t)}));
}

/**
 * Where a dragged row lands: after every other row whose middle is above the pointer. `mids` are
 * the rows' vertical middles as laid out when the drag began, `from` the dragged row's index.
 */
export function dropIndex(mids, y, from) {
    let to = 0;
    mids.forEach((mid, i) => { if (i !== from && y > mid) { to++; } });
    return to;
}

/** The nearest ancestor of `node` that scrolls vertically, or null. */
function scrollParent(node) {
    const style = globalThis.getComputedStyle;
    for (let at = node?.parentElement; at && style; at = at.parentElement) {
        const overflow = style(at).overflowY;
        if ((overflow === 'auto' || overflow === 'scroll') && at.scrollHeight > at.clientHeight) { return at; }
    }
    return null;
}

/**
 * Lets a row of `list` be dragged to another place, as Yomitan's lists are. It uses pointer events,
 * not HTML drag and drop: Tauri's native file drop handler (on by default) keeps WebView2 on Windows
 * from delivering HTML drag events, and they are unreliable in WebKitGTK too. The pointer is
 * captured on the press, and the row follows it with a transform while the others slide aside to
 * open its place; nothing moves in the document until the drop. (Until 2026-11-24 the row itself was
 * moved among the others while dragged, which released the pointer capture: the row then lagged,
 * stuck or missed the release. The user found it "strange".) Near the top or bottom of a scrolled
 * pane the pane scrolls. The `.num` labels show the order being made; `onDrop(from, to)` gets the
 * old and new index. A press on something matching `ignore` (a switch, a field) never starts a drag.
 */
export function sortableRow(list, row, {onDrop, ignore = '.switch'}) {
    let drag = null;
    const rowsOf = () => [...list.children];
    const scrolled = () => (drag?.scroller ? drag.scroller.scrollTop - drag.scrollTop : 0);
    const place = (clientY) => {
        const dy = clientY - drag.y + scrolled();
        row.style.transform = `translateY(${dy}px)`;
        const box = drag.boxes[drag.from];
        const to = dropIndex(drag.mids, box.top + box.height / 2 + dy, drag.from);
        if (to === drag.to) { return; }
        drag.to = to;
        drag.rows.forEach((other, i) => {
            if (other === row) { return; }
            const shift = i > drag.from && i <= to ? -drag.step : i < drag.from && i >= to ? drag.step : 0;
            other.style.transform = shift ? `translateY(${shift}px)` : '';
        });
        const order = drag.rows.filter((r) => r !== row);
        order.splice(to, 0, row);
        order.forEach((r, i) => { const num = r.querySelector?.('.num'); if (num) { num.textContent = String(i + 1); } });
    };
    const autoScroll = () => {
        if (!drag?.moving || !drag.scroller) { return; }
        const box = drag.scroller.getBoundingClientRect();
        const edge = 36;
        const speed = drag.lastY < box.top + edge ? -(box.top + edge - drag.lastY) / 3
            : drag.lastY > box.bottom - edge ? (drag.lastY - (box.bottom - edge)) / 3 : 0;
        if (speed) {
            drag.scroller.scrollTop += Math.max(-14, Math.min(14, speed));
            place(drag.lastY);
        }
        drag.frame = globalThis.requestAnimationFrame?.(autoScroll);
    };
    row.addEventListener('pointerdown', (ev) => {
        if (ev.button !== 0 || ev.target.closest?.(ignore)) { return; }
        const rows = rowsOf();
        const boxes = rows.map((r) => r.getBoundingClientRect());
        const from = rows.indexOf(row);
        const next = boxes[from + 1] || null;
        const previous = boxes[from - 1] || null;
        const step = next ? next.top - boxes[from].top : previous ? boxes[from].top - previous.top : boxes[from].height;
        const scroller = scrollParent(list);
        drag = {pointer: ev.pointerId, y: ev.clientY, lastY: ev.clientY, from, to: from, moving: false, rows, boxes, step,
            mids: boxes.map((b) => b.top + b.height / 2), scroller, scrollTop: scroller ? scroller.scrollTop : 0, frame: 0};
        // Captured at once, so every move and the release reach this row wherever the pointer goes.
        try { row.setPointerCapture?.(ev.pointerId); } catch (_e) { /* the pointer is already gone */ }
    });
    row.addEventListener('pointermove', (ev) => {
        if (!drag || ev.pointerId !== drag.pointer) { return; }
        drag.lastY = ev.clientY;
        if (!drag.moving) {
            if (Math.abs(ev.clientY - drag.y) < 5) { return; }
            drag.moving = true;
            row.classList.add('dragging');
            list.classList.add('sorting');
            autoScroll();
        }
        ev.preventDefault?.();
        place(ev.clientY);
    });
    const end = (ev) => {
        if (!drag || ev.pointerId !== drag.pointer) { return; }
        const {moving, from, to, rows, frame} = drag;
        drag = null;
        if (frame) { globalThis.cancelAnimationFrame?.(frame); }
        try { row.releasePointerCapture?.(ev.pointerId); } catch (_e) { /* already released */ }
        if (!moving) { return; }
        row.classList.remove('dragging');
        list.classList.remove('sorting');
        for (const r of rows) { r.style.transform = ''; }
        const landed = ev.type === 'pointerup' ? to : from;
        // The row takes its new place now (the capture is over), so the list shows the new order
        // while it is saved.
        if (landed !== from) { list.insertBefore(row, rows.filter((r) => r !== row)[landed] || null); }
        onDrop(from, landed);
    };
    row.addEventListener('pointerup', end);
    row.addEventListener('pointercancel', end);
    row.addEventListener('lostpointercapture', (ev) => { if (drag?.moving) { end({...ev, type: 'pointercancel', pointerId: drag.pointer}); } });
}
