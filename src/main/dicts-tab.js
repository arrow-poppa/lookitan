// Dictionaries tab: the pop-up order on the left (drag a row or press Alt+↑/↓ to reorder, switch to
// enable) and the selected dictionary's details on the right. The built-in AI dictionaries carry the BYOK provider settings.

import {ask, open} from '@tauri-apps/plugin-dialog';
import {api, escapeHtml} from '../lib/api.js';
import {DEFAULT_PROMPT, aiOptionsFor} from '../lib/ai/defaults.js';
import {DEFAULT_MODELS, explain} from '../lib/ai/explain.js';
import {CARD_PROMPT, COMPONENTS_PROMPT, CONTEXT_OCR_PROMPT, CONTEXT_PROMPT, EXPRESSIONS_PROMPT, FORM_PROMPT, OCR_ERRORS_PROMPT, READING_PROMPT, TRANSLITERATION_PROMPT, WORD_TRANSLATION_PROMPT, fillLanguages, takesCardNotes} from '../lib/ai/word-card.js';
import {IMAGE_QUERY_PROMPT, IMAGE_QUERY_SYSTEM_PROMPT, cleanQuery, imagePrompts, searchImages} from '../lib/images.js';
import {icon} from '../lib/icons.js';
import {store, save} from './store.js';
import {cardRow, dropIndex, el, languageOptions, select, sortableRow, textArea, textInput, toggle} from './ui.js';

let dicts = [];
let selectedId = null;
/** The row to focus after the list is drawn again (keyboard reordering). */
let focusId = null;
/** Set by a drag that just ended, so its click does not count as choosing the row. */
let dragEnded = false;

const AI_NAMES = {'ai-word': 'IA — palavra no contexto', 'ai-context': 'Tradutor de contexto', 'ai-images': 'Imagens'};

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
        cardRow({icon: 'translate', title: 'Idioma da tradução', desc: 'A tradução da palavra no card e o tradutor de contexto aparecem neste idioma.', wide: true,
            control: select(languageOptions(), s.translationLanguage, (v) => { s.translationLanguage = v; save(); })}),
        cardRow({icon: 'sparkle', title: 'Idioma da explicação', desc: 'A explicação da IA (botão Explicar) aparece neste idioma.', wide: true,
            control: select(languageOptions(), s.explanationLanguage || s.translationLanguage, (v) => { s.explanationLanguage = v; save(); })}),
    ];
}

/** For the card's notes: a card prompt that does not ask for the card's JSON is sent as written. */
function cardNotesWarning() {
    return takesCardNotes(store.ai.cardPrompt || '') ? '' : ' O seu prompt do card não pede o JSON do card (nem os marcadores {{…Field}} nem a chave "headword"), então esta nota não é enviada.';
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
    const vars = escapeHtml('{{target}} (palavra), {{sentence}} (o parágrafo em volta da palavra), {{definitionLanguage}}, {{translationLanguage}}, {{explanationLanguage}}');
    root.append(
        el('h2', {}, AI_NAMES['ai-word']),
        el('p', {class: 'lead'}, 'Explica a palavra no sentido exato em que aparece na tela, com forma de dicionário, IPA, nível CEFR, classe gramatical, os traços da forma na tela (pessoa, gênero, tempo verbal…), definição, tradução e sinônimos, a leitura em pinyin ou kana no chinês e no japonês, a transliteração nas escritas não latinas (árabe, russo, grego, coreano…) e, se quiser, os componentes da palavra. Usa o seu provedor e o seu modelo (BYOK); nada passa por servidores do Lookitan.'),
        el('h3', {}, 'Configurações do dicionário'),
        el('div', {class: 'group'},
            ...languageRows(),
            cardRow({icon: 'eye', title: 'Tradução da palavra', desc: 'Desligada, a IA nem gera a tradução, o que gasta menos tokens.',
                control: toggle(s.showWordTranslation !== false, (v) => { s.showWordTranslation = v; save(); renderDetail(); })}),
            s.showWordTranslation !== false ? cardRow({icon: 'eyeOff', title: 'Ocultar a tradução da palavra', desc: 'A tradução só aparece quando você clica nela.',
                control: toggle(s.hideWordTranslation, (v) => { s.hideWordTranslation = v; save(); })}) : null,
            cardRow({icon: 'book', title: 'Componentes', desc: 'As partes da palavra, em qualquer idioma: cada caractere no chinês e no japonês (com a leitura), raízes, afixos ou as palavras de um composto nos outros. Também vão para o Anki em {ai-card} e {ai-components}. Desligado, a IA nem gera os componentes, o que gasta menos tokens.',
                control: aiControl('cardComponents', 'Componentes', toggle(ai.cardComponents !== false, (v) => { ai.cardComponents = v; save(); }))}),
            cardRow({icon: 'book', title: 'Reconhecer expressões', desc: 'Quando a palavra faz parte de uma unidade maior no contexto (um nome como "Socrates Awards", um verbo como "turned … off", uma expressão idiomática, um composto), o card explica a unidade inteira e marca as partes dela no contexto. Desligado, o card explica só a palavra, e o pedido fica menor.' + cardNotesWarning(),
                control: aiControl('cardExpressions', 'Reconhecer expressões', toggle(ai.cardExpressions !== false, (v) => { ai.cardExpressions = v; save(); renderDetail(); }))}),
            cardRow({icon: 'sparkle', title: 'Corrigir erros de OCR com IA', desc: 'A IA sabe que a palavra veio do OCR: se ela não fizer sentido no contexto (um caractere trocado, uma letra perdida), a IA explica a palavra que o texto tem e mostra o que corrigiu ("OCR: 進 → 盡"). Desligado, a IA explica exatamente o que o OCR leu, e o pedido fica menor.' + cardNotesWarning(),
                control: aiControl('cardOcrFix', 'Corrigir erros de OCR com IA', toggle(ai.cardOcrFix !== false, (v) => { ai.cardOcrFix = v; save(); renderDetail(); }))}),
            cardRow({icon: 'sparkle', title: 'Gerar o card automaticamente ao abrir o popup', desc: 'Desligado: o card só é gerado quando você clica em Gerar.',
                control: aiControl('autoGenerateOnLookup', 'Gerar o card automaticamente ao abrir o popup', toggle(ai.autoGenerateOnLookup, (v) => { ai.autoGenerateOnLookup = v; save(); }))}),
            cardRow({icon: 'sparkle', title: 'Explicar automaticamente ao consultar', desc: 'Desligado: gera a explicação somente ao clicar em Explicar.',
                control: aiControl('autoExplain', 'Explicar automaticamente ao consultar', toggle(ai.autoExplain, (v) => { ai.autoExplain = v; save(); }))})),
        el('h3', {}, 'Provedor de IA (sua chave, seu modelo)'),
        providerSection(),
        el('h3', {}, 'Prompts'),
        el('div', {class: 'group'},
            promptCard('Prompt do card', `Pede o JSON com os campos do card. Variáveis: ${vars}. ${escapeHtml('{{translationField}}')} recebe a tradução da palavra (o campo abaixo) ou fica vazio quando ela está desligada; ${escapeHtml('{{readingField}}')} pede a leitura só no chinês (pinyin) e no japonês (kana); ${escapeHtml('{{componentsField}}')} pede os componentes só com a opção Componentes ligada; ${escapeHtml('{{formField}}')} pede os traços da forma e, nas escritas não latinas, a transliteração.`, () => ai.cardPrompt, (v) => { ai.cardPrompt = v; }, CARD_PROMPT, 'cardPrompt'),
            promptCard('Prompt da tradução da palavra', `O que a IA põe no campo "translation" do card: a melhor tradução e, separadas por vírgula, até duas outras com o mesmo sentido no contexto ("terminou, concluiu"), só quando houver. Só é enviado com a tradução da palavra ligada. Variáveis: ${vars}.`, () => ai.wordTranslationPrompt, (v) => { ai.wordTranslationPrompt = v; }, WORD_TRANSLATION_PROMPT, 'wordTranslationPrompt'),
            promptCard('Prompt da leitura', `O que a IA põe no campo "reading" do card. Só é enviado no chinês e no japonês. ${escapeHtml('{{readingSystem}}')} vira "pinyin with tone marks" no chinês e "hiragana" no japonês.`, () => ai.readingPrompt, (v) => { ai.readingPrompt = v; }, READING_PROMPT, 'readingPrompt'),
            promptCard('Prompt da forma', `O que a IA põe no campo "form" do card: os traços gramaticais da palavra como está na tela (pessoa, número, gênero, tempo verbal…), que aparecem como etiquetas antes da classe gramatical e vão para o Anki em {ai-form} e {tags}. Variáveis: ${vars}.`, () => ai.formPrompt, (v) => { ai.formPrompt = v; }, FORM_PROMPT, 'formPrompt'),
            promptCard('Prompt da transliteração', 'O que a IA põe no campo "transliteration" do card: a palavra como está na tela, em letras latinas (como o "fa-bāḍat" do Lookupper). Só é enviado nos idiomas de escrita não latina, menos o chinês e o japonês, que têm a leitura. Vai para o Anki em {ai-transliteration}.', () => ai.transliterationPrompt, (v) => { ai.transliterationPrompt = v; }, TRANSLITERATION_PROMPT, 'transliterationPrompt'),
            ai.cardExpressions === false ? null : promptCard('Prompt das expressões', 'Vai no fim do prompt do card, antes da nota de erros do OCR: diz à IA quando explicar a unidade maior de que a palavra faz parte e como devolver as partes dela ("selection"), que o Lookitan procura no contexto para marcá-las. Os cinco exemplos ficam no texto fixo, que o provedor guarda em cache. Sem variáveis.', () => ai.expressionsPrompt, (v) => { ai.expressionsPrompt = v; }, EXPRESSIONS_PROMPT, 'expressionsPrompt'),
            ai.cardOcrFix === false ? null : promptCard('Prompt de erros do OCR', `Vai no fim do prompt do card, em todos os idiomas: a palavra e o contexto vêm do OCR, que troca ou perde caracteres, e a IA usa o contexto para achar a palavra que o texto tem e explica essa. Quando corrige, ela diz o que corrigiu ("ocrFix"), que aparece como uma etiqueta "OCR: 進 → 盡" no card. ${escapeHtml('{{ocrErrors}}')} vira os erros típicos da escrita do idioma do OCR: letras e prefixos perdidos no árabe, caracteres parecidos e palavra cortada no lugar errado no chinês e no japonês, rn/m e acentos perdidos nos latinos…`, () => ai.ocrErrorsPrompt, (v) => { ai.ocrErrorsPrompt = v; }, OCR_ERRORS_PROMPT, 'ocrErrorsPrompt'),
            promptCard('Prompt dos componentes', `O que a IA põe no campo "components" do card. Só é enviado com Componentes ligado. ${escapeHtml('{{componentParts}}')} vira o que são as partes no idioma (caracteres no chinês e no japonês, sílabas com hanja no coreano, raízes e afixos nos outros) e ${escapeHtml('{{componentReading}}')} o campo de leitura de cada parte (pinyin, kana ou hanja; vazio nos outros idiomas). Variáveis: ${vars}.`, () => ai.componentsPrompt, (v) => { ai.componentsPrompt = v; }, COMPONENTS_PROMPT, 'componentsPrompt'),
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
            cardRow({icon: 'sparkle', title: 'Corrigir erros de OCR com IA', desc: 'A IA sabe que a frase e a palavra vieram do OCR: traduz o que o texto quis dizer e destaca a palavra mesmo que ela tenha sido lida errado. Desligado, ela traduz exatamente o que o OCR leu, e o pedido fica menor.',
                control: aiControl('contextOcrFix', 'Corrigir erros de OCR com IA', toggle(ai.contextOcrFix !== false, (v) => { ai.contextOcrFix = v; save(); renderDetail(); }))}),
            promptCard('Prompt da tradução', `A tradução da palavra deve vir entre **asteriscos duplos** para ser destacada. Variáveis: ${vars}.`, () => ai.contextPrompt, (v) => { ai.contextPrompt = v; }, CONTEXT_PROMPT, 'contextPrompt'),
            ai.contextOcrFix === false ? null : promptCard('Prompt de erros do OCR', `O aviso que vai no fim do prompt da tradução, com o interruptor acima ligado. ${escapeHtml('{{ocrErrors}}')} vira os erros típicos da escrita do idioma do OCR, como no card da palavra.`, () => ai.contextOcrPrompt, (v) => { ai.contextOcrPrompt = v; }, CONTEXT_OCR_PROMPT, 'contextOcrPrompt'),
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
    const vars = escapeHtml('{{target}} (palavra), {{sentence}} (a frase da palavra), {{wordLanguage}} (idioma do OCR), {{definitionLanguage}}, {{translationLanguage}}, {{explanationLanguage}}');
    root.append(
        el('h2', {}, AI_NAMES['ai-images']),
        el('p', {class: 'lead'}, 'Mostra imagens da palavra no popup, como o Migaku: a IA transforma a palavra num termo de busca em inglês, e o Lookitan procura esse termo no Bing Imagens. Sem IA, busca a palavra como está na tela. Passe o mouse numa imagem e clique na marca do canto para mandá-la ao Anki (marcador {image}); clique na imagem para ampliar.'),
        el('h3', {}, 'Configurações do dicionário'),
        el('div', {class: 'group'},
            cardRow({icon: 'image', title: 'Mostrar imagens automaticamente', desc: 'Desligado: as imagens só aparecem quando você clica em "Mostrar imagens" no popup, como o botão Explicar.',
                control: aiControl('autoShowImages', 'Mostrar imagens automaticamente', toggle(ai.autoShowImages === true, (v) => { ai.autoShowImages = v; save(); }))}),
            cardRow({icon: 'sparkle', title: 'Criar o termo de busca com a IA', desc: 'Desligado: busca a palavra como ela aparece na tela, sem gastar tokens.',
                control: aiControl('imageQueryUseAi', 'Criar o termo de busca com a IA', toggle(ai.imageQueryUseAi !== false, (v) => { ai.imageQueryUseAi = v; save(); }))}),
            cardRow({icon: 'sparkle', title: 'Usar a palavra corrigida pelo card', desc: 'Quando o card "IA — palavra no contexto" corrige uma palavra que o OCR leu errado ("OCR: 進 → 盡"), as imagens buscam a palavra corrigida; quando ele explica uma expressão maior que foi achada no texto ("Socrates Awards" para "Socrates"), buscam a expressão, que já vem corrigida. Nenhum outro pedido à IA. Desligado, buscam a palavra exatamente como o OCR leu.',
                control: aiControl('imageUseCardWord', 'Usar a palavra corrigida pelo card', toggle(ai.imageUseCardWord !== false, (v) => { ai.imageUseCardWord = v; save(); }))})),
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
