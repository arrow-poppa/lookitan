// Main window: Words, Dictionaries and Settings tabs.

import {listen} from '@tauri-apps/api/event';
import {api} from '../lib/api.js';
import {hydrateIcons} from '../lib/icons.js';
import {initDictsTab, refreshDicts} from './dicts-tab.js';
import {initSettingsTab, refreshStatus} from './settings-tab.js';
import {loadStore, store} from './store.js';
import {initWordsTab, refreshWords} from './words-tab.js';

function showTab(name) {
    for (const b of document.querySelectorAll('.tabs button')) { b.classList.toggle('on', b.dataset.tab === name); }
    for (const t of document.querySelectorAll('.tab')) { t.hidden = t.id !== `tab-${name}`; }
    localStorage.setItem('tab', name);
}

async function main() {
    hydrateIcons();
    await loadStore();
    for (const b of document.querySelectorAll('.tabs button')) {
        b.addEventListener('click', () => showTab(b.dataset.tab));
    }
    showTab(localStorage.getItem('tab') || 'settings');
    await Promise.all([
        initWordsTab(),
        initDictsTab(),
        initSettingsTab(document.getElementById('settings-col')),
    ]);
    void listen('engine-status', () => { void refreshStatus(); });
    void listen('dictionaries-changed', () => { void refreshDicts(); });
    void listen('words-changed', () => { void refreshWords(); });
    void listen('settings-changed', async () => {
        // The back end adds app profiles on its own; keep them without losing local edits.
        const fresh = await api.getSettings();
        store.settings.profiles = fresh.profiles;
    });
}

void main();
