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
    updateDictionary: (id, changes) => invoke('update_dictionary', {id, enabled: changes.enabled ?? null, moveTo: changes.moveTo ?? null}),
    lookupText: (text, sentence) => invoke('lookup_text', {text, sentence: sentence ?? null}),
    scanNow: () => invoke('scan_now'),
    hidePopup: () => invoke('hide_popup'),
    setPopupPinned: (pinned) => invoke('set_popup_pinned', {pinned}),
    openExternal: (url) => invoke('open_external', {url}),
    openSettings: () => invoke('open_settings'),
    entryHtml: (dictId, entry) => invoke('entry_html', {dictId, entry}),
    captureImage: (recognition, area) => invoke('capture_image', {recognition: recognition ?? null, area}),
    listWords: () => invoke('list_words'),
    saveWord: (card) => invoke('save_word', {card}),
    deleteWord: (id) => invoke('delete_word', {id}),
    wordImage: (name) => invoke('word_image', {name}),
};

/** Settings shared by every window, with AI and Anki options filled with defaults. */
export async function loadAllSettings() {
    const [{normalizeAiOptions}, {normalizeAnkiOptions}] = await Promise.all([
        import('./ai/defaults.js'),
        import('./anki/notes.js'),
    ]);
    const settings = await api.getSettings();
    return {settings, ai: normalizeAiOptions(settings.ai), anki: normalizeAnkiOptions(settings.anki)};
}

/** Base URL of the `lkt` scheme as the WebView sees it on this platform. */
export function schemeBase() {
    return navigator.userAgent.includes('Windows') ? 'http://lkt.localhost' : 'lkt://localhost';
}

export function entryUrl(dictId, entry) {
    return `${schemeBase()}/e/${encodeURIComponent(dictId)}/${entry}`;
}

export {escapeHtml, renderMarkdown} from './markdown.js';
