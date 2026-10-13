// Anki settings, following Yomitan: AnkiConnect server, deck, note type, field markers, tags and
// duplicate handling. Uses the AnkiConnect client ported from yomitan-byok-ai.

import {escapeHtml} from '../lib/api.js';
import {icon} from '../lib/icons.js';
import {createConnection, fieldValue, MARKERS, suggestMarker} from '../lib/anki/notes.js';
import {store, save} from './store.js';
import {cardRow, el, select, textInput, toggle} from './ui.js';

let decks = [];
let models = [];
let modelFields = [];
let connection = {ok: null, message: ''};

async function refreshAnki() {
    const anki = store.anki;
    const conn = createConnection({...anki, enable: true});
    try {
        const version = await conn.getVersion();
        decks = await conn.getDeckNames();
        models = await conn.getModelNames();
        const format = anki.cardFormats[0];
        modelFields = format.model ? await conn.getModelFieldNames(format.model) : [];
        connection = {ok: true, message: `Conectado ao AnkiConnect (versão ${version}).`};
    } catch (e) {
        decks = [];
        models = [];
        modelFields = [];
        connection = {ok: false, message: `Sem conexão com o Anki: ${e?.message || e}. Abra o Anki com o add-on AnkiConnect instalado.`};
    }
}

function fieldsEditor(root) {
    const format = store.anki.cardFormats[0];
    if (!format.model) {
        return el('p', {class: 'note'}, 'Escolha o tipo de nota para configurar os campos.');
    }
    if (modelFields.length === 0) {
        return el('p', {class: 'note'}, 'Conecte ao Anki para ver os campos deste tipo de nota.');
    }
    const table = el('table', {class: 'fields-table'});
    modelFields.forEach((name, i) => {
        if (!(name in format.fields)) {
            format.fields[name] = {value: suggestMarker(name, i), overwriteMode: 'coalesce'};
        }
        const input = textInput(fieldValue(format.fields[name]), (v) => {
            format.fields[name] = {value: v, overwriteMode: 'coalesce'};
            save();
        });
        const menu = select([{value: '', label: 'Marcador…'}, ...MARKERS.map((m) => ({value: m, label: `{${m}}`}))], '', (v) => {
            if (!v) { return; }
            input.value = `${input.value}{${v}}`;
            format.fields[name] = {value: input.value, overwriteMode: 'coalesce'};
            menu.value = '';
            save();
        });
        menu.style.width = '170px';
        table.append(el('tr', {}, el('td', {}, name), el('td', {}, input), el('td', {}, menu)));
    });
    for (const key of Object.keys(format.fields)) {
        if (!modelFields.includes(key)) { delete format.fields[key]; }
    }
    save();
    return table;
}

export async function renderAnkiSection(root) {
    const anki = store.anki;
    const format = anki.cardFormats[0];
    if (anki.enable) { await refreshAnki(); }
    root.replaceChildren();

    root.append(cardRow({icon: 'cards', title: 'Integração com o Anki', desc: 'Adiciona cards pelo botão + do popup, via o add-on AnkiConnect.',
        control: toggle(anki.enable, async (v) => { anki.enable = v; save(); await renderAnkiSection(root); })}));
    if (!anki.enable) { return; }

    const status = el('div', {class: connection.ok ? 'd ok' : 'd error'}, connection.message);
    const retry = el('button', {html: `${icon('refresh', 'sm')}Verificar`});
    retry.addEventListener('click', async () => { await renderAnkiSection(root); });
    root.append(el('div', {class: 'card-row'}, el('span', {class: 'ico', html: icon('sync')}),
        el('div', {class: 'txt'}, el('div', {class: 't'}, 'Conexão'), status),
        el('div', {class: 'ctl wide'},
            textInput(anki.server, (v) => { anki.server = v.trim(); save(); }, {placeholder: 'http://127.0.0.1:8765'}),
            retry)));
    root.append(cardRow({icon: 'settings', title: 'Chave da API do AnkiConnect', desc: 'Só se você definiu "apiKey" na configuração do AnkiConnect.', wide: true,
        control: textInput(anki.apiKey, (v) => { anki.apiKey = v.trim(); save(); }, {type: 'password'})}));

    const deckOptions = [{value: '', label: 'Escolha o baralho'}, ...decks.map((d) => ({value: d, label: d}))];
    if (format.deck && !decks.includes(format.deck)) { deckOptions.push({value: format.deck, label: format.deck}); }
    const modelOptions = [{value: '', label: 'Escolha o tipo de nota'}, ...models.map((m) => ({value: m, label: m}))];
    if (format.model && !models.includes(format.model)) { modelOptions.push({value: format.model, label: format.model}); }
    root.append(cardRow({icon: 'cards', title: 'Baralho', wide: true, control: select(deckOptions, format.deck, (v) => { format.deck = v; save(); })}));
    root.append(cardRow({icon: 'list', title: 'Tipo de nota', wide: true,
        control: select(modelOptions, format.model, async (v) => {
            format.model = v;
            format.fields = {};
            save();
            await renderAnkiSection(root);
        })}));

    const fieldsCard = el('div', {class: 'card-row stack'},
        el('div', {class: 'head'}, el('span', {class: 'ico', html: icon('list')}),
            el('div', {class: 'txt'}, el('div', {class: 't'}, 'Campos'), el('div', {class: 'd', html: `Marcadores como no Yomitan: ${escapeHtml('{expression} {glossary} {sentence} {cloze-body} {screenshot} {ai-explanation}')}…`}))),
        fieldsEditor(root));
    root.append(fieldsCard);

    root.append(cardRow({icon: 'list', title: 'Tags', desc: 'Separadas por espaço.', wide: true,
        control: textInput((anki.tags || []).join(' '), (v) => { anki.tags = v.split(/\s+/).filter(Boolean); save(); })}));
    root.append(cardRow({icon: 'alert', title: 'Verificar duplicadas', desc: 'Mostra ✓ no popup quando a palavra já está no Anki.',
        control: toggle(anki.checkForDuplicates, (v) => { anki.checkForDuplicates = v; save(); })}));
    root.append(cardRow({icon: 'cards', title: 'Escopo das duplicadas', wide: true,
        control: select([
            {value: 'collection', label: 'Coleção inteira'},
            {value: 'deck', label: 'Só o baralho escolhido'},
            {value: 'deck-root', label: 'O baralho raiz e seus sub-baralhos'},
        ], anki.duplicateScope, (v) => { anki.duplicateScope = v; save(); })}));
    root.append(cardRow({icon: 'plus', title: 'Quando já existir', wide: true,
        control: select([
            {value: 'new', label: 'Permitir adicionar outra nota'},
            {value: 'prevent', label: 'Não adicionar'},
        ], anki.duplicateBehavior === 'prevent' ? 'prevent' : 'new', (v) => { anki.duplicateBehavior = v; save(); })}));
    root.append(cardRow({icon: 'focus', title: 'Captura de tela no card ({screenshot})', wide: true,
        control: select([
            {value: 'sentence', label: 'Só a frase'},
            {value: 'whole', label: 'A tela inteira'},
        ], anki.screenshotArea, (v) => { anki.screenshotArea = v; save(); })}));
    root.append(cardRow({icon: 'pause', title: 'Suspender cards novos', control: toggle(anki.suspendNewCards, (v) => { anki.suspendNewCards = v; save(); })}));
}
