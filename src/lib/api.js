// Thin wrappers over the Rust commands.

import {invoke} from '@tauri-apps/api/core';

export const api = {
    getSettings: () => invoke('get_settings'),
    saveSettings: (settings) => invoke('save_settings', {settings}),
    /** Settings profiles (Configurações › "Perfil"); '' is the global settings. */
    chooseSettingsProfile: (id) => invoke('choose_settings_profile', {id}),
    createSettingsProfile: (name) => invoke('create_settings_profile', {name}),
    deleteSettingsProfile: (id) => invoke('delete_settings_profile', {id}),
    engineStatus: () => invoke('engine_status'),
    installSegmenter: () => invoke('install_segmenter'),
    removeSegmenter: () => invoke('remove_segmenter'),
    installOcrModels: (engine) => invoke('install_ocr_models', {engine}),
    removeOcrModels: (engine) => invoke('remove_ocr_models', {engine}),
    ocrLanguages: () => invoke('ocr_languages'),
    listDictionaries: () => invoke('list_dictionaries'),
    addDictionary: (path) => invoke('add_dictionary', {path}),
    onDictionaryProgress: async (handler) => {
        const {listen} = await import('@tauri-apps/api/event');
        return listen('dictionary-progress', ({payload}) => handler(payload));
    },
    removeDictionary: (id) => invoke('remove_dictionary', {id}),
    updateDictionary: (id, changes) => invoke('update_dictionary', {id, enabled: changes.enabled ?? null, moveTo: changes.moveTo ?? null}),
    lookupText: (text, sentence, context) => invoke('lookup_text', {text, sentence: sentence ?? null, context: context ?? null}),
    scanNow: () => invoke('scan_now'),
    hidePopup: () => invoke('hide_popup'),
    webviewBlank: (token) => invoke('webview_blank', {token}),
    setPopupPinned: (pinned) => invoke('set_popup_pinned', {pinned}),
    openExternal: (url) => invoke('open_external', {url}),
    openSettings: () => invoke('open_settings'),
    entryHtml: (dictId, entry) => invoke('entry_html', {dictId, entry}),
    captureImage: (recognition, area) => invoke('capture_image', {recognition: recognition ?? null, area}),
    listWords: () => invoke('list_words'),
    saveWord: (card) => invoke('save_word', {card}),
    deleteWord: (id) => invoke('delete_word', {id}),
    /** A saved word's picture, cropped to `crop` ([x, y, width, height]) when given. */
    wordImage: (name, crop = null) => invoke('word_image', {name, crop: crop || null}),
    /** Starts (true) or stops recording a controller combination; see onGamepadCapture. */
    gamepadCapture: (on) => invoke('gamepad_capture', {on}),
    /** The connected controller's name, or null. */
    gamepadStatus: () => invoke('gamepad_status'),
    /** `handler({buttons, done})` as a combination is recorded. Resolves to the unlisten function. */
    onGamepadCapture: async (handler) => {
        const {listen} = await import('@tauri-apps/api/event');
        return listen('gamepad-capture', ({payload}) => handler(payload));
    },
    /** Hides the pop-up and keeps the overlay (the controller's Close). */
    popupDismiss: () => invoke('popup_dismiss'),
    /** The whole image OCR read for a lookup, with the sentence's crop: `{data, crop}`. */
    captureCardImage: (recognition) => invoke('capture_card_image', {recognition: recognition ?? null}),
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
