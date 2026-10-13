// Thin wrappers over the Rust commands.

import {invoke} from '@tauri-apps/api/core';

export const api = {
    getSettings: () => invoke('get_settings'),
    saveSettings: (settings) => invoke('save_settings', {settings}),
    engineStatus: () => invoke('engine_status'),
    ocrLanguages: () => invoke('ocr_languages'),
    listDictionaries: () => invoke('list_dictionaries'),
    addDictionary: (path) => invoke('add_dictionary', {path}),
    removeDictionary: (id) => invoke('remove_dictionary', {id}),
    updateDictionary: (id, changes) => invoke('update_dictionary', {id, enabled: changes.enabled ?? null, moveBy: changes.moveBy ?? null}),
    lookupText: (text, sentence) => invoke('lookup_text', {text, sentence: sentence ?? null}),
    scanNow: () => invoke('scan_now'),
    hidePopup: () => invoke('hide_popup'),
    setPopupPinned: (pinned) => invoke('set_popup_pinned', {pinned}),
    openExternal: (url) => invoke('open_external', {url}),
    openSettings: () => invoke('open_settings'),
};

/** Base URL of the `lkt` scheme as the WebView sees it on this platform. */
export function schemeBase() {
    return navigator.userAgent.includes('Windows') ? 'http://lkt.localhost' : 'lkt://localhost';
}

export function entryUrl(dictId, entry) {
    return `${schemeBase()}/e/${encodeURIComponent(dictId)}/${entry}`;
}

export function escapeHtml(s) {
    return String(s).replace(/[&<>"']/g, (c) => ({'&': '&amp;', '<': '&lt;', '>': '&gt;', '"': '&quot;', "'": '&#39;'})[c]);
}

/** Small, safe Markdown subset for AI answers: paragraphs, lists, bold, italic, code. */
export function renderMarkdown(text) {
    const lines = escapeHtml(text).split(/\r?\n/);
    const out = [];
    let list = null;
    const inline = (s) => s
        .replace(/`([^`]+)`/g, '<code>$1</code>')
        .replace(/\*\*([^*]+)\*\*/g, '<strong>$1</strong>')
        .replace(/(^|[^*])\*([^*\s][^*]*)\*/g, '$1<em>$2</em>');
    for (const raw of lines) {
        const line = raw.trimEnd();
        const m = /^\s*(?:[-*•]|\d+[.)])\s+(.*)$/.exec(line);
        if (m) {
            if (!list) { list = []; }
            list.push(`<li>${inline(m[1])}</li>`);
            continue;
        }
        if (list) { out.push(`<ul>${list.join('')}</ul>`); list = null; }
        const h = /^#{1,4}\s+(.*)$/.exec(line);
        if (h) { out.push(`<p><strong>${inline(h[1])}</strong></p>`); continue; }
        if (line.trim() === '') { continue; }
        out.push(`<p>${inline(line)}</p>`);
    }
    if (list) { out.push(`<ul>${list.join('')}</ul>`); }
    return out.join('');
}
