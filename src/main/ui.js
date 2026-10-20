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
 * `key` in this window only.
 */
export function collapser(key, body, onChange = () => {}) {
    const storageKey = `lookitan.collapsed.${key}`;
    let collapsed = false;
    try { collapsed = localStorage.getItem(storageKey) === '1'; } catch (_e) { /* storage unavailable */ }
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

/**
 * Lets a row of `list` be dragged to another place, as Yomitan's lists are. It uses pointer events,
 * not HTML drag and drop: Tauri's native file drop handler (on by default) keeps WebView2 on Windows
 * from delivering HTML drag events, and they are unreliable in WebKitGTK too. The row moves among
 * the others while dragged and the `.num` labels follow; `onDrop(from, to)` gets the old and new
 * index. A press on something matching `ignore` (a switch, a field) never starts a drag.
 */
export function sortableRow(list, row, {onDrop, ignore = '.switch'}) {
    let drag = null;
    row.addEventListener('pointerdown', (ev) => {
        if (ev.button !== 0 || ev.target.closest?.(ignore)) { return; }
        const rows = [...list.children];
        drag = {pointer: ev.pointerId, y: ev.clientY, from: rows.indexOf(row), to: rows.indexOf(row), moving: false,
            mids: rows.map((r) => { const b = r.getBoundingClientRect(); return b.top + b.height / 2; })};
    });
    row.addEventListener('pointermove', (ev) => {
        if (!drag || ev.pointerId !== drag.pointer) { return; }
        if (!drag.moving) {
            if (Math.abs(ev.clientY - drag.y) < 5) { return; }
            drag.moving = true;
            row.setPointerCapture?.(ev.pointerId);
            row.classList.add('dragging');
        }
        const to = dropIndex(drag.mids, ev.clientY, drag.from);
        if (to === drag.to) { return; }
        drag.to = to;
        const others = [...list.children].filter((r) => r !== row);
        list.insertBefore(row, others[to] || null);
        [...list.children].forEach((r, i) => { const num = r.querySelector('.num'); if (num) { num.textContent = String(i + 1); } });
    });
    const end = (ev) => {
        if (!drag || ev.pointerId !== drag.pointer) { return; }
        const {moving, from, to} = drag;
        drag = null;
        if (!moving) { return; }
        row.classList.remove('dragging');
        onDrop(from, ev.type === 'pointerup' ? to : from);
    };
    row.addEventListener('pointerup', end);
    row.addEventListener('pointercancel', end);
}
