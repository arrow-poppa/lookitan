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
export const LANGUAGE_TAGS = [
    'pt-BR', 'pt-PT', 'en-US', 'en-GB', 'es-ES', 'es-MX', 'fr-FR', 'de-DE', 'it-IT', 'nl-NL', 'pl-PL', 'ru-RU',
    'uk-UA', 'tr-TR', 'ja-JP', 'zh-CN', 'zh-TW', 'ko-KR', 'ar-SA', 'el-GR', 'sv-SE', 'da-DK', 'nb-NO', 'fi-FI',
    'cs-CZ', 'ro-RO', 'hu-HU', 'id-ID', 'vi-VN', 'th-TH', 'hi-IN', 'he-IL',
];

export function languageLabel(tag) {
    try {
        const name = new Intl.DisplayNames(['pt-BR'], {type: 'language'}).of(tag) || tag;
        return name.charAt(0).toUpperCase() + name.slice(1);
    } catch (_e) {
        return tag;
    }
}

export function languageOptions() {
    return LANGUAGE_TAGS.map((t) => ({value: t, label: languageLabel(t)}));
}
