// Dictionaries tab: the pop-up order on the left (drag to reorder, switch to enable) and the selected
// dictionary's details on the right. The two AI dictionaries carry the BYOK provider settings.

import {open} from '@tauri-apps/plugin-dialog';
import {api, escapeHtml} from '../lib/api.js';
import {DEFAULT_PROMPT} from '../lib/ai/defaults.js';
import {DEFAULT_MODELS, explain} from '../lib/ai/explain.js';
import {CARD_PROMPT, CONTEXT_PROMPT, fillLanguages} from '../lib/ai/word-card.js';
import {icon} from '../lib/icons.js';
import {store, save} from './store.js';
import {cardRow, el, languageOptions, select, textArea, textInput, toggle} from './ui.js';

let dicts = [];
let selectedId = null;
let dragId = null;

const AI_NAMES = {'ai-word': 'IA — palavra no contexto', 'ai-context': 'Tradutor de contexto'};

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
    const path = await open({multiple: false, directory: false, filters: [{name: 'MDict', extensions: ['mdx']}]});
    if (!path) { return; }
    const btn = document.getElementById('add-dict');
    btn.disabled = true;
    try {
        dicts = await api.addDictionary(path);
        selectedId = dicts[dicts.length - 1]?.id || selectedId;
    } catch (e) {
        alertDetail(String(e));
    } finally {
        btn.disabled = false;
    }
    renderList();
}

function alertDetail(text) {
    document.getElementById('dict-detail').prepend(el('p', {class: 'error'}, text));
}

function renderList() {
    const list = document.getElementById('dict-list');
    list.replaceChildren();
    if (!selectedId || !dicts.some((d) => d.id === selectedId)) { selectedId = dicts[0]?.id || null; }
    dicts.forEach((d, i) => {
        const sw = toggle(d.enabled, async (v) => { dicts = await api.updateDictionary(d.id, {enabled: v}); renderList(); }, {on: '', off: ''});
        sw.addEventListener('click', (ev) => ev.stopPropagation());
        const li = el('li', {class: d.id === selectedId ? 'sel' : '', draggable: 'true'},
            el('span', {class: 'num'}, String(i + 1)),
            d.kind !== 'mdict' ? el('span', {class: 'faint', html: icon('cloud', 'sm')}) : null,
            el('span', {class: 'name'}, displayName(d)),
            d.error ? el('span', {class: 'err', title: d.error, html: icon('alert', 'sm')}) : null,
            sw);
        li.addEventListener('click', () => { selectedId = d.id; renderList(); });
        li.addEventListener('dragstart', (ev) => { dragId = d.id; ev.dataTransfer.effectAllowed = 'move'; });
        li.addEventListener('dragover', (ev) => { ev.preventDefault(); li.classList.add('drag-over'); });
        li.addEventListener('dragleave', () => li.classList.remove('drag-over'));
        li.addEventListener('drop', async (ev) => {
            ev.preventDefault();
            li.classList.remove('drag-over');
            if (dragId && dragId !== d.id) {
                dicts = await api.updateDictionary(dragId, {moveTo: i});
                renderList();
            }
        });
        list.append(li);
    });
    renderDetail();
}

function renderDetail() {
    const root = document.getElementById('dict-detail');
    root.replaceChildren();
    const d = dicts.find((x) => x.id === selectedId);
    if (!d) {
        root.append(el('div', {class: 'empty', html: `${icon('book', 'lg')}<strong>Nenhum dicionário</strong>Adicione um dicionário MDict (.mdx).`}));
        return;
    }
    if (d.id === 'ai-word') { renderAiWord(root); return; }
    if (d.id === 'ai-context') { renderAiContext(root); return; }
    renderMdict(root, d);
}

function renderMdict(root, d) {
    const remove = el('button', {html: `${icon('trash', 'sm')}Remover dicionário`});
    remove.addEventListener('click', async () => {
        dicts = await api.removeDictionary(d.id);
        selectedId = null;
        renderList();
    });
    root.append(
        el('h2', {}, d.name || d.path),
        el('p', {class: 'lead'}, d.error ? `Erro ao abrir: ${d.error}` : 'Dicionário MDict instalado. Os arquivos .mdd e .css com o mesmo nome, na mesma pasta, são usados para estilos, imagens e áudio.'),
        el('h3', {}, 'Informações'),
        el('div', {class: 'group'},
            cardRow({icon: 'book', title: 'Formato', control: el('span', {class: 'muted'}, 'MDict (.mdx)')}),
            cardRow({icon: 'list', title: 'Verbetes', control: el('span', {class: 'muted'}, d.loaded ? d.entries.toLocaleString('pt-BR') : (d.enabled ? 'Carregando…' : 'Desativado'))}),
            cardRow({icon: 'folder', title: 'Arquivos de recursos (.mdd)', control: el('span', {class: 'muted'}, String(d.resourceFiles))}),
            cardRow({icon: 'folder', title: 'Arquivo', desc: d.path, control: remove})),
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

function providerSection() {
    const ai = store.ai;
    const cfg = () => ai[ai.provider];
    const wrap = el('div', {class: 'group'});
    const draw = () => {
        wrap.replaceChildren();
        const models = (DEFAULT_MODELS[ai.provider] || []);
        const modelInput = aiControl(`${ai.provider}.model`, 'Modelo', textInput(cfg().model, (v) => { cfg().model = v.trim(); save(); }, {placeholder: models[0] || 'nome-do-modelo'}));
        const listId = `models-${ai.provider}`;
        modelInput.setAttribute('list', listId);
        const datalist = el('datalist', {id: listId}, ...models.map((m) => el('option', {value: m})));
        wrap.append(...[
            cardRow({icon: 'cloud', title: 'Provedor', wide: true,
                control: aiControl('provider', 'Provedor', select([
                    {value: 'openai', label: 'OpenAI'},
                    {value: 'gemini', label: 'Google Gemini'},
                    {value: 'deepseek', label: 'DeepSeek'},
                    {value: 'custom', label: 'Personalizado (OpenRouter, Ollama, LM Studio…)'},
                ], ai.provider, (v) => { ai.provider = v; save(); draw(); }))}),
            ai.provider === 'custom' ? cardRow({icon: 'cloud', title: 'Endpoint', desc: 'URL completa da API compatível com OpenAI. Usada exatamente como informada.', wide: true,
                control: aiControl('custom.endpoint', 'Endpoint', textInput(ai.custom.endpoint, (v) => { ai.custom.endpoint = v.trim(); save(); }, {placeholder: 'https://openrouter.ai/api/v1/chat/completions'}))}) : null,
            cardRow({icon: 'settings', title: 'Chave da API', wide: true,
                control: aiControl(`${ai.provider}.apiKey`, 'Chave da API', textInput(cfg().apiKey, (v) => { cfg().apiKey = v.trim(); save(); }, {type: 'password'}))}),
            cardRow({icon: 'sparkle', title: 'Modelo', wide: true, control: el('div', {style: 'width:100%'}, modelInput, datalist)}),
        ].filter(Boolean));
        if (ai.provider === 'gemini') {
            wrap.append(cardRow({icon: 'sparkle', title: 'Nível de raciocínio (Gemini 3)', wide: true,
                control: aiControl('gemini.thinkingLevel', 'Nível de raciocínio (Gemini 3)', select([{value: '', label: 'Padrão do modelo'}, ...['MINIMAL', 'LOW', 'MEDIUM', 'HIGH'].map((v) => ({value: v, label: v}))],
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
                    control: aiControl(`${ai.provider}.thinkingMode`, 'Raciocínio (thinking)', select([
                        {value: '', label: 'Padrão'}, {value: 'enabled', label: 'Ligado'}, {value: 'disabled', label: 'Desligado'},
                    ], cfg().thinkingMode, (v) => { cfg().thinkingMode = v; save(); }))}),
                cardRow({icon: 'sparkle', title: 'Esforço do raciocínio (thinking)', wide: true,
                    desc: 'O modelo precisa aceitar o nível escolhido. Máxima equivale a xhigh no OpenRouter.',
                    control: aiControl(`${ai.provider}.thinkingIntensity`, 'Esforço do raciocínio (thinking)', select(intensities, cfg().thinkingIntensity,
                        (v) => { cfg().thinkingIntensity = v; save(); draw(); }))}),
            );
            if (ai.provider === 'custom') {
                wrap.append(cardRow({icon: 'sparkle', title: 'Valor personalizado de thinking',
                    desc: 'Usado quando o esforço é Personalizada. OpenRouter: medium, 2000 ou {"max_tokens":2000}. Outras APIs: reasoning_effort.', wide: true,
                    control: aiControl('custom.customThinkingIntensity', 'Valor personalizado de thinking', textInput(ai.custom.customThinkingIntensity,
                        (v) => { ai.custom.customThinkingIntensity = v; save(); }, {placeholder: 'medium, 2000 ou {"max_tokens":2000}'}))}));
            }
        }
        if (ai.provider === 'custom') {
            wrap.append(
                cardRow({icon: 'list', title: 'Roteamento de provedores (OpenRouter)', wide: true,
                    control: aiControl('custom.providerRoutingMode', 'Roteamento de provedores (OpenRouter)', select([
                        {value: '', label: 'Sem roteamento'}, {value: 'order', label: 'Ordem'}, {value: 'only', label: 'Somente'}, {value: 'ignore', label: 'Ignorar'},
                    ], ai.custom.providerRoutingMode, (v) => { ai.custom.providerRoutingMode = v; save(); }))}),
                cardRow({icon: 'list', title: 'Provedores do OpenRouter', desc: 'Slugs separados por vírgula, na ordem desejada.', wide: true,
                    control: aiControl('custom.providerSlugs', 'Provedores do OpenRouter', textInput(ai.custom.providerSlugs,
                        (v) => { ai.custom.providerSlugs = v; save(); }, {placeholder: 'deepinfra/turbo, fireworks'}))}),
                cardRow({icon: 'list', title: 'Permitir outros provedores se os escolhidos falharem',
                    control: aiControl('custom.providerAllowFallbacks', 'Permitir outros provedores', toggle(ai.custom.providerAllowFallbacks !== false, (v) => { ai.custom.providerAllowFallbacks = v; save(); }))}),
                el('div', {class: 'card-row stack'},
                    el('div', {class: 'head'}, el('span', {class: 'ico', html: icon('settings')}), el('div', {class: 'txt'}, el('div', {class: 't'}, 'JSON extra no corpo da requisição'), el('div', {class: 'd'}, 'Mesclado ao corpo de cada pedido. Ex.: {"top_p": 0.9}'))),
                    aiControl('custom.requestBodyJson', 'JSON extra no corpo da requisição', textArea(ai.custom.requestBodyJson, (v) => { ai.custom.requestBodyJson = v; save(); }, 3))),
            );
        }
        const temp = el('input', {type: 'range', min: '0', max: '2', step: '0.05', style: 'width:220px'});
        temp.value = String(ai.temperature);
        aiControl('temperature', 'Temperatura', temp);
        const tempLabel = el('span', {class: 'muted', style: 'width:40px;text-align:right'}, Number(ai.temperature).toFixed(2));
        temp.addEventListener('input', () => { ai.temperature = Number(temp.value); tempLabel.textContent = ai.temperature.toFixed(2); save(); });
        wrap.append(
            cardRow({icon: 'settings', title: 'Temperatura', control: el('div', {style: 'display:flex;gap:10px;align-items:center'}, temp, tempLabel)}),
            cardRow({icon: 'sparkle', title: 'Gerar automaticamente ao abrir o popup', desc: 'Gera o card da palavra e a tradução de contexto.', control: aiControl('autoGenerateOnLookup', 'Gerar automaticamente ao abrir o popup', toggle(ai.autoGenerateOnLookup, (v) => { ai.autoGenerateOnLookup = v; save(); }))}),
            cardRow({icon: 'sparkle', title: 'Explicar automaticamente ao consultar', desc: 'Desligado: gera a explicação somente ao clicar em Explicar.',
                control: aiControl('autoExplain', 'Explicar automaticamente ao consultar', toggle(ai.autoExplain, (v) => { ai.autoExplain = v; save(); }))}),
            cardRow({icon: 'play', title: 'Mostrar a resposta enquanto é gerada (streaming)', control: aiControl('streamResponse', 'Streaming', toggle(ai.streamResponse, (v) => { ai.streamResponse = v; save(); }))}),
            cardRow({icon: 'close', title: 'Cancelar pedidos pendentes ao consultar outra palavra', control: aiControl('cancelPendingRequests', 'Cancelar pedidos pendentes', toggle(ai.cancelPendingRequests, (v) => { ai.cancelPendingRequests = v; save(); }))}),
            cardRow({icon: 'book', title: 'Usar a IA quando a palavra não estiver nos dicionários', control: aiControl('unknownWordFallback', 'IA quando a palavra não estiver nos dicionários', toggle(ai.unknownWordFallback, (v) => { ai.unknownWordFallback = v; save(); }))}),
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

function testButton() {
    const out = el('span', {class: 'muted'});
    const btn = el('button', {class: 'primary', html: `${icon('sparkle', 'sm')}Testar a IA`});
    btn.addEventListener('click', async () => {
        out.className = 'muted';
        out.textContent = 'Testando…';
        try {
            const text = await explain(store.ai, 'serendipity', 'Finding that old book in the attic was pure serendipity.', {
                signal: new AbortController().signal,
                prompt: fillLanguages(store.ai.customPrompt, store.settings),
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
    const vars = escapeHtml('{{target}} (palavra), {{sentence}} (frase), {{definitionLanguage}}, {{translationLanguage}}');
    root.append(
        el('h2', {}, AI_NAMES['ai-word']),
        el('p', {class: 'lead'}, 'Explica a palavra no sentido exato em que aparece na tela, com forma de dicionário, IPA, nível CEFR, classe gramatical, definição e tradução. Usa o seu provedor e o seu modelo (BYOK); nada passa por servidores do Lookitan.'),
        el('h3', {}, 'Configurações do dicionário'),
        el('div', {class: 'group'},
            ...languageRows(),
            cardRow({icon: 'eye', title: 'Tradução da palavra', desc: 'Desligada, a IA nem gera a tradução, o que gasta menos tokens. Com um prompt do card personalizado, só some da tela.',
                control: toggle(s.showWordTranslation !== false, (v) => { s.showWordTranslation = v; save(); renderDetail(); })}),
            s.showWordTranslation !== false ? cardRow({icon: 'eyeOff', title: 'Ocultar a tradução da palavra', desc: 'A tradução só aparece quando você clica nela.',
                control: toggle(s.hideWordTranslation, (v) => { s.hideWordTranslation = v; save(); })}) : null),
        el('h3', {}, 'Provedor de IA (sua chave, seu modelo)'),
        providerSection(),
        el('h3', {}, 'Prompts'),
        el('div', {class: 'group'},
            promptCard('Prompt do card', `Pede o JSON com os campos do card. Variáveis: ${vars}.`, () => ai.cardPrompt, (v) => { ai.cardPrompt = v; }, CARD_PROMPT, 'cardPrompt'),
            promptCard('Prompt da explicação (botão Explicar)', `Prompt personalizado da sua fork. Variáveis: ${vars}.`, () => ai.customPrompt, (v) => { ai.customPrompt = v; }, DEFAULT_PROMPT, 'customPrompt'),
            el('div', {class: 'card-row stack'},
                el('div', {class: 'head'}, el('span', {class: 'ico', html: icon('settings')}), el('div', {class: 'txt'}, el('div', {class: 't'}, 'System prompt'), el('div', {class: 'd'}, 'Enviado em todos os pedidos (card, explicação e tradução). Opcional.'))),
                aiControl('systemPrompt', 'System prompt', textArea(ai.systemPrompt, (v) => { ai.systemPrompt = v; save(); }, 3)))),
        testButton(),
    );
}

function renderAiContext(root) {
    const ai = store.ai;
    const vars = escapeHtml('{{target}}, {{sentence}}, {{translationLanguage}}');
    root.append(
        el('h2', {}, AI_NAMES['ai-context']),
        el('p', {class: 'lead'}, 'Traduz a frase em volta da palavra e destaca a tradução da palavra. Usa o mesmo provedor de IA configurado em "IA — palavra no contexto".'),
        el('h3', {}, 'Configurações do dicionário'),
        el('div', {class: 'group'},
            languageRows()[1],
            promptCard('Prompt da tradução', `A tradução da palavra deve vir entre **asteriscos duplos** para ser destacada. Variáveis: ${vars}.`, () => ai.contextPrompt, (v) => { ai.contextPrompt = v; }, CONTEXT_PROMPT, 'contextPrompt')),
        el('h3', {}, 'Provedor de IA (compartilhado com a palavra no contexto)'),
        providerSection(),
        testButton(),
    );
}
