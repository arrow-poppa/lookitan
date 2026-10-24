// Settings shared by the tabs of the main window, saved shortly after each change.

import {api, loadAllSettings} from '../lib/api.js';

export const store = {settings: null, ai: null, anki: null};
let timer = null;

export async function loadStore() {
    const all = await loadAllSettings();
    store.settings = all.settings;
    store.ai = all.ai;
    store.anki = all.anki;
}

let pending = null;

export function save() {
    const state = document.getElementById('save-state');
    state.textContent = 'Salvando…';
    clearTimeout(timer);
    pending = async () => {
        pending = null;
        store.settings.ai = store.ai;
        store.settings.anki = store.anki;
        try {
            await api.saveSettings(store.settings);
            state.textContent = 'Salvo';
            setTimeout(() => { if (state.textContent === 'Salvo') { state.textContent = ''; } }, 1600);
        } catch (e) {
            state.textContent = `Erro ao salvar: ${e}`;
        }
    };
    timer = setTimeout(() => pending?.(), 350);
}

/** Saves a change still waiting for its timer now, before the settings are replaced (a profile switch). */
export async function saveNow() {
    clearTimeout(timer);
    if (pending) { await pending(); }
}
