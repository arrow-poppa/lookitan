// Dictionaries tab: the pop-up order on the left (drag a row or press Alt+↑/↓ to reorder, switch to
// enable) and the selected dictionary's details on the right. The built-in AI dictionaries carry the BYOK provider settings.

import {ask, open} from '@tauri-apps/plugin-dialog';
import {api, escapeHtml} from '../lib/api.js';
import {DEFAULT_PROMPT, aiOptionsFor} from '../lib/ai/defaults.js';
import {DEFAULT_MODELS, explain} from '../lib/ai/explain.js';
import {CARD_PROMPT, CONTEXT_PROMPT, WORD_TRANSLATION_PROMPT, fillLanguages} from '../lib/ai/word-card.js';
import {IMAGE_QUERY_PROMPT, IMAGE_QUERY_SYSTEM_PROMPT, cleanQuery, imagePrompts, searchImages} from '../lib/images.js';
import {IMAGE_GEN_MAX_COUNT, IMAGE_GEN_MODELS, IMAGE_GEN_PRESETS, IMAGE_GEN_PROMPT, chosenImageGenPrompt, generateImages, imageGenPrompt, newImageGenPromptId} from '../lib/image-gen.js';
import {icon} from '../lib/icons.js';
import {store, save} from './store.js';
import {cardRow, dropIndex, el, languageOptions, select, sortableRow, textArea, textInput, toggle} from './ui.js';

let dicts = [];
let selectedId = null;
/** The row to focus after the list is drawn again (keyboard reordering). */
let focusId = null;
/** Set by a drag that just ended, so its click does not count as choosing the row. */
let dragEnded = false;

const AI_NAMES = {'ai-word': 'IA — palavra no contexto', 'ai-context': 'Tradutor de contexto', 'ai-images': 'Imagens', 'ai-image-gen': 'Imagens IA'};

function displayName(d) {
    return AI_NAMES[d.id] || d.name || d.path;
}

export async function initDictsTab() {
    document.getElementById('add-dict').addEventListener('click', addDictionary);
    await refreshDicts();
}

export async function refreshDicts() {
    dicts = await api.listDictionaries();
    renderList();
}

async function addDictionary() {
    // As in Lookupper, a dictionary normally comes as an archive (ZIP, RAR, 7z, TAR, gzip/bzip2/xz),
    // which is unpacked into Lookitan's own folder. A main file can also be chosen; it is then read
    // where it is.
    const path = await open({multiple: false, directory: false, filters: [
        {name: 'Dicionário compactado', extensions: ['zip', 'rar', '7z', 'tar', 'tgz', 'gz', 'bz2', 'xz', 'txz', 'tbz2']},
        {name: 'Arquivo de dicionário', extensions: ['mdx', 'ifo', 'dsl', 'dz', 'xml', 'gz', 'bz2', 'txt', 'u8', 'json', 'jsonl', 'slob', 'noun', 'verb', 'adj', 'adv']},
    ]});
    if (!path) { return; }
    const btn = document.getElementById('add-dict');
    const label = btn.innerHTML;
    btn.disabled = true;
    btn.textContent = 'Abrindo dicionário…';
    let stop;
    try {
        stop = await api.onDictionaryProgress?.(({stage, entries, bytes}) => {
            btn.textContent = stage === 'unpacking' ? `Extraindo… ${Math.round(Number(bytes || 0) / 1048576).toLocaleString('pt-BR')} MB`
                : stage === 'indexing' ? `Indexando… ${Number(entries).toLocaleString('pt-BR')} verbetes` : 'Lendo dicionário…';
        });
        dicts = await api.addDictionary(path);
        selectedId = dicts[dicts.length - 1]?.id || selectedId;
    } catch (e) {
        alertDetail(String(e));
    } finally {
        stop?.();
        btn.innerHTML = label;
        btn.disabled = false;
    }
    renderList();
}

function alertDetail(text) {
    document.getElementById('dict-detail').prepend(el('p', {class: 'error'}, text));
}

export {dropIndex};

async function moveDictionary(id, to) {
    if (to < 0 || to >= dicts.length || dicts.findIndex((d) => d.id === id) === to) { return; }
    dicts = await api.updateDictionary(id, {moveTo: to});
    renderList();
}

function renderList() {
    const list = document.getElementById('dict-list');
    list.replaceChildren();
    if (!selectedId || !dicts.some((d) => d.id === selectedId)) { selectedId = dicts[0]?.id || null; }
    dicts.forEach((d, i) => {
        const sw = toggle(d.enabled, async (v) => { dicts = await api.updateDictionary(d.id, {enabled: v}); renderList(); }, {on: '', off: ''});
        sw.addEventListener('click', (ev) => ev.stopPropagation());
        const li = el('li', {class: d.id === selectedId ? 'sel' : '', tabindex: '0', title: 'Arraste para mudar a ordem no popup (ou Alt+↑/↓)'},
            el('span', {class: 'grip', html: icon('drag', 'sm')}),
            el('span', {class: 'num'}, String(i + 1)),
            d.kind !== 'mdict' ? el('span', {class: 'faint', html: icon('cloud', 'sm')}) : null,
            el('span', {class: 'name'}, displayName(d)),
            d.error ? el('span', {class: 'err', title: d.error, html: icon('alert', 'sm')}) : null,
            sw);
        li.addEventListener('click', () => {
            if (dragEnded) { return; }
            selectedId = d.id;
            renderList();
        });
        li.addEventListener('keydown', (ev) => {
            if (!ev.altKey || (ev.key !== 'ArrowUp' && ev.key !== 'ArrowDown')) { return; }
            ev.preventDefault?.();
            focusId = d.id;
            void moveDictionary(d.id, i + (ev.key === 'ArrowUp' ? -1 : 1));
        });
        sortableRow(list, li, {
            ignore: '.switch',
            onDrop: (from, to) => {
                dragEnded = true;
                setTimeout(() => { dragEnded = false; }, 0);
                if (to === from) { renderList(); return; }
                void moveDictionary(d.id, to).catch((e) => { alertDetail(String(e)); renderList(); });
            },
        });
        list.append(li);
        if (d.id === focusId) { focusId = null; li.focus?.(); }
    });
    renderDetail();
}

function renderDetail() {
    const root = document.getElementById('dict-detail');
    root.replaceChildren();
    const d = dicts.find((x) => x.id === selectedId);
    if (!d) {
        root.append(el('div', {class: 'empty', html: `${icon('book', 'lg')}<strong>Nenhum dicionário</strong>Adicione um dicionário local ou um arquivo ZIP.`}));
        return;
    }
    if (d.id === 'ai-word') { renderAiWord(root); return; }
    if (d.id === 'ai-context') { renderAiContext(root); return; }
    if (d.id === 'ai-images') { renderAiImages(root); return; }
    if (d.id === 'ai-image-gen') { renderAiImageGen(root); return; }
    renderMdict(root, d);
}

function renderMdict(root, d) {
    const remove = el('button', {html: `${icon('trash', 'sm')}Remover dicionário`});
    remove.addEventListener('click', async () => {
        const copy = d.installed;
        const message = copy
            ? `Remover "${d.name || d.path}"? Os arquivos que o Lookitan extraiu para a pasta dele serão apagados; o arquivo original que você importou não é alterado.`
            : `Remover "${d.name || d.path}" da lista? O arquivo do dicionário não é apagado.`;
        if (!await ask(message, {title: 'Remover dicionário', kind: 'warning', okLabel: 'Remover', cancelLabel: 'Cancelar'})) { return; }
        dicts = await api.removeDictionary(d.id);
        selectedId = null;
        renderList();
    });
    root.append(
        el('h2', {}, d.name || d.path),
        el('p', {class: 'lead'}, d.error ? `Erro ao abrir: ${d.error}` : (d.installed
            ? 'Dicionário importado. Os arquivos foram extraídos para a pasta do Lookitan e são lidos de lá, sem conversão.'
            : 'Dicionário local, lido do lugar onde está. Estilos, imagens e áudio vêm da mesma pasta ou dos arquivos .mdd.')),
        d.description ? el('p', {class: 'muted'}, d.description) : null,
        ...(d.warnings || []).map((text) => el('p', {class: 'error'}, text)),
        el('h3', {}, 'Informações'),
        el('div', {class: 'group'},
            cardRow({icon: 'book', title: 'Formato', control: el('span', {class: 'muted'}, d.format || 'Dicionário local')}),
            cardRow({icon: 'list', title: 'Verbetes', control: el('span', {class: 'muted'}, d.loaded ? d.entries.toLocaleString('pt-BR') : (d.enabled ? 'Carregando…' : 'Desativado'))}),
            cardRow({icon: 'folder', title: 'Arquivos de recursos (.mdd)', control: el('span', {class: 'muted'}, String(d.resourceFiles))}),
            cardRow({icon: 'folder', title: d.installed ? 'Pasta' : 'Arquivo', desc: d.path, control: remove})),
    );
}

function languageRows() {
    const s = store.settings;
    return [
        cardRow({icon: 'book', title: 'Idioma da definição', wide: true,
            control: select(languageOptions(), s.definitionLanguage, (v) => { s.definitionLanguage = v; save(); })}),
        cardRow({icon: 'translate', title: 'Idioma da tradução e da explicação', desc: 'As traduções e explicações da IA aparecem neste idioma.', wide: true,
            control: select(languageOptions(), s.translationLanguage, (v) => { s.translationLanguage = v; save(); })}),
    ];
}

function aiControl(path, label, control) {
    control.setAttribute('data-setting', `ai.${path}`);
    control.setAttribute('id', `ai-${path.replaceAll('.', '-')}`);
    control.setAttribute('aria-label', label);
    return control;
}

/**
 * The AI provider fields of one AI dictionary. "IA — palavra no contexto" keeps its own at the top
 * of the AI options; the context translator and the pictures keep theirs in
 * `ai.sectionProviders[<id>]` (see aiOptionsFor), so each tab changes only its own section.
 */
function providerSection(ai = store.ai, prefix = '') {
    const ctl = (path, label, control) => aiControl(`${prefix}${path}`, label, control);
    const cfg = () => ai[ai.provider];
    const wrap = el('div', {class: 'group'});
    const draw = () => {
        wrap.replaceChildren();
        const models = (DEFAULT_MODELS[ai.provider] || []);
        const modelInput = ctl(`${ai.provider}.model`, 'Modelo', textInput(cfg().model, (v) => { cfg().model = v.trim(); save(); }, {placeholder: models[0] || 'nome-do-modelo'}));
        const listId = `models-${prefix.replace(/\W/g, '-')}${ai.provider}`;
        modelInput.setAttribute('list', listId);
        const datalist = el('datalist', {id: listId}, ...models.map((m) => el('option', {value: m})));
        wrap.append(...[
            cardRow({icon: 'cloud', title: 'Provedor', wide: true,
                control: ctl('provider', 'Provedor', select([
                    {value: 'openai', label: 'OpenAI'},
                    {value: 'gemini', label: 'Google Gemini'},
                    {value: 'deepseek', label: 'DeepSeek'},
                    {value: 'custom', label: 'Personalizado (OpenRouter, Ollama, LM Studio…)'},
                ], ai.provider, (v) => { ai.provider = v; save(); draw(); }))}),
            ai.provider === 'custom' ? cardRow({icon: 'cloud', title: 'Endpoint', desc: 'URL completa da API compatível com OpenAI. Usada exatamente como informada.', wide: true,
                control: ctl('custom.endpoint', 'Endpoint', textInput(ai.custom.endpoint, (v) => { ai.custom.endpoint = v.trim(); save(); }, {placeholder: 'https://openrouter.ai/api/v1/chat/completions'}))}) : null,
            cardRow({icon: 'settings', title: 'Chave da API', wide: true,
                control: ctl(`${ai.provider}.apiKey`, 'Chave da API', textInput(cfg().apiKey, (v) => { cfg().apiKey = v.trim(); save(); }, {type: 'password'}))}),
            cardRow({icon: 'sparkle', title: 'Modelo', wide: true, control: el('div', {style: 'width:100%'}, modelInput, datalist)}),
        ].filter(Boolean));
        if (ai.provider === 'gemini') {
            wrap.append(cardRow({icon: 'sparkle', title: 'Nível de raciocínio (Gemini 3)', wide: true,
                control: ctl('gemini.thinkingLevel', 'Nível de raciocínio (Gemini 3)', select([{value: '', label: 'Padrão do modelo'}, ...['MINIMAL', 'LOW', 'MEDIUM', 'HIGH'].map((v) => ({value: v, label: v}))],
                    ai.gemini.thinkingLevel, (v) => { ai.gemini.thinkingLevel = v; save(); }))}));
        }
        if (ai.provider === 'deepseek' || ai.provider === 'custom') {
            const intensities = ai.provider === 'custom' ? [
                {value: '', label: 'Padrão do modelo'}, {value: 'none', label: 'Nenhuma (none)'},
                {value: 'minimal', label: 'Mínima (minimal)'}, {value: 'low', label: 'Baixa (low)'},
                {value: 'medium', label: 'Média (medium)'}, {value: 'high', label: 'Alta (high)'},
                {value: 'xhigh', label: 'Muito alta (xhigh)'}, {value: 'max', label: 'Máxima (max)'},
                {value: 'custom', label: 'Personalizada'},
            ] : [{value: '', label: 'Padrão do modelo'}, {value: 'high', label: 'Alta'}, {value: 'max', label: 'Máxima'}];
            wrap.append(
                cardRow({icon: 'sparkle', title: 'Raciocínio (thinking)', wide: true,
                    desc: 'Use o padrão do modelo ou escolha se o raciocínio deve ficar ligado ou desligado.',
                    control: ctl(`${ai.provider}.thinkingMode`, 'Raciocínio (thinking)', select([
                        {value: '', label: 'Padrão'}, {value: 'enabled', label: 'Ligado'}, {value: 'disabled', label: 'Desligado'},
                    ], cfg().thinkingMode, (v) => { cfg().thinkingMode = v; save(); }))}),
                cardRow({icon: 'sparkle', title: 'Esforço do raciocínio (thinking)', wide: true,
                    desc: 'O modelo precisa aceitar o nível escolhido. Máxima equivale a xhigh no OpenRouter.',
                    control: ctl(`${ai.provider}.thinkingIntensity`, 'Esforço do raciocínio (thinking)', select(intensities, cfg().thinkingIntensity,
                        (v) => { cfg().thinkingIntensity = v; save(); draw(); }))}),
            );
            if (ai.provider === 'custom') {
                wrap.append(cardRow({icon: 'sparkle', title: 'Valor personalizado de thinking',
                    desc: 'Usado quando o esforço é Personalizada. OpenRouter: medium, 2000 ou {"max_tokens":2000}. Outras APIs: reasoning_effort.', wide: true,
                    control: ctl('custom.customThinkingIntensity', 'Valor personalizado de thinking', textInput(ai.custom.customThinkingIntensity,
                        (v) => { ai.custom.customThinkingIntensity = v; save(); }, {placeholder: 'medium, 2000 ou {"max_tokens":2000}'}))}));
            }
        }
        if (ai.provider === 'custom') {
            wrap.append(
                cardRow({icon: 'list', title: 'Roteamento de provedores (OpenRouter)', wide: true,
                    control: ctl('custom.providerRoutingMode', 'Roteamento de provedores (OpenRouter)', select([
                        {value: '', label: 'Sem roteamento'}, {value: 'order', label: 'Ordem'}, {value: 'only', label: 'Somente'}, {value: 'ignore', label: 'Ignorar'},
                    ], ai.custom.providerRoutingMode, (v) => { ai.custom.providerRoutingMode = v; save(); }))}),
                cardRow({icon: 'list', title: 'Provedores do OpenRouter', desc: 'Slugs separados por vírgula, na ordem desejada.', wide: true,
                    control: ctl('custom.providerSlugs', 'Provedores do OpenRouter', textInput(ai.custom.providerSlugs,
                        (v) => { ai.custom.providerSlugs = v; save(); }, {placeholder: 'deepinfra/turbo, fireworks'}))}),
                cardRow({icon: 'list', title: 'Permitir outros provedores se os escolhidos falharem',
                    control: ctl('custom.providerAllowFallbacks', 'Permitir outros provedores', toggle(ai.custom.providerAllowFallbacks !== false, (v) => { ai.custom.providerAllowFallbacks = v; save(); }))}),
                el('div', {class: 'card-row stack'},
                    el('div', {class: 'head'}, el('span', {class: 'ico', html: icon('settings')}), el('div', {class: 'txt'}, el('div', {class: 't'}, 'JSON extra no corpo da requisição'), el('div', {class: 'd'}, 'Mesclado ao corpo de cada pedido. Ex.: {"top_p": 0.9}'))),
                    ctl('custom.requestBodyJson', 'JSON extra no corpo da requisição', textArea(ai.custom.requestBodyJson, (v) => { ai.custom.requestBodyJson = v; save(); }, 3))),
            );
        }
        const temp = el('input', {type: 'range', min: '0', max: '2', step: '0.05', style: 'width:220px'});
        temp.value = String(ai.temperature);
        ctl('temperature', 'Temperatura', temp);
        const tempLabel = el('span', {class: 'muted', style: 'width:40px;text-align:right'}, Number(ai.temperature).toFixed(2));
        temp.addEventListener('input', () => { ai.temperature = Number(temp.value); tempLabel.textContent = ai.temperature.toFixed(2); save(); });
        wrap.append(
            cardRow({icon: 'settings', title: 'Temperatura', control: el('div', {style: 'display:flex;gap:10px;align-items:center'}, temp, tempLabel)}),
            cardRow({icon: 'play', title: 'Mostrar a resposta enquanto é gerada (streaming)', control: ctl('streamResponse', 'Streaming', toggle(ai.streamResponse, (v) => { ai.streamResponse = v; save(); }))}),
            cardRow({icon: 'close', title: 'Cancelar pedidos pendentes ao consultar outra palavra', control: ctl('cancelPendingRequests', 'Cancelar pedidos pendentes', toggle(ai.cancelPendingRequests, (v) => { ai.cancelPendingRequests = v; save(); }))}),
        );
    };
    draw();
    return wrap;
}

function promptCard(title, desc, get, set, fallback, setting = null) {
    const area = textArea(get(), (v) => { set(v); save(); }, 5);
    if (setting) { aiControl(setting, title, area); }
    const reset = el('button', {class: 'ghost', html: `${icon('refresh', 'sm')}Restaurar padrão`});
    reset.addEventListener('click', () => { set(fallback); area.value = fallback; save(); });
    return el('div', {class: 'card-row stack'},
        el('div', {class: 'head'}, el('span', {class: 'ico', html: icon('sparkle')}),
            el('div', {class: 'txt'}, el('div', {class: 't'}, title), el('div', {class: 'd', html: desc})), reset),
        area);
}

/** Tries a dictionary's own provider with one of its prompts. */
function testButton(dictId = 'ai-word', promptKey = 'customPrompt') {
    const out = el('span', {class: 'muted'});
    const btn = el('button', {class: 'primary', html: `${icon('sparkle', 'sm')}Testar a IA`});
    btn.addEventListener('click', async () => {
        out.className = 'muted';
        out.textContent = 'Testando…';
        try {
            const text = await explain(aiOptionsFor(store.ai, dictId), 'serendipity', 'Finding that old book in the attic was pure serendipity.', {
                signal: new AbortController().signal,
                prompt: fillLanguages(store.ai[promptKey], store.settings),
                onChunk: (_d, full) => { out.textContent = full.slice(0, 200); },
            });
            out.className = 'ok';
            out.textContent = `Funcionou: ${text.slice(0, 200)}${text.length > 200 ? '…' : ''}`;
        } catch (e) {
            out.className = 'error';
            out.textContent = String(e?.message || e);
        }
    });
    return el('div', {class: 'actions'}, btn, out);
}

function renderAiWord(root) {
    const ai = store.ai;
    const s = store.settings;
    const vars = escapeHtml('{{target}} (palavra), {{sentence}} (o parágrafo em volta da palavra), {{definitionLanguage}}, {{translationLanguage}}');
    root.append(
        el('h2', {}, AI_NAMES['ai-word']),
        el('p', {class: 'lead'}, 'Explica a palavra no sentido exato em que aparece na tela, com forma de dicionário, IPA, nível CEFR, classe gramatical, definição e tradução. Usa o seu provedor e o seu modelo (BYOK); nada passa por servidores do Lookitan.'),
        el('h3', {}, 'Configurações do dicionário'),
        el('div', {class: 'group'},
            ...languageRows(),
            cardRow({icon: 'eye', title: 'Tradução da palavra', desc: 'Desligada, a IA nem gera a tradução, o que gasta menos tokens.',
                control: toggle(s.showWordTranslation !== false, (v) => { s.showWordTranslation = v; save(); renderDetail(); })}),
            s.showWordTranslation !== false ? cardRow({icon: 'eyeOff', title: 'Ocultar a tradução da palavra', desc: 'A tradução só aparece quando você clica nela.',
                control: toggle(s.hideWordTranslation, (v) => { s.hideWordTranslation = v; save(); })}) : null,
            cardRow({icon: 'sparkle', title: 'Gerar o card automaticamente ao abrir o popup', desc: 'Desligado: o card só é gerado quando você clica em Gerar.',
                control: aiControl('autoGenerateOnLookup', 'Gerar o card automaticamente ao abrir o popup', toggle(ai.autoGenerateOnLookup, (v) => { ai.autoGenerateOnLookup = v; save(); }))}),
            cardRow({icon: 'sparkle', title: 'Explicar automaticamente ao consultar', desc: 'Desligado: gera a explicação somente ao clicar em Explicar.',
                control: aiControl('autoExplain', 'Explicar automaticamente ao consultar', toggle(ai.autoExplain, (v) => { ai.autoExplain = v; save(); }))}),
            cardRow({icon: 'book', title: 'Usar a IA quando a palavra não estiver nos dicionários', desc: 'Gera o card mesmo com a opção acima desligada, quando nenhum dicionário tem a palavra.',
                control: aiControl('unknownWordFallback', 'IA quando a palavra não estiver nos dicionários', toggle(ai.unknownWordFallback, (v) => { ai.unknownWordFallback = v; save(); }))})),
        el('h3', {}, 'Provedor de IA (sua chave, seu modelo)'),
        providerSection(),
        el('h3', {}, 'Prompts'),
        el('div', {class: 'group'},
            promptCard('Prompt do card', `Pede o JSON com os campos do card. Variáveis: ${vars}. ${escapeHtml('{{translationField}}')} recebe a tradução da palavra (o campo abaixo) ou fica vazio quando ela está desligada.`, () => ai.cardPrompt, (v) => { ai.cardPrompt = v; }, CARD_PROMPT, 'cardPrompt'),
            promptCard('Prompt da tradução da palavra', `O que a IA põe no campo "translation" do card. Só é enviado com a tradução da palavra ligada. Variáveis: ${vars}.`, () => ai.wordTranslationPrompt, (v) => { ai.wordTranslationPrompt = v; }, WORD_TRANSLATION_PROMPT, 'wordTranslationPrompt'),
            promptCard('Prompt da explicação (botão Explicar)', `Prompt personalizado da sua fork. Variáveis: ${vars}.`, () => ai.customPrompt, (v) => { ai.customPrompt = v; }, DEFAULT_PROMPT, 'customPrompt'),
            el('div', {class: 'card-row stack'},
                el('div', {class: 'head'}, el('span', {class: 'ico', html: icon('settings')}), el('div', {class: 'txt'}, el('div', {class: 't'}, 'System prompt'), el('div', {class: 'd'}, 'Enviado no card e na explicação. Opcional.'))),
                aiControl('systemPrompt', 'System prompt', textArea(ai.systemPrompt, (v) => { ai.systemPrompt = v; save(); }, 3)))),
        testButton(),
    );
}

function renderAiContext(root) {
    const ai = store.ai;
    const own = ai.sectionProviders['ai-context'];
    const vars = escapeHtml('{{target}}, {{sentence}} (só a frase da palavra), {{translationLanguage}}');
    root.append(
        el('h2', {}, AI_NAMES['ai-context']),
        el('p', {class: 'lead'}, 'Traduz a frase em volta da palavra e destaca a tradução da palavra.'),
        el('h3', {}, 'Configurações do dicionário'),
        el('div', {class: 'group'},
            languageRows()[1],
            cardRow({icon: 'sparkle', title: 'Traduzir automaticamente ao abrir o popup', desc: 'Desligado: a frase só é traduzida quando você clica em Traduzir.',
                control: aiControl('autoTranslateContext', 'Traduzir automaticamente ao abrir o popup', toggle(ai.autoTranslateContext, (v) => { ai.autoTranslateContext = v; save(); }))}),
            cardRow({icon: 'eyeOff', title: 'Ocultar a tradução', desc: 'A tradução da frase só aparece quando você clica nela.',
                control: aiControl('hideContextTranslation', 'Ocultar a tradução', toggle(ai.hideContextTranslation === true, (v) => { ai.hideContextTranslation = v; save(); }))}),
            promptCard('Prompt da tradução', `A tradução da palavra deve vir entre **asteriscos duplos** para ser destacada. Variáveis: ${vars}.`, () => ai.contextPrompt, (v) => { ai.contextPrompt = v; }, CONTEXT_PROMPT, 'contextPrompt'),
            el('div', {class: 'card-row stack'},
                el('div', {class: 'head'}, el('span', {class: 'ico', html: icon('settings')}), el('div', {class: 'txt'}, el('div', {class: 't'}, 'System prompt'), el('div', {class: 'd'}, 'Enviado na tradução da frase. Opcional.'))),
                aiControl('sectionProviders.ai-context.systemPrompt', 'System prompt', textArea(own.systemPrompt, (v) => { own.systemPrompt = v; save(); }, 3)))),
        el('h3', {}, 'Provedor de IA (sua chave, seu modelo)'),
        el('p', {class: 'muted'}, 'Só para o tradutor de contexto. Começa como uma cópia do provedor de "IA — palavra no contexto".'),
        providerSection(own, 'sectionProviders.ai-context.'),
        testButton('ai-context', 'contextPrompt'),
    );
}

/** Runs the image query and the Bing search for a sample word, as the pop-up does. */
function imageTestButton() {
    const out = el('span', {class: 'muted'});
    const btn = el('button', {class: 'primary', html: `${icon('image', 'sm')}Testar a busca`});
    btn.addEventListener('click', async () => {
        out.className = 'muted';
        out.textContent = 'Testando…';
        const word = 'serendipity';
        try {
            let query = word;
            if (store.ai.imageQueryUseAi !== false) {
                const {system, user} = imagePrompts(store.ai, store.settings);
                query = cleanQuery(await explain(aiOptionsFor(store.ai, 'ai-images'), word, 'Finding that old book in the attic was pure serendipity.', {
                    signal: new AbortController().signal, prompt: user, system,
                }), word);
            }
            const items = await searchImages(query);
            out.className = items.length ? 'ok' : 'error';
            out.textContent = `Busca: "${query}" · ${items.length} imagens`;
        } catch (e) {
            out.className = 'error';
            out.textContent = String(e?.message || e);
        }
    });
    return el('div', {class: 'actions'}, btn, out);
}

function renderAiImages(root) {
    const ai = store.ai;
    const vars = escapeHtml('{{target}} (palavra), {{sentence}} (a frase da palavra), {{wordLanguage}} (idioma do OCR), {{definitionLanguage}}, {{translationLanguage}}');
    root.append(
        el('h2', {}, AI_NAMES['ai-images']),
        el('p', {class: 'lead'}, 'Mostra imagens da palavra no popup, como o Migaku: a IA transforma a palavra num termo de busca em inglês, e o Lookitan procura esse termo no Bing Imagens. Sem IA, busca a palavra como está na tela. Passe o mouse numa imagem e clique na marca do canto para mandá-la ao Anki (marcador {image}); clique na imagem para ampliar.'),
        el('h3', {}, 'Configurações do dicionário'),
        el('div', {class: 'group'},
            cardRow({icon: 'image', title: 'Mostrar imagens automaticamente', desc: 'Desligado: as imagens só aparecem quando você clica em "Mostrar imagens" no popup, como o botão Explicar.',
                control: aiControl('autoShowImages', 'Mostrar imagens automaticamente', toggle(ai.autoShowImages === true, (v) => { ai.autoShowImages = v; save(); }))}),
            cardRow({icon: 'sparkle', title: 'Criar o termo de busca com a IA', desc: 'Desligado: busca a palavra como ela aparece na tela, sem gastar tokens.',
                control: aiControl('imageQueryUseAi', 'Criar o termo de busca com a IA', toggle(ai.imageQueryUseAi !== false, (v) => { ai.imageQueryUseAi = v; save(); }))})),
        el('h3', {}, 'Image query (termo de busca)'),
        el('div', {class: 'group'},
            promptCard('System prompt', `As regras do termo de busca. Substitui o system prompt geral só neste pedido. Variáveis: ${vars}.`,
                () => ai.imageQuerySystemPrompt, (v) => { ai.imageQuerySystemPrompt = v; }, IMAGE_QUERY_SYSTEM_PROMPT, 'imageQuerySystemPrompt'),
            promptCard('User prompt', `A mensagem com a palavra. A primeira linha da resposta vira a busca. Variáveis: ${vars}.`,
                () => ai.imageQueryPrompt, (v) => { ai.imageQueryPrompt = v; }, IMAGE_QUERY_PROMPT, 'imageQueryPrompt')),
        el('h3', {}, 'Provedor de IA (sua chave, seu modelo)'),
        el('p', {class: 'muted'}, 'Só para o termo de busca das imagens. Começa como uma cópia do provedor de "IA — palavra no contexto".'),
        providerSection(ai.sectionProviders['ai-images'], 'sectionProviders.ai-images.'),
        imageTestButton(),
    );
}

/**
 * The provider of "Imagens IA": only those that make pictures, with image models. It lives in
 * `ai.sectionProviders['ai-image-gen']` like the other dictionaries' own blocks.
 */
function imageProviderSection(own, prefix) {
    const ctl = (path, label, control) => aiControl(`${prefix}${path}`, label, control);
    const wrap = el('div', {class: 'group'});
    const draw = () => {
        wrap.replaceChildren();
        const cfg = own[own.provider];
        const models = IMAGE_GEN_MODELS[own.provider] || [];
        const modelInput = ctl(`${own.provider}.model`, 'Modelo de imagem', textInput(cfg.model, (v) => { cfg.model = v.trim(); save(); }, {placeholder: models[0] || 'nome-do-modelo'}));
        const listId = `image-gen-models-${own.provider}`;
        modelInput.setAttribute('list', listId);
        const modelHelp = {
            openai: 'gpt-image-1-mini é o mais barato. DALL·E 3 faz uma imagem por pedido.',
            gemini: 'gemini-2.5-flash-image (Nano Banana) ou Imagen 4. Termine com -vertex para usar o Vertex AI Express.',
            custom: 'OpenRouter: um modelo que gera imagens, como google/gemini-2.5-flash-image.',
        }[own.provider];
        wrap.append(...[
            cardRow({icon: 'cloud', title: 'Provedor', wide: true,
                control: ctl('provider', 'Provedor de imagens', select([
                    {value: 'openai', label: 'OpenAI'},
                    {value: 'gemini', label: 'Google Gemini'},
                    {value: 'custom', label: 'Personalizado (OpenRouter, xAI, servidor compatível…)'},
                ], own.provider, (v) => { own.provider = v; save(); draw(); }))}),
            own.provider === 'custom' ? cardRow({icon: 'cloud', title: 'Endpoint', wide: true,
                desc: 'Chat (como o do OpenRouter): a resposta traz a imagem. Terminado em /images/generations: a API de imagens da OpenAI, como a da xAI.',
                control: ctl('custom.endpoint', 'Endpoint', textInput(own.custom.endpoint, (v) => { own.custom.endpoint = v.trim(); save(); }, {placeholder: 'https://openrouter.ai/api/v1/chat/completions'}))}) : null,
            cardRow({icon: 'settings', title: 'Chave da API', wide: true,
                control: ctl(`${own.provider}.apiKey`, 'Chave da API', textInput(cfg.apiKey, (v) => { cfg.apiKey = v.trim(); save(); }, {type: 'password'}))}),
            cardRow({icon: 'sparkle', title: 'Modelo de imagem', desc: modelHelp, wide: true,
                control: el('div', {style: 'width:100%'}, modelInput, el('datalist', {id: listId}, ...models.map((m) => el('option', {value: m}))))}),
            own.provider === 'custom' ? el('div', {class: 'card-row stack'},
                el('div', {class: 'head'}, el('span', {class: 'ico', html: icon('settings')}), el('div', {class: 'txt'}, el('div', {class: 't'}, 'JSON extra no corpo da requisição'), el('div', {class: 'd'}, 'Mesclado ao corpo de cada pedido. Ex.: {"size": "1024x1024"}'))),
                ctl('custom.requestBodyJson', 'JSON extra no corpo da requisição', textArea(own.custom.requestBodyJson, (v) => { own.custom.requestBodyJson = v; save(); }, 3))) : null,
        ].filter(Boolean));
    };
    draw();
    return wrap;
}

/** Makes one picture of a sample word with this dictionary's own provider and the chosen prompt. */
function imageGenTestButton() {
    const out = el('span', {class: 'muted'});
    const shot = el('div', {class: 'image-gen-test'});
    const btn = el('button', {class: 'primary', html: `${icon('image', 'sm')}Testar (gera 1 imagem)`});
    btn.addEventListener('click', async () => {
        out.className = 'muted';
        out.textContent = 'Gerando…';
        shot.replaceChildren();
        btn.disabled = true;
        try {
            const options = aiOptionsFor(store.ai, 'ai-image-gen');
            const prompt = imageGenPrompt(chosenImageGenPrompt(store.ai).prompt, {word: 'serendipity', sentence: 'Finding that old book in the attic was pure serendipity.'}, store.settings);
            const [picture] = await generateImages(options, prompt, {count: 1, aspect: store.ai.imageGenAspect, quality: store.ai.imageGenQuality});
            shot.append(el('img', {src: 'url' in picture ? picture.url : URL.createObjectURL(picture.blob), alt: 'serendipity'}));
            out.className = 'ok';
            out.textContent = 'Funcionou.';
        } catch (e) {
            out.className = 'error';
            out.textContent = String(e?.message || e);
        } finally {
            btn.disabled = false;
        }
    });
    return el('div', {}, el('div', {class: 'actions'}, btn, out), shot);
}

function renderAiImageGen(root) {
    const ai = store.ai;
    const own = ai.sectionProviders['ai-image-gen'];
    const vars = escapeHtml('{{target}} (palavra), {{sentence}} (a frase da palavra), {{wordLanguage}} (idioma do OCR), {{definitionLanguage}}, {{translationLanguage}}');
    const number = (n) => ({value: String(n), label: n === 1 ? '1 imagem' : `${n} imagens`});
    root.append(
        el('h2', {}, AI_NAMES['ai-image-gen']),
        el('p', {class: 'lead'}, 'Gera imagens da palavra no popup com um modelo de imagem e o seu prompt, por exemplo para entender o sentido de um verbo. Usa a sua chave (BYOK). Como na seção Imagens, clique numa imagem para ampliar e na marca do canto para mandá-la ao Anki: vai para o marcador {ai-image} ou, se o tipo de nota não o usa, para o {image}.'),
        el('h3', {}, 'Configurações do dicionário'),
        el('div', {class: 'group'},
            cardRow({icon: 'sparkle', title: 'Gerar automaticamente ao abrir o popup', desc: 'Desligado: as imagens só são geradas quando você clica em "Gerar imagem", como o botão Explicar. Cada imagem gerada é cobrada pelo provedor.',
                control: aiControl('autoGenerateImages', 'Gerar automaticamente ao abrir o popup', toggle(ai.autoGenerateImages === true, (v) => { ai.autoGenerateImages = v; save(); }))}),
            cardRow({icon: 'image', title: 'Imagens por vez', desc: 'Quantas imagens cada clique gera. "Gerar mais", embaixo das imagens, gera outra leva.', wide: true,
                control: aiControl('imageGenCount', 'Imagens por vez', select(Array.from({length: IMAGE_GEN_MAX_COUNT}, (_v, i) => number(i + 1)),
                    String(ai.imageGenCount || 1), (v) => { ai.imageGenCount = Number(v); save(); }))}),
            cardRow({icon: 'image', title: 'Formato', wide: true,
                control: aiControl('imageGenAspect', 'Formato', select([
                    {value: '1:1', label: 'Quadrada (1:1)'}, {value: '16:9', label: 'Paisagem (16:9)'}, {value: '9:16', label: 'Retrato (9:16)'},
                    {value: '4:3', label: 'Paisagem (4:3)'}, {value: '3:4', label: 'Retrato (3:4)'},
                ], ai.imageGenAspect || '1:1', (v) => { ai.imageGenAspect = v; save(); }))}),
            cardRow({icon: 'settings', title: 'Qualidade (OpenAI)', desc: 'Baixa é a mais barata e já basta para um card. No DALL·E 3, Alta é "hd".', wide: true,
                control: aiControl('imageGenQuality', 'Qualidade (OpenAI)', select([
                    {value: 'low', label: 'Baixa'}, {value: 'medium', label: 'Média'}, {value: 'high', label: 'Alta'}, {value: 'auto', label: 'Automática'},
                ], ai.imageGenQuality || 'low', (v) => { ai.imageGenQuality = v; save(); }))})),
        el('h3', {}, 'Prompts'),
        el('p', {class: 'muted', html: `Cada prompt é o que o modelo de imagem recebe. As imagens de todos os prompts se juntam na mesma faixa do popup. Variáveis: ${vars}.`}),
        imagePromptsEditor(ai),
        el('h3', {}, 'Provedor de imagens (sua chave, seu modelo)'),
        el('p', {class: 'muted'}, 'Só para as Imagens IA. Começa com a chave de "IA — palavra no contexto" e um modelo de imagem.'),
        imageProviderSection(own, 'sectionProviders.ai-image-gen.'),
        imageGenTestButton(),
    );
}

/**
 * The named prompts of "Imagens IA": the pop-up has a selector among them and remembers the last
 * one chosen (`imageGenPromptId`), which is also the choice here.
 */
function imagePromptsEditor(ai) {
    const wrap = el('div', {class: 'group image-prompts'});
    const draw = (focusIndex = null) => {
        wrap.replaceChildren();
        const list = ai.imageGenPrompts;
        const chooser = aiControl('imageGenPromptId', 'Prompt escolhido', select(list.map((p) => ({value: p.id, label: p.name})),
            ai.imageGenPromptId, (v) => { ai.imageGenPromptId = v; save(); }));
        wrap.append(cardRow({icon: 'sparkle', title: 'Prompt escolhido', desc: 'O que o popup usa ao abrir. Lá, um seletor ao lado de "Gerar imagem" troca de prompt, e o último escolhido fica valendo.', wide: true, control: chooser}));
        list.forEach((p, i) => {
            const name = textInput(p.name, (v) => {
                p.name = v;
                save();
                // The chooser shows the new name at once.
                const option = [...chooser.children].find((o) => o.value === p.id);
                if (option) { option.textContent = v || `Prompt ${i + 1}`; }
            }, {placeholder: 'Nome do prompt'});
            aiControl(`imageGenPrompts.${i}.name`, `Nome do prompt ${i + 1}`, name);
            const area = aiControl(`imageGenPrompts.${i}.prompt`, `Prompt ${i + 1}`, textArea(p.prompt, (v) => { p.prompt = v; save(); }, 4));
            const move = (to) => {
                const [item] = list.splice(i, 1);
                list.splice(to, 0, item);
                save();
                draw(to);
            };
            const button = (iconName, label, onClick, disabled = false) => {
                const b = el('button', {class: 'ghost icon', type: 'button', title: label, 'aria-label': label, html: icon(iconName, 'sm')});
                b.disabled = disabled;
                b.addEventListener('click', onClick);
                return b;
            };
            const remove = button('trash', 'Apagar este prompt', () => {
                list.splice(i, 1);
                if (!list.some((x) => x.id === ai.imageGenPromptId)) { ai.imageGenPromptId = list[0].id; }
                save();
                draw();
            }, list.length === 1);
            const row = el('div', {class: 'card-row stack image-prompt'},
                el('div', {class: 'head'},
                    el('span', {class: 'num'}, String(i + 1)),
                    el('div', {class: 'txt'}, name),
                    el('span', {class: 'audio-move'}, button('chevronUp', 'Subir', () => move(i - 1), i === 0), button('chevronDown', 'Descer', () => move(i + 1), i === list.length - 1)),
                    remove),
                area);
            wrap.append(row);
            if (focusIndex === i) { name.focus?.(); }
        });
        const add = el('button', {class: 'ghost', type: 'button', html: `${icon('plus', 'sm')}Adicionar prompt`});
        add.addEventListener('click', () => {
            list.push({id: newImageGenPromptId(), name: `Prompt ${list.length + 1}`, prompt: IMAGE_GEN_PROMPT});
            save();
            draw(list.length - 1);
        });
        const missing = IMAGE_GEN_PRESETS.filter((preset) => !list.some((p) => p.id === preset.id));
        const restore = el('button', {class: 'ghost', type: 'button', html: `${icon('refresh', 'sm')}Trazer de volta os exemplos`});
        restore.addEventListener('click', () => {
            list.push(...missing.map((preset) => ({...preset})));
            save();
            draw();
        });
        wrap.append(el('div', {class: 'actions'}, add, missing.length ? restore : null));
    };
    draw();
    return wrap;
}
