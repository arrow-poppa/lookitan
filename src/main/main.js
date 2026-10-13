// Main window: Words, Dictionaries and Settings tabs.

import {listen} from '@tauri-apps/api/event';
import {api} from '../lib/api.js';
import {hydrateIcons} from '../lib/icons.js';
import {initDictsTab, refreshDicts} from './dicts-tab.js';
import {initSettingsTab, refreshStatus} from './settings-tab.js';
import {loadStore, store} from './store.js';
import {initWordsTab, refreshWords} from './words-tab.js';

function showTab(name) {
    if (!['words', 'dicts', 'settings'].includes(name)) { name = 'settings'; }
    for (const b of document.querySelectorAll('.tabs button')) { b.classList.toggle('on', b.dataset.tab === name); }
    for (const t of document.querySelectorAll('.tab')) { t.hidden = t.id !== `tab-${name}`; }
    localStorage.setItem('tab', name);
}

function reportError(error) {
    console.error(error);
    const state = document.getElementById('save-state');
    state.textContent = `Erro: ${error?.message || error}`;
    state.setAttribute('role', 'alert');
}

async function main() {
    hydrateIcons();
    for (const b of document.querySelectorAll('.tabs button')) {
        b.addEventListener('click', () => showTab(b.dataset.tab));
    }
    showTab(localStorage.getItem('tab') || 'settings');
    const settingsRoot = document.getElementById('settings-col');
    settingsRoot.textContent = 'Carregando configurações…';
    await loadStore();
    // Subscribe before the initial requests so a slow OCR status cannot prevent updates.
    void listen('engine-status', () => { void refreshStatus().catch(reportError); }).catch(reportError);
    void listen('dictionaries-changed', () => { void refreshDicts().catch(reportError); }).catch(reportError);
    void listen('words-changed', () => { void refreshWords().catch(reportError); }).catch(reportError);
    void listen('navigate-settings', (event) => {
        showTab('settings');
        if (event.payload?.section === 'anki') {
            document.getElementById('anki-settings')?.scrollIntoView?.({behavior: 'smooth', block: 'start'});
        }
    }).catch(reportError);
    void listen('settings-changed', async () => {
        // The back end adds app profiles on its own; keep them without losing local edits.
        try {
            const fresh = await api.getSettings();
            store.settings.profiles = fresh.profiles;
        } catch (error) { reportError(error); }
    }).catch(reportError);
    await Promise.all([
        initWordsTab().catch(reportError),
        initDictsTab().catch(reportError),
        initSettingsTab(settingsRoot).catch(reportError),
    ]);
}

void main().catch(reportError);
