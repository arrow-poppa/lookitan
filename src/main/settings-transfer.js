// Export and import of the configuration, without the API keys (see src/lib/settings-file.js).

import {ask, message, open, save as saveDialog} from '@tauri-apps/plugin-dialog';
import {invoke} from '@tauri-apps/api/core';
import {api} from '../lib/api.js';
import {icon} from '../lib/icons.js';
import {exportSettingsFile, keepSecrets, parseSettingsFile} from '../lib/settings-file.js';
import {store} from './store.js';
import {cardRow, el} from './ui.js';

const FILTERS = [{name: 'Configurações do Lookitan', extensions: ['json']}];

function report(text) {
    const state = document.getElementById('save-state');
    if (state) { state.textContent = text; }
}

export async function exportSettings() {
    const path = await saveDialog({defaultPath: 'lookitan-configuracoes.json', filters: FILTERS});
    if (!path) { return; }
    const settings = {...store.settings, ai: store.ai, anki: store.anki};
    await invoke('write_text_file', {path, content: exportSettingsFile(settings)});
    report(`Configurações exportadas · ${path}`);
}

export async function importSettings() {
    const path = await open({multiple: false, directory: false, filters: FILTERS});
    if (!path) { return; }
    let imported;
    try {
        imported = parseSettingsFile(await invoke('read_text_file', {path}));
    } catch (error) {
        await message(String(error?.message || error), {title: 'Importar configurações', kind: 'error'});
        return;
    }
    const replace = await ask('As configurações atuais serão substituídas pelas do arquivo. As chaves de API deste computador são mantidas.',
        {title: 'Importar configurações', kind: 'warning', okLabel: 'Importar', cancelLabel: 'Cancelar'});
    if (!replace) { return; }
    const current = await api.getSettings();
    const missing = await invoke('import_settings', {settings: keepSecrets(imported, current)});
    if (missing.length) {
        await message(`Estes dicionários não estão neste computador e ficaram de fora: ${missing.join(', ')}. Adicione-os de novo na aba Dicionários.`,
            {title: 'Importar configurações', kind: 'info'});
    }
    // Every tab reads the settings once; start the window again with the imported ones.
    location.reload();
}

export function transferRow() {
    const exportButton = el('button', {html: `${icon('export', 'sm')}Exportar`});
    const importButton = el('button', {html: `${icon('folder', 'sm')}Importar`});
    const failed = (error) => report(`Erro: ${error?.message || error}`);
    exportButton.addEventListener('click', () => { void exportSettings().catch(failed); });
    importButton.addEventListener('click', () => { void importSettings().catch(failed); });
    return cardRow({icon: 'settings', title: 'Exportar e importar configurações',
        desc: 'Um arquivo com todas as configurações, para levar a outro computador. As chaves de API ficam de fora.',
        control: [exportButton, importButton]});
}
