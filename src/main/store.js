// Settings shared by the tabs of the main window, saved shortly after each change.

import {api, loadAllSettings} from '../lib/api.js';

export const store = {settings: null, ai: null, anki: null};
let timer = null;

/** Run before each save, to fold edits made on a copy back into the store (AI profiles). */
const beforeSave = [];
export function onBeforeSave(hook) { beforeSave.push(hook); }

export async function loadStore() {
    const all = await loadAllSettings();
    store.settings = all.settings;
    store.ai = all.ai;
    store.anki = all.anki;
}

export function save() {
    for (const hook of beforeSave) { hook(); }
    const state = document.getElementById('save-state');
    state.textContent = 'Salvando…';
    clearTimeout(timer);
    timer = setTimeout(async () => {
        store.settings.ai = store.ai;
        store.settings.anki = store.anki;
        try {
            await api.saveSettings(store.settings);
            state.textContent = 'Salvo';
            setTimeout(() => { if (state.textContent === 'Salvo') { state.textContent = ''; } }, 1600);
        } catch (e) {
            state.textContent = `Erro ao salvar: ${e}`;
        }
    }, 350);
}
