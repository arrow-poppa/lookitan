// "Imagens IA": pictures made by an image model from the user's own prompt, with the user's own key
// (BYOK). OpenAI and compatible servers use the Images API (`/v1/images/generations`); Gemini uses
// generateContent with an IMAGE answer, or `:predict` for Imagen; OpenRouter, and other chat
// servers that answer with pictures, get a chat request with `modalities: ["image", "text"]`.

import {CustomProvider} from './ai/ai-provider.js';
import {fillLanguages, languageName} from './ai/word-card.js';

/** The providers that can make pictures. DeepSeek cannot. */
export const IMAGE_GEN_PROVIDERS = ['openai', 'gemini', 'custom'];

export const IMAGE_GEN_DEFAULT_MODELS = {openai: 'gpt-image-1-mini', gemini: 'gemini-2.5-flash-image', custom: ''};

/** Suggestions for the model box; any other model name can be typed. */
export const IMAGE_GEN_MODELS = {
    openai: ['gpt-image-1-mini', 'gpt-image-1', 'gpt-image-1.5', 'dall-e-3'],
    gemini: ['gemini-2.5-flash-image', 'gemini-3.1-flash-image-preview', 'gemini-3-pro-image-preview', 'imagen-4.0-fast-generate-001', 'imagen-4.0-generate-001'],
    custom: ['google/gemini-2.5-flash-image', 'openai/gpt-5-image-mini', 'grok-2-image'],
};

/** Pictures made per click: 1 by default, at most this many. */
export const IMAGE_GEN_MAX_COUNT = 4;

export const IMAGE_GEN_ASPECTS = ['1:1', '16:9', '9:16', '4:3', '3:4'];
export const IMAGE_GEN_QUALITIES = ['low', 'medium', 'high', 'auto'];

const REQUEST_TIMEOUT_MS = 120000;

export const IMAGE_GEN_PROMPT =
    'A simple, clear illustration for a vocabulary flashcard that shows the meaning of the {{wordLanguage}} word "{{target}}" ' +
    'exactly as it is used in this sentence: "{{sentence}}"\n\n' +
    'Show the action, thing or idea itself, so the picture explains the word on its own. ' +
    'Plain background. No text, letters, numbers or captions in the picture.';

/**
 * The prompt as sent: {{target}}, {{sentence}}, {{wordLanguage}} (the OCR language),
 * {{definitionLanguage}} and {{translationLanguage}} filled in.
 * @param {string} template
 * @param {{word?: string, sentence?: string}} payload
 * @param {{ocrLanguage?: string, definitionLanguage?: string, translationLanguage?: string}} settings
 */
export function imageGenPrompt(template, payload, settings) {
    const text = typeof template === 'string' && template.trim() ? template : IMAGE_GEN_PROMPT;
    return fillLanguages(text, settings)
        .replaceAll('{{wordLanguage}}', languageName(settings.ocrLanguage || 'en-US'))
        .replaceAll('{{target}}', payload.word || '')
        .replaceAll('{{sentence}}', payload.sentence || payload.word || '');
}

/** How many pictures one click makes, between 1 and IMAGE_GEN_MAX_COUNT. */
export function imageGenCount(value) {
    const n = Math.round(Number(value));
    return Number.isFinite(n) ? Math.min(IMAGE_GEN_MAX_COUNT, Math.max(1, n)) : 1;
}

/** The account of the chosen provider, or null when it cannot make pictures. */
function account(options) {
    if (!IMAGE_GEN_PROVIDERS.includes(options?.provider)) { return null; }
    const cfg = options[options.provider];
    return cfg && typeof cfg === 'object' ? cfg : null;
}

/** True when the provider has a key and a model (and, for "Personalizado", an endpoint). */
export function isImageGenConfigured(options) {
    const cfg = account(options);
    if (!cfg || !String(cfg.apiKey || '').trim() || !String(cfg.model || '').trim()) { return false; }
    return options.provider !== 'custom' || /^https?:\/\//i.test(String(cfg.endpoint || '').trim());
}

/** OpenAI's sizes for an aspect ratio: gpt-image-* and DALL·E 3 have different ones. */
function openAiSize(model, aspect) {
    const wide = aspect === '16:9' || aspect === '4:3';
    const tall = aspect === '9:16' || aspect === '3:4';
    if (/^dall-e-2/.test(model)) { return '1024x1024'; }
    if (/^dall-e-3/.test(model)) { return wide ? '1792x1024' : tall ? '1024x1792' : '1024x1024'; }
    return wide ? '1536x1024' : tall ? '1024x1536' : '1024x1024';
}

function extraBody(json) {
    if (typeof json !== 'string' || !json.trim()) { return {}; }
    try {
        const value = JSON.parse(json);
        return value && typeof value === 'object' && !Array.isArray(value) ? value : {};
    } catch (_e) {
        throw new Error('O JSON extra do provedor de imagens não é válido.');
    }
}

function isOpenRouter(endpoint) {
    try {
        const {hostname} = new URL(endpoint);
        return hostname === 'openrouter.ai' || hostname.endsWith('.openrouter.ai');
    } catch (_e) {
        return false;
    }
}

/**
 * The HTTP requests that make `count` pictures. Servers that make several at once get one request;
 * the others (Gemini's image models, chat servers, DALL·E 3) get one request per picture.
 * @param {object} options the dictionary's AI options (see aiOptionsFor)
 * @param {string} prompt
 * @param {{count?: number, aspect?: string, quality?: string}} [choice]
 * @returns {{url: string, headers: Record<string, string>, body: object, format: 'images'|'gemini'|'imagen'|'chat', count: number}[]}
 */
export function imageGenRequests(options, prompt, {count = 1, aspect = '1:1', quality = 'low'} = {}) {
    const cfg = account(options);
    if (!cfg) { throw new Error('Este provedor não gera imagens. Escolha OpenAI, Google Gemini ou Personalizado.'); }
    const n = imageGenCount(count);
    const key = String(cfg.apiKey || '').trim();
    const model = String(cfg.model || '').trim();
    const ratio = IMAGE_GEN_ASPECTS.includes(aspect) ? aspect : '1:1';
    const each = (request) => Array.from({length: n}, () => ({...request, count: 1}));
    if (options.provider === 'openai') {
        const headers = {'Content-Type': 'application/json', 'Authorization': `Bearer ${key}`};
        const url = 'https://api.openai.com/v1/images/generations';
        const size = openAiSize(model, ratio);
        if (/^dall-e/.test(model)) {
            const body = {model, prompt, size, response_format: 'b64_json', n: 1};
            if (/^dall-e-3/.test(model)) {
                // DALL·E 3 makes one picture per request, in "standard" or "hd".
                body.quality = quality === 'high' ? 'hd' : 'standard';
                return each({url, headers, body, format: 'images'});
            }
            return [{url, headers, body: {...body, n}, format: 'images', count: n}];
        }
        const body = {model, prompt, n, size};
        if (IMAGE_GEN_QUALITIES.includes(quality)) { body.quality = quality; }
        return [{url, headers, body, format: 'images', count: n}];
    }
    if (options.provider === 'gemini') {
        let id = model.replace(/^models\//, '');
        const vertex = id.endsWith('-vertex');
        if (vertex) { id = id.replace(/-vertex$/, ''); }
        // Vertex AI Express takes the key in the address, as the word card sends it there.
        const base = vertex ? 'https://aiplatform.googleapis.com/v1/publishers/google/models' : 'https://generativelanguage.googleapis.com/v1beta/models';
        const query = vertex ? `?key=${encodeURIComponent(key)}` : '';
        const headers = vertex ? {'Content-Type': 'application/json'} : {'Content-Type': 'application/json', 'x-goog-api-key': key};
        if (/^imagen/.test(id)) {
            return [{url: `${base}/${id}:predict${query}`, headers, format: 'imagen', count: n,
                body: {instances: [{prompt}], parameters: {sampleCount: n, aspectRatio: ratio}}}];
        }
        return each({url: `${base}/${id}:generateContent${query}`, headers, format: 'gemini', body: {
            contents: [{role: 'user', parts: [{text: prompt}]}],
            generationConfig: {responseModalities: ['TEXT', 'IMAGE'], imageConfig: {aspectRatio: ratio}},
        }});
    }
    const endpoint = String(cfg.endpoint || '').trim();
    const extra = extraBody(cfg.requestBodyJson);
    const headers = {'Content-Type': 'application/json', 'Authorization': `Bearer ${key}`,
        ...(isOpenRouter(endpoint) ? {'HTTP-Referer': 'https://github.com/arrow-poppa/lookitan', 'X-OpenRouter-Title': 'Lookitan'} : {})};
    if (/\/images\/generations\/?$/i.test(endpoint.split('?')[0])) {
        // An OpenAI-compatible Images API (xAI, Together, a local server…). The size is left to the
        // server, since not every one takes it; "JSON extra" can add it.
        return [{url: endpoint, headers, format: 'images', count: n, body: {model, prompt, n, ...extra}}];
    }
    const body = {model, messages: [{role: 'user', content: prompt}], modalities: ['image', 'text'], image_config: {aspect_ratio: ratio}};
    // The same thinking and OpenRouter routing options as the text requests of "Personalizado"
    // (reasoning, or thinking/reasoning_effort elsewhere; provider order/only/ignore, fallbacks).
    const text = new CustomProvider(cfg);
    text._applyReasoningOptions(body);
    text._applyOpenRouterProviderOptions(body);
    return each({url: endpoint, headers, format: 'chat', body: {...body, ...extra, stream: false}});
}

/** A data URL's type and base64 data, or null. */
function dataUrl(text) {
    const m = /^data:([^;,]+)(?:;[^,]*)?;base64,(.*)$/s.exec(String(text || ''));
    return m ? {mimeType: m[1].toLowerCase(), data: m[2]} : null;
}

/**
 * The pictures in one answer, as {mimeType, data} (base64) or {url}.
 * @param {'images'|'gemini'|'imagen'|'chat'} format
 * @param {any} json
 * @returns {({mimeType: string, data: string}|{url: string})[]}
 */
export function parseGeneratedImages(format, json) {
    const out = [];
    const add = (value) => {
        if (typeof value !== 'string' || !value) { return; }
        const inline = dataUrl(value);
        if (inline) { out.push(inline); } else if (/^https?:\/\//i.test(value)) { out.push({url: value}); }
    };
    if (format === 'images') {
        for (const d of Array.isArray(json?.data) ? json.data : []) {
            if (typeof d?.b64_json === 'string' && d.b64_json) { out.push({mimeType: `image/${json.output_format || 'png'}`.replace('jpg', 'jpeg'), data: d.b64_json}); }
            else { add(d?.url); }
        }
    } else if (format === 'imagen') {
        for (const p of Array.isArray(json?.predictions) ? json.predictions : []) {
            if (typeof p?.bytesBase64Encoded === 'string' && p.bytesBase64Encoded) { out.push({mimeType: p.mimeType || 'image/png', data: p.bytesBase64Encoded}); }
        }
    } else if (format === 'gemini') {
        for (const c of Array.isArray(json?.candidates) ? json.candidates : []) {
            for (const part of Array.isArray(c?.content?.parts) ? c.content.parts : []) {
                const inline = part?.inlineData || part?.inline_data;
                if (typeof inline?.data === 'string' && inline.data) { out.push({mimeType: inline.mimeType || inline.mime_type || 'image/png', data: inline.data}); }
            }
        }
    } else {
        for (const c of Array.isArray(json?.choices) ? json.choices : []) {
            const message = c?.message || {};
            for (const image of Array.isArray(message.images) ? message.images : []) { add(image?.image_url?.url || image?.url); }
            for (const part of Array.isArray(message.content) ? message.content : []) {
                if (part?.type === 'image_url') { add(part.image_url?.url); }
            }
        }
    }
    return out;
}

/** Why an answer had no picture: the server's error, a safety block, or the model's own words. */
function noPictureReason(format, json, status) {
    const error = json?.error;
    const message = typeof error === 'string' ? error : error?.message;
    if (message) { return `${status ? `${status}: ` : ''}${message}`; }
    if (format === 'gemini') {
        const block = json?.promptFeedback?.blockReason || json?.candidates?.[0]?.finishReason;
        const said = (json?.candidates?.[0]?.content?.parts || []).map((p) => p?.text).filter(Boolean).join(' ').trim();
        if (said) { return said.slice(0, 300); }
        if (block && block !== 'STOP') { return `bloqueado (${block})`; }
    }
    if (format === 'chat') {
        const said = json?.choices?.[0]?.message?.content;
        if (typeof said === 'string' && said.trim()) { return said.trim().slice(0, 300); }
    }
    return status ? `resposta ${status}` : 'nenhuma imagem na resposta';
}

function deadline(ms, signal) {
    const controller = new AbortController();
    const timer = setTimeout(() => controller.abort(new DOMException('Image generation timed out', 'TimeoutError')), ms);
    const abort = () => controller.abort(signal.reason);
    signal?.addEventListener('abort', abort, {once: true});
    return {signal: controller.signal, done: () => { clearTimeout(timer); signal?.removeEventListener('abort', abort); }};
}

function fromBase64(data) {
    const binary = atob(data.replace(/\s+/g, ''));
    const bytes = new Uint8Array(binary.length);
    for (let i = 0; i < binary.length; i++) { bytes[i] = binary.charCodeAt(i); }
    return bytes;
}

/**
 * Makes the pictures. Each is {blob}, or {url} when the server answered with an address it would
 * not serve. Requests that fail are skipped while another one gives a picture;
 * when none does, the first error is thrown.
 * @param {object} options
 * @param {string} prompt
 * @param {{count?: number, aspect?: string, quality?: string, signal?: AbortSignal, fetch?: typeof globalThis.fetch}} [choice]
 * @returns {Promise<({blob: Blob}|{url: string})[]>}
 */
export async function generateImages(options, prompt, {signal, fetch: request, ...choice} = {}) {
    const doFetch = request || (await import('@tauri-apps/plugin-http')).fetch;
    const requests = imageGenRequests(options, prompt, choice);
    const results = await Promise.allSettled(requests.map(async (r) => {
        const timeout = deadline(REQUEST_TIMEOUT_MS, signal);
        try {
            const response = await doFetch(r.url, {method: 'POST', headers: r.headers, body: JSON.stringify(r.body), signal: timeout.signal});
            const text = await response.text();
            let json = null;
            try { json = JSON.parse(text); } catch (_e) { /* not JSON: the status says enough */ }
            const pictures = response.ok ? parseGeneratedImages(r.format, json) : [];
            if (pictures.length === 0) { throw new Error(`Imagens IA: ${noPictureReason(r.format, json, response.ok ? 0 : response.status)}`); }
            return pictures;
        } finally {
            timeout.done();
        }
    }));
    if (signal?.aborted) { throw signal.reason || new DOMException('Request cancelled', 'AbortError'); }
    const pictures = results.filter((r) => r.status === 'fulfilled').flatMap((r) => r.value);
    if (pictures.length === 0) { throw results.find((r) => r.status === 'rejected')?.reason || new Error('Imagens IA: nenhuma imagem na resposta'); }
    return Promise.all(pictures.map((p) => ('url' in p ? download(doFetch, p.url, signal) : {blob: new Blob([fromBase64(p.data)], {type: p.mimeType})})));
}

/**
 * A picture the server keeps at an address (some answer only that, and OpenAI's addresses expire
 * in an hour): fetched once, so it can be kept. When it cannot be read, the address stays.
 */
async function download(doFetch, url, signal) {
    const timeout = deadline(REQUEST_TIMEOUT_MS, signal);
    try {
        const response = await doFetch(url, {method: 'GET', signal: timeout.signal});
        const type = (response.headers?.get?.('content-type') || '').split(';')[0].trim().toLowerCase();
        if (response.ok && type.startsWith('image/')) { return {blob: new Blob([await response.arrayBuffer()], {type})}; }
    } catch (_e) {
        // The address is shown as it is.
    } finally {
        timeout.done();
    }
    return {url};
}
