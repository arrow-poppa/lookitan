// Settings files for moving the configuration between computers. API keys never leave in an
// export, and an import keeps the keys this computer already has.

export const SETTINGS_FILE_FORMAT = 'lookitan-settings';
export const SETTINGS_FILE_VERSION = 1;

/** Keys holding credentials: the Google Lens key, each AI provider's key and AnkiConnect's key. */
const SECRET_KEYS = new Set(['lensApiKey', 'apiKey']);

function isObject(value) {
    return typeof value === 'object' && value !== null && !Array.isArray(value);
}

/** A copy of `value` without any credential, at any depth. */
export function withoutSecrets(value) {
    if (Array.isArray(value)) { return value.map(withoutSecrets); }
    if (!isObject(value)) { return value; }
    const out = {};
    for (const [key, item] of Object.entries(value)) {
        if (!SECRET_KEYS.has(key)) { out[key] = withoutSecrets(item); }
    }
    return out;
}

/** The text of a settings file for `settings` (with its `ai` and `anki` options filled in). */
export function exportSettingsFile(settings, exportedAt = new Date()) {
    return `${JSON.stringify({
        format: SETTINGS_FILE_FORMAT,
        version: SETTINGS_FILE_VERSION,
        exportedAt: exportedAt.toISOString(),
        settings: withoutSecrets(settings),
    }, null, 2)}\n`;
}

/**
 * `imported` with this computer's credentials wherever the file has none, so importing a file
 * exported elsewhere keeps the keys already typed here.
 */
export function keepSecrets(imported, current) {
    if (!isObject(imported) || !isObject(current)) { return imported; }
    const out = {...imported};
    for (const [key, item] of Object.entries(current)) {
        if (SECRET_KEYS.has(key)) {
            if (typeof out[key] !== 'string' || out[key] === '') { out[key] = item; }
        } else if (isObject(item) && isObject(out[key])) {
            out[key] = keepSecrets(out[key], item);
        }
    }
    return out;
}

/**
 * The settings in the text of a settings file: an export, or a copy of Lookitan's own
 * settings.json. Throws a message for the user when the file is neither.
 */
export function parseSettingsFile(text) {
    let data;
    try {
        data = JSON.parse(String(text).replace(/^﻿/, ''));
    } catch (_e) {
        throw new Error('O arquivo não é um JSON válido.');
    }
    if (isObject(data) && data.format === SETTINGS_FILE_FORMAT) {
        if (typeof data.version === 'number' && data.version > SETTINGS_FILE_VERSION) {
            throw new Error('O arquivo foi exportado por uma versão mais nova do Lookitan.');
        }
        data = data.settings;
    }
    // A copy of settings.json has no wrapper; any settings file has at least one of these.
    if (!isObject(data) || !['hotkey', 'ocrLanguage', 'dictionaries', 'ai', 'anki'].some((key) => key in data)) {
        throw new Error('O arquivo não tem configurações do Lookitan.');
    }
    return data;
}
