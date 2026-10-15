import assert from 'node:assert/strict';
import test from 'node:test';
import {exportSettingsFile, keepSecrets, parseSettingsFile, withoutSecrets} from '../src/lib/settings-file.js';

const settings = {
    hotkey: 'Alt+D',
    lensApiKey: 'lens-secret',
    profiles: [{executable: 'game.exe', freeze: 'frame'}],
    ai: {provider: 'gemini', openai: {apiKey: 'sk-secret', model: 'gpt-4o-mini'}, gemini: {apiKey: 'g-secret', model: 'gemini-2.5-flash'}},
    anki: {apiKey: 'anki-secret', server: 'http://127.0.0.1:8765', contextMode: 'paragraph'},
};

test('an export leaves every API key out and keeps the rest', () => {
    const text = exportSettingsFile(settings, new Date('2026-10-15T00:00:00Z'));
    assert(!/secret/.test(text), 'no key may be written to the file');
    const file = JSON.parse(text);
    assert.equal(file.format, 'lookitan-settings');
    assert.equal(file.settings.hotkey, 'Alt+D');
    assert.equal(file.settings.ai.gemini.model, 'gemini-2.5-flash');
    assert.equal(file.settings.anki.contextMode, 'paragraph');
    assert.deepEqual(file.settings.profiles, settings.profiles);
    assert.equal(settings.ai.openai.apiKey, 'sk-secret', 'the settings in use are not changed');
});

test('an import keeps the keys of this computer and the file wins elsewhere', () => {
    const here = {...settings, hotkey: 'Ctrl+Q', ai: {...settings.ai, provider: 'openai'}};
    const imported = keepSecrets(parseSettingsFile(exportSettingsFile(settings)), here);
    assert.equal(imported.hotkey, 'Alt+D');
    assert.equal(imported.ai.provider, 'gemini');
    assert.equal(imported.lensApiKey, 'lens-secret');
    assert.equal(imported.ai.openai.apiKey, 'sk-secret');
    assert.equal(imported.ai.gemini.apiKey, 'g-secret');
    assert.equal(imported.anki.apiKey, 'anki-secret');
});

test('a copy of settings.json imports with its own keys', () => {
    const parsed = parseSettingsFile(`﻿${JSON.stringify({hotkey: 'Alt+D', ai: {openai: {apiKey: 'from-file'}}})}`);
    assert.equal(keepSecrets(parsed, settings).ai.openai.apiKey, 'from-file');
});

test('files that are not Lookitan settings are refused', () => {
    assert.throws(() => parseSettingsFile('not json'), /JSON válido/);
    assert.throws(() => parseSettingsFile('{"words": []}'), /não tem configurações/);
    assert.throws(() => parseSettingsFile('{"format": "lookitan-settings", "version": 99, "settings": {}}'), /versão mais nova/);
});
