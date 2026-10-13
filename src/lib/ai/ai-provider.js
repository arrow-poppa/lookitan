// Ported to Lookitan from arrow-poppa/yomitan-byok-ai (branch feature/byok-ai-explanations,
// ext/js/comm/ai-provider.js). Changes: local imports, and `fetch` comes from the Tauri HTTP
// plugin so requests go through the app instead of the WebView (no CORS, streaming kept).
/*
 * Copyright (C) 2023-2026  Yomitan Authors
 *
 * This program is free software: you can redistribute it and/or modify
 * it under the terms of the GNU General Public License as published by
 * the Free Software Foundation, either version 3 of the License, or
 * (at your option) any later version.
 *
 * This program is distributed in the hope that it will be useful,
 * but WITHOUT ANY WARRANTY; without even the implied warranty of
 * MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE.  See the
 * GNU General Public License for more details.
 *
 * You should have received a copy of the GNU General Public License
 * along with this program.  If not, see <https://www.gnu.org/licenses/>.
 */

import {fetch} from '@tauri-apps/plugin-http';
import {parseJson, readResponseJson} from './json.js';
import {isObjectNotArray} from './object-utilities.js';

/**
 * Abstract base class for AI providers
 */
export class AIProvider {
    /**
     * @param {string} name The name of the provider
     * @param {import('settings').AIProviderConfig} config Provider configuration
     */
    constructor(name, config) {
        /** @type {string} */
        this._name = name;
        /** @type {import('settings').AIProviderConfig} */
        this._config = config;
    }

    /**
     * Get the provider name
     * @returns {string}
     */
    get name() {
        return this._name;
    }

    /**
     * Generate contextual explanation for a term
     * @param {string} _term The term to explain
     * @param {string} _sentence The sentence context
     * @param {string} _customPrompt Custom prompt template with placeholders
     * @param {string} [_systemPrompt] Optional system prompt text
     * @param {AbortSignal} [_abortSignal] Optional abort signal to cancel the request
     * @returns {Promise<string>}
     */
    async generateExplanation(_term, _sentence, _customPrompt, _systemPrompt, _abortSignal) {
        throw new Error('generateExplanation must be implemented by subclass');
    }

    /**
     * Check if the provider is properly configured
     * @returns {boolean}
     */
    isConfigured() {
        return this._config && !!this._config.apiKey;
    }

    /**
     * Replace placeholders in prompt template
     * @param {string} template The prompt template
     * @param {string} term The target term
     * @param {string} sentence The sentence context
     * @returns {string}
     */
    _replacePlaceholders(template, term, sentence) {
        return template
            .replace(/\{\{target\}\}/g, term)
            .replace(/\{\{sentence\}\}/g, sentence);
    }

    /**
     * Get a normalized temperature value for provider requests.
     * @returns {number}
     */
    _getTemperature() {
        const temperature = this._config?.temperature;
        return typeof temperature === 'number' && Number.isFinite(temperature) ?
            Math.max(0, Math.min(2, temperature)) :
            0.7;
    }

    /**
     * @returns {boolean}
     */
    _isOpenRouterEndpoint() {
        try {
            const {hostname} = new URL(this._config?.endpoint || '');
            return hostname === 'openrouter.ai' || hostname.endsWith('.openrouter.ai');
        } catch (_error) {
            return false;
        }
    }

    /**
     * Adds DeepSeek/OpenRouter reasoning options to an OpenAI-compatible request body.
     * @param {Record<string, unknown>} requestBody
     */
    _applyReasoningOptions(requestBody) {
        const thinkingMode = this._config?.thinkingMode || '';
        const thinkingIntensity = this._config?.thinkingIntensity || '';
        const customThinkingIntensity = typeof this._config?.customThinkingIntensity === 'string' ? this._config.customThinkingIntensity.trim() : '';
        const effectiveThinkingIntensity = thinkingIntensity === 'custom' ? customThinkingIntensity : thinkingIntensity;
        const hasThinkingMode = thinkingMode === 'enabled' || thinkingMode === 'disabled';
        const hasThinkingIntensity = effectiveThinkingIntensity === 'high' || effectiveThinkingIntensity === 'max' || effectiveThinkingIntensity.length > 0;
        if (!hasThinkingMode && !hasThinkingIntensity) { return; }

        if (this._isOpenRouterEndpoint()) {
            const reasoning = /** @type {Record<string, unknown>} */ ({});
            if (thinkingMode === 'disabled') {
                reasoning.effort = 'none';
            } else if (thinkingIntensity === 'custom' && customThinkingIntensity) {
                Object.assign(reasoning, this._parseCustomReasoningValue(customThinkingIntensity));
            } else if (hasThinkingIntensity) {
                reasoning.effort = effectiveThinkingIntensity === 'max' ? 'xhigh' : effectiveThinkingIntensity;
            } else if (hasThinkingMode) {
                reasoning.enabled = true;
            }
            reasoning.exclude = true;
            requestBody.reasoning = reasoning;
            return;
        }

        if (hasThinkingMode) {
            requestBody.thinking = {type: thinkingMode};
        }
        if (thinkingMode !== 'disabled' && hasThinkingIntensity) {
            requestBody.reasoning_effort = effectiveThinkingIntensity;
        }
    }

    /**
     * Parses the manual Custom Thinking Intensity Value.
     * @param {string} value
     * @returns {Record<string, unknown>}
     */
    _parseCustomReasoningValue(value) {
        const trimmed = value.trim();
        if (!trimmed) { return {}; }
        if (/^\d+$/.test(trimmed)) {
            return {max_tokens: Number.parseInt(trimmed, 10)};
        }
        if (trimmed.startsWith('{')) {
            try {
                const parsed = /** @type {unknown} */ (parseJson(trimmed));
                if (isObjectNotArray(parsed)) {
                    return parsed;
                }
            } catch (_error) {
                // Fall through to effort below for invalid JSON.
            }
        }
        return {effort: trimmed === 'max' ? 'xhigh' : trimmed};
    }

    /**
     * Merges custom request body JSON into a request body.
     * @param {Record<string, unknown>} requestBody
     * @throws {Error} If the configured value is not a JSON object.
     */
    _applyCustomRequestBodyJson(requestBody) {
        const raw = typeof this._config?.requestBodyJson === 'string' ? this._config.requestBodyJson.trim() : '';
        if (!raw) { return; }
        /** @type {unknown} */
        let parsed;
        try {
            parsed = parseJson(raw);
        } catch (error) {
            const message = error instanceof Error ? error.message : `${error}`;
            throw new Error(`Invalid Custom Request Body JSON: ${message}`);
        }
        if (!isObjectNotArray(parsed)) {
            throw new Error('Invalid Custom Request Body JSON: expected a JSON object.');
        }
        this._mergeRequestBody(requestBody, parsed);
    }

    /**
     * @param {Record<string, unknown>} target
     * @param {Record<string, unknown>} source
     */
    _mergeRequestBody(target, source) {
        for (const [key, value] of Object.entries(source)) {
            if (key === '__proto__' || key === 'prototype' || key === 'constructor') { continue; }
            const existing = target[key];
            if (
                existing && typeof existing === 'object' && !Array.isArray(existing) &&
                value && typeof value === 'object' && !Array.isArray(value)
            ) {
                this._mergeRequestBody(/** @type {Record<string, unknown>} */ (existing), /** @type {Record<string, unknown>} */ (value));
            } else {
                target[key] = value;
            }
        }
    }

    /**
     * Extracts text from common OpenAI-compatible response shapes.
     * @param {unknown} content
     * @returns {string}
     */
    _stringifyMessageContent(content) {
        if (typeof content === 'string') { return content; }
        if (Array.isArray(content)) {
            return /** @type {unknown[]} */ (content).map((part) => this._getContentPartText(part)).join('');
        }
        return this._getContentPartText(content);
    }

    /**
     * @param {unknown} part
     * @returns {string}
     */
    _getContentPartText(part) {
        if (typeof part === 'string') { return part; }
        if (!isObjectNotArray(part)) { return ''; }
        if (typeof part.text === 'string') { return part.text; }
        return typeof part.content === 'string' ? part.content : '';
    }

    /**
     * @param {unknown} payload
     * @returns {?Record<string, unknown>}
     */
    _getFirstChoice(payload) {
        if (!isObjectNotArray(payload) || !Array.isArray(payload.choices)) { return null; }
        const choice = /** @type {unknown[]} */ (payload.choices)[0];
        return isObjectNotArray(choice) ? choice : null;
    }

    /**
     * Adds OpenRouter provider routing options to an OpenAI-compatible request body.
     * @param {Record<string, unknown>} requestBody
     */
    _applyOpenRouterProviderOptions(requestBody) {
        if (!this._isOpenRouterEndpoint()) { return; }

        const mode = this._config?.providerRoutingMode || '';
        const providerSlugs = typeof this._config?.providerSlugs === 'string' ?
            this._config.providerSlugs.split(/[\n,]+/).map((slug) => slug.trim()).filter((slug) => slug.length > 0) :
            [];
        const providerOptions = {};

        switch (mode) {
            case 'order':
                if (providerSlugs.length > 0) { providerOptions.order = providerSlugs; }
                break;
            case 'only':
                if (providerSlugs.length > 0) { providerOptions.only = providerSlugs; }
                break;
            case 'ignore':
                if (providerSlugs.length > 0) { providerOptions.ignore = providerSlugs; }
                break;
        }

        if (typeof this._config?.providerAllowFallbacks === 'boolean') {
            providerOptions.allow_fallbacks = this._config.providerAllowFallbacks;
        }

        if (Object.keys(providerOptions).length > 0) {
            requestBody.provider = providerOptions;
        }
    }

    /**
     * Whether this provider can deliver a response incrementally.
     * @returns {boolean}
     */
    supportsStreaming() {
        return false;
    }

    /**
     * Generate a contextual explanation, delivering it in pieces as the model writes it.
     * The default implementation performs a normal request and hands over the whole answer
     * as a single chunk, so a provider without streaming support still behaves correctly.
     * @param {string} term The term to explain
     * @param {string} sentence The sentence context
     * @param {string} customPrompt Custom prompt template with placeholders
     * @param {string} [systemPrompt] Optional system prompt text
     * @param {AbortSignal} [abortSignal] Optional abort signal to cancel the request
     * @param {(chunk: string, full: string) => void} [onChunk] Called for every piece of text received
     * @returns {Promise<string>}
     */
    async generateExplanationStream(term, sentence, customPrompt, systemPrompt, abortSignal, onChunk) {
        const explanation = await this.generateExplanation(term, sentence, customPrompt, systemPrompt, abortSignal);
        if (typeof onChunk === 'function' && explanation) { onChunk(explanation, explanation); }
        return explanation;
    }

    /**
     * Resolves the overloaded `(systemPrompt, signal)` / `(signal)` argument pair which every
     * provider accepts for backwards compatibility.
     * @param {string} term
     * @param {string} sentence
     * @param {string|AbortSignal} [systemPromptOrAbort]
     * @param {AbortSignal} [abortSignal]
     * @returns {{systemPrompt: string, signal: (AbortSignal|undefined)}}
     */
    _resolvePromptArguments(term, sentence, systemPromptOrAbort, abortSignal) {
        let systemPrompt = '';
        let signal = abortSignal;
        if (typeof systemPromptOrAbort === 'string') {
            systemPrompt = this._replacePlaceholders(systemPromptOrAbort, term, sentence);
        } else if (typeof systemPromptOrAbort === 'object' && systemPromptOrAbort !== null) {
            signal = /** @type {AbortSignal} */ (systemPromptOrAbort);
        }
        return {systemPrompt, signal};
    }

    /**
     * Reads a `text/event-stream` body, invoking the callback once per event with its joined
     * `data:` payload. The reader is always cancelled on the way out, so an aborted request
     * tears the connection down instead of leaving the model generating tokens server side.
     * @param {Response} response
     * @param {(payload: string) => (void|boolean)} onEvent Return false when the stream is complete.
     * @param {AbortSignal} [signal]
     * @returns {Promise<void>}
     */
    async _readSseStream(response, onEvent, signal) {
        const body = response.body;
        if (!body || typeof body.getReader !== 'function') {
            throw new Error('Streaming response body is not available');
        }
        const reader = body.getReader();
        const decoder = new TextDecoder();
        const separatorPattern = /\r\n\r\n|\n\n|\r\r/;
        let buffer = '';
        const checkAbort = () => {
            if (signal?.aborted) { throw signal.reason || new DOMException('Request cancelled', 'AbortError'); }
        };
        const abort = () => { void reader.cancel().catch(() => {}); };
        /** @param {string} rawEvent */
        const emit = (rawEvent) => {
            checkAbort();
            /** @type {string[]} */
            const dataLines = [];
            for (const line of rawEvent.split(/\r\n|\n|\r/)) {
                // Anything else (`event:`, `id:`, `: keep-alive` comments) carries no text.
                if (line.startsWith('data:')) { dataLines.push(line.slice(5).replace(/^ /, '')); }
            }
            return dataLines.length === 0 || onEvent(dataLines.join('\n')) !== false;
        };
        signal?.addEventListener('abort', abort, {once: true});
        try {
            checkAbort();
            for (;;) {
                const {done, value} = await reader.read();
                checkAbort();
                buffer += done ? decoder.decode() : decoder.decode(value, {stream: true});
                let match;
                while ((match = separatorPattern.exec(buffer)) !== null) {
                    if (!emit(buffer.slice(0, match.index))) { return; }
                    buffer = buffer.slice(match.index + match[0].length);
                }
                if (done) { break; }
            }
            // Some gateways close the stream without the trailing blank line.
            if (buffer.trim().length > 0) { emit(buffer); }
        } finally {
            signal?.removeEventListener('abort', abort);
            try { await reader.cancel(); } catch (_error) { /* the stream is already gone */ }
            reader.releaseLock();
        }
    }

    _providerError(label, payload, status, statusText = '') {
        const value = isObjectNotArray(payload) && typeof payload.error !== 'undefined' ? payload.error : payload;
        const detail = typeof value === 'string' ? value : isObjectNotArray(value) && typeof value.message === 'string' ? value.message : '';
        const error = new Error(`${label} API error${status ? `: ${status} ${statusText}`.trimEnd() : ''}${detail ? `: ${detail.slice(0, 500)}` : ''}`);
        error.status = status;
        return error;
    }

    async _responseError(label, response) {
        let detail;
        try {
            const text = await response.text();
            try { detail = parseJson(text); } catch (_error) { detail = text; }
        } catch (_error) { /* Status still identifies an error when its body cannot be read. */ }
        return this._providerError(label, detail, response.status, response.statusText);
    }

    /**
     * Extracts the incremental text of a single OpenAI-compatible stream chunk.
     * @param {unknown} payload
     * @returns {string}
     */
    _extractOpenAIStreamDelta(payload) {
        if (!isObjectNotArray(payload) || !Array.isArray(payload.choices)) { return ''; }
        const choice = /** @type {unknown[]} */ (payload.choices)[0];
        if (!isObjectNotArray(choice)) { return ''; }
        const delta = choice.delta;
        if (isObjectNotArray(delta)) {
            const {content} = delta;
            // `reasoning_content` is deliberately ignored: it is thinking, not the answer.
            return content === null || typeof content === 'undefined' ? '' : this._stringifyMessageContent(content);
        }
        // Legacy completion-style streams.
        return typeof choice.text === 'string' ? choice.text : '';
    }

    /**
     * Performs an OpenAI-compatible streaming chat completion request.
     * @param {string} label Provider name used in error messages
     * @param {string} url
     * @param {Record<string, string>} headers
     * @param {Record<string, unknown>} requestBody
     * @param {AbortSignal|undefined} signal
     * @param {(chunk: string, full: string) => void} [onChunk]
     * @returns {Promise<string>}
     */
    async _streamOpenAICompatibleRequest(label, url, headers, requestBody, signal, onChunk) {
        const response = await fetch(url, {
            method: 'POST',
            headers,
            signal,
            body: JSON.stringify(requestBody),
        });

        if (!response.ok) {
            throw await this._responseError(label, response);
        }

        let full = '';
        await this._readSseStream(response, (payload) => {
            if (payload === '[DONE]') {
                return false;
            }
            /** @type {unknown} */
            let parsed;
            try {
                parsed = parseJson(payload);
            } catch (_error) {
                return; // Keep-alive frames and non-JSON padding are not fatal.
            }
            const error = isObjectNotArray(parsed) ? parsed.error ?? this._getFirstChoice(parsed)?.error : undefined;
            if (typeof error !== 'undefined') {
                throw this._providerError(`${label} stream`, error, typeof error?.code === 'number' ? error.code : undefined);
            }
            const delta = this._extractOpenAIStreamDelta(parsed);
            if (delta) {
                full += delta;
                if (typeof onChunk === 'function') { onChunk(delta, full); }
            }
        }, signal);
        return full;
    }
}

/**
 * OpenAI provider implementation
 */
export class OpenAIProvider extends AIProvider {
    /**
     * @param {import('settings').AIProviderConfig} config
     */
    constructor(config) {
        super('openai', config);
    }

    /**
     * Whether this provider can deliver a response incrementally.
     * @returns {boolean}
     */
    supportsStreaming() {
        return true;
    }

    /**
     * Builds everything the request needs, shared by the streaming and non-streaming paths.
     * @param {string} term
     * @param {string} sentence
     * @param {string} customPrompt
     * @param {string|AbortSignal} [systemPromptOrAbort]
     * @param {AbortSignal} [abortSignal]
     * @returns {{url: string, headers: Record<string, string>, requestBody: Record<string, unknown>, signal: (AbortSignal|undefined), prompt: string, systemPrompt: string}}
     * @throws {Error} If the provider is not configured.
     */
    _buildRequest(term, sentence, customPrompt, systemPromptOrAbort, abortSignal) {
        if (!this.isConfigured()) {
            throw new Error('OpenAI provider is not configured');
        }

        const prompt = this._replacePlaceholders(customPrompt, term, sentence);
        const {systemPrompt, signal} = this._resolvePromptArguments(term, sentence, systemPromptOrAbort, abortSignal);

        return {
            url: 'https://api.openai.com/v1/chat/completions',
            headers: {
                'Content-Type': 'application/json',
                'Authorization': `Bearer ${this._config.apiKey}`,
            },
            requestBody: {
                model: this._config.model || 'gpt-4o-mini',
                messages: (
                    systemPrompt ?
[
    {role: 'system', content: systemPrompt},
    {role: 'user', content: prompt},
] :
[
    {role: 'user', content: prompt},
]
                ),
                temperature: this._getTemperature(),
            },
            signal,
            prompt,
            systemPrompt,
        };
    }

    /**
     * @param {string} term
     * @param {string} sentence
     * @param {string} customPrompt
     * @param {string} [systemPrompt]
     * @param {AbortSignal} [abortSignal]
     * @param {(chunk: string, full: string) => void} [onChunk]
     * @returns {Promise<string>}
     */
    async generateExplanationStream(term, sentence, customPrompt, systemPrompt, abortSignal, onChunk) {
        const request = this._buildRequest(term, sentence, customPrompt, systemPrompt, abortSignal);
        request.requestBody.stream = true;
        const text = await this._streamOpenAICompatibleRequest('OpenAI', request.url, request.headers, request.requestBody, request.signal, onChunk);
        return text.trim() || 'No explanation available.';
    }

    /**
     * @param {string} term
     * @param {string} sentence
     * @param {string} customPrompt
     * @param {string|AbortSignal} [systemPromptOrAbort]
     * @param {AbortSignal} [abortSignal]
     * @returns {Promise<string>}
     */
    async generateExplanation(term, sentence, customPrompt, systemPromptOrAbort, abortSignal) {
        const {url, headers, requestBody, signal} = this._buildRequest(term, sentence, customPrompt, systemPromptOrAbort, abortSignal);
        const response = await fetch(url, {
            method: 'POST',
            headers,
            signal,
            body: JSON.stringify(requestBody),
        });

        if (!response.ok) {
            throw await this._responseError('OpenAI', response);
        }

        const data = await readResponseJson(response);
        if (isObjectNotArray(data) && typeof data.error !== 'undefined') { throw this._providerError('OpenAI', data.error); }
        const choice = this._getFirstChoice(data);
        const message = choice !== null && isObjectNotArray(choice.message) ? choice.message : null;
        const plain = this._stringifyMessageContent(message?.content ?? choice?.text ?? '');
        return plain.trim() || 'No explanation available.';
    }
}

/**
 * Gemini provider implementation
 */
export class GeminiProvider extends AIProvider {
    /**
     * @param {import('settings').AIProviderConfig} config
     */
    constructor(config) {
        super('gemini', config);
    }

    /**
     * Whether this provider can deliver a response incrementally.
     * @returns {boolean}
     */
    supportsStreaming() {
        return true;
    }

    /**
     * Builds the request body for either route.
     * @param {string} promptText
     * @param {string} systemPromptText
     * @param {string} thinkingLevel
     * @param {boolean} isGemini3Model
     * @param {boolean} useVertexExpressRoute
     * @returns {Record<string, unknown>}
     */
    _buildRequestBody(promptText, systemPromptText, thinkingLevel, isGemini3Model, useVertexExpressRoute) {
        const generationConfig = /** @type {Record<string, unknown>} */ ({
            temperature: this._getTemperature(),
        });
        if (thinkingLevel && thinkingLevel !== '' && isGemini3Model) {
            generationConfig.thinkingConfig = {
                thinkingLevel: thinkingLevel,
            };
        }

        if (useVertexExpressRoute) {
            const body = /** @type {Record<string, unknown>} */ ({
                contents: [{
                    role: 'user',
                    parts: [{text: promptText}],
                }],
                generationConfig,
            });
            if (systemPromptText) {
                body.systemInstruction = {
                    parts: [{text: systemPromptText}],
                };
            }
            return body;
        }

        const body = /** @type {Record<string, unknown>} */ ({
            contents: [{
                parts: [{text: promptText}],
            }],
            generationConfig,
        });

        // System Instruction (suportado em 1.5+ e 3.0)
        if (systemPromptText) {
            body.systemInstruction = {parts: [{text: systemPromptText}]};
        }

        return body;
    }

    /**
     * Resolves the endpoint and body for a model id, for either the streaming or the
     * single-response method.
     * @param {string} modelIdRaw
     * @param {string} prompt
     * @param {string} systemPrompt
     * @param {boolean} [stream]
     * @returns {{url: string, body: Record<string, unknown>}}
     */
    _resolveRoute(modelIdRaw, prompt, systemPrompt, stream = false) {
        const vertexExpressBaseUrl = 'https://aiplatform.googleapis.com/v1/publishers/google/models';

        // Sanitização: remove prefixo 'models/' se existir
        const raw = modelIdRaw || 'gemini-2.5-flash';
        let modelId = raw.replace(/^models\//, '');
        let useVertexExpressRoute = false;

        // Normalizar modelos com sufixo -latest para o ID base
        if (modelId.endsWith('-latest')) {
            modelId = modelId.replace(/-latest$/, '');
        }

        if (modelId.endsWith('-vertex')) {
            modelId = modelId.replace(/-vertex$/, '');
            useVertexExpressRoute = true;
        }

        // Se for low-thinking, remove o sufixo customizado
        // ID Real: gemini-3-flash-preview (ou o que estiver configurado sem o sufixo)
        const isLowThinking = modelId.includes('low-thinking');
        const apiModelId = isLowThinking ? 'gemini-3-flash-preview' : modelId;
        if (isLowThinking) {
            useVertexExpressRoute = true;
        }

        // Compatibilidade com configuração antiga do Pro preview.
        if (apiModelId === 'gemini-3-pro-preview' || apiModelId === 'gemini-3-pro-image-preview') {
            useVertexExpressRoute = true;
        }

        // Verificar se é um modelo Gemini 3.x (apenas esses suportam thinking config)
        const isGemini3Model = apiModelId.includes('gemini-3');

        // Obter e sanitizar thinking level da configuração.
        let thinkingLevel = typeof this._config.thinkingLevel === 'string' ? this._config.thinkingLevel.toUpperCase() : '';

        // Fallback para modelos low-thinking antigos (compatibilidade)
        if (isLowThinking && !thinkingLevel) {
            thinkingLevel = 'LOW';
        }

        // Gemini 3 Pro aceita apenas LOW e HIGH. Níveis antigos/ambíguos
        // são convertidos para o equivalente compatível mais próximo.
        if (apiModelId === 'gemini-3-pro-preview' || apiModelId === 'gemini-3.1-pro-preview') {
            if (thinkingLevel === 'MINIMAL') {
                thinkingLevel = 'LOW';
            } else if (thinkingLevel === 'MEDIUM') {
                thinkingLevel = 'HIGH';
            } else if (thinkingLevel !== '' && thinkingLevel !== 'LOW' && thinkingLevel !== 'HIGH') {
                thinkingLevel = '';
            }
        }

        // Se não é modelo 3.x, limpar thinking level (modelos antigos não suportam)
        // Se é modelo 3.x mas não tem thinking level configurado, também limpar (usar default da API)
        if (!isGemini3Model || !thinkingLevel) {
            thinkingLevel = '';
        }

        const method = stream ? 'streamGenerateContent' : 'generateContent';
        const query = stream ? `?alt=sse&key=${this._config.apiKey}` : `?key=${this._config.apiKey}`;
        const url = useVertexExpressRoute ?
            `${vertexExpressBaseUrl}/${apiModelId}:${method}${query}` :
            `https://generativelanguage.googleapis.com/v1beta/models/${apiModelId}:${method}${query}`;
        const body = this._buildRequestBody(prompt, systemPrompt, thinkingLevel, isGemini3Model, useVertexExpressRoute);
        return {url, body};
    }

    /**
     * Extracts the answer text of one streamed Gemini chunk, skipping thinking parts.
     * @param {unknown} payload
     * @returns {string}
     */
    _extractGeminiText(payload) {
        if (!isObjectNotArray(payload) || !Array.isArray(payload.candidates)) { return ''; }
        const candidate = /** @type {unknown[]} */ (payload.candidates)[0];
        if (!isObjectNotArray(candidate) || !isObjectNotArray(candidate.content)) { return ''; }
        const {parts} = candidate.content;
        if (!Array.isArray(parts)) { return ''; }
        let text = '';
        for (const part of /** @type {unknown[]} */ (parts)) {
            if (!isObjectNotArray(part) || part.thought === true) { continue; }
            if (typeof part.text === 'string') { text += part.text; }
        }
        return text;
    }

    /**
     * @param {string} term
     * @param {string} sentence
     * @param {string} customPrompt
     * @param {string} [systemPrompt]
     * @param {AbortSignal} [abortSignal]
     * @param {(chunk: string, full: string) => void} [onChunk]
     * @returns {Promise<string>}
     */
    async generateExplanationStream(term, sentence, customPrompt, systemPrompt, abortSignal, onChunk) {
        if (!this.isConfigured()) {
            throw new Error('Gemini provider is not configured');
        }

        const prompt = this._replacePlaceholders(customPrompt, term, sentence);
        const {systemPrompt: resolvedSystemPrompt, signal} = this._resolvePromptArguments(term, sentence, systemPrompt, abortSignal);

        const {url, body} = this._resolveRoute(this._config.model || 'gemini-2.5-flash', prompt, resolvedSystemPrompt, true);
        const response = await fetch(url, {
            method: 'POST',
            headers: {'Content-Type': 'application/json'},
            signal,
            body: JSON.stringify(body),
        });

        if (!response.ok) {
            throw await this._responseError('Gemini', response);
        }

        let full = '';
        await this._readSseStream(response, (payload) => {
            /** @type {unknown} */
            let parsed;
            try {
                parsed = parseJson(payload);
            } catch (_error) {
                return;
            }
            if (isObjectNotArray(parsed) && typeof parsed.error !== 'undefined') {
                throw this._providerError('Gemini stream', parsed.error, typeof parsed.error?.code === 'number' ? parsed.error.code : undefined);
            }
            const delta = this._extractGeminiText(parsed);
            if (delta) {
                full += delta;
                if (typeof onChunk === 'function') { onChunk(delta, full); }
            }
        }, signal);
        return full.trim() || 'No explanation available.';
    }

    /**
     * @param {string} term
     * @param {string} sentence
     * @param {string} customPrompt
     * @param {string|AbortSignal} [systemPromptOrAbort]
     * @param {AbortSignal} [abortSignal]
     * @returns {Promise<string>}
     */
    async generateExplanation(term, sentence, customPrompt, systemPromptOrAbort, abortSignal) {
        if (!this.isConfigured()) {
            throw new Error('Gemini provider is not configured');
        }

        const prompt = this._replacePlaceholders(customPrompt, term, sentence);
        const {systemPrompt, signal} = this._resolvePromptArguments(term, sentence, systemPromptOrAbort, abortSignal);

        const modelIdRaw = this._config.model || 'gemini-2.5-flash';
        const {url, body} = this._resolveRoute(modelIdRaw, prompt, systemPrompt, false);
        const response = await fetch(url, {
            method: 'POST',
            headers: {'Content-Type': 'application/json'},
            signal,
            body: JSON.stringify(body),
        });
        if (!response.ok) {
            throw await this._responseError('Gemini', response);
        }

        const data = await readResponseJson(response);
        if (isObjectNotArray(data) && typeof data.error !== 'undefined') { throw this._providerError('Gemini', data.error); }
        const text = this._extractGeminiText(data);
        return text.trim() || 'No explanation available.';
    }
}

/**
 * Custom provider implementation (OpenAI v1-style API compatible)
 * Works with OpenRouter, Agent Router, and any OpenAI-compatible gateway
 */
export class CustomProvider extends AIProvider {
    /**
     * @param {import('settings').AIProviderConfig} config
     */
    constructor(config) {
        super('custom', config);
    }

    /**
     * Check if the provider is properly configured
     * @returns {boolean}
     */
    isConfigured() {
        return this._config && !!this._config.apiKey && !!this._config.endpoint;
    }

    /**
     * Whether this provider can deliver a response incrementally.
     * @returns {boolean}
     */
    supportsStreaming() {
        return true;
    }

    /**
     * Builds everything the request needs, shared by the streaming and non-streaming paths.
     * @param {string} term
     * @param {string} sentence
     * @param {string} customPrompt
     * @param {string|AbortSignal} [systemPromptOrAbort]
     * @param {AbortSignal} [abortSignal]
     * @returns {{url: string, headers: Record<string, string>, requestBody: Record<string, unknown>, signal: (AbortSignal|undefined), prompt: string, systemPrompt: string}}
     * @throws {Error} If the provider is not configured or custom JSON is invalid.
     */
    _buildRequest(term, sentence, customPrompt, systemPromptOrAbort, abortSignal) {
        if (!this.isConfigured()) {
            throw new Error('Custom provider is not configured. Please set endpoint, API key, and model.');
        }

        const prompt = this._replacePlaceholders(customPrompt, term, sentence);
        const {systemPrompt, signal} = this._resolvePromptArguments(term, sentence, systemPromptOrAbort, abortSignal);

        // Build OpenAI v1-style request body
        const messages = systemPrompt ?
[
    {role: 'system', content: systemPrompt},
    {role: 'user', content: prompt},
] :
[
    {role: 'user', content: prompt},
];

        const requestBody = {
            model: this._config.model || 'gpt-4o-mini',
            messages,
            temperature: this._getTemperature(),
        };
        this._applyReasoningOptions(requestBody);
        this._applyOpenRouterProviderOptions(requestBody);
        this._applyCustomRequestBodyJson(requestBody);
        requestBody.stream = false;
        if (!Array.isArray(requestBody.messages) || requestBody.messages.length === 0) {
            requestBody.messages = messages;
        }

        return {
            url: this._config.endpoint || '',
            headers: {
                'Content-Type': 'application/json',
                'Authorization': `Bearer ${this._config.apiKey.trim()}`,
                ...(this._isOpenRouterEndpoint() ?
{
    'HTTP-Referer': 'https://github.com/arrow-poppa/lookitan',
    'X-OpenRouter-Title': 'Lookitan',
} :
{}),
            },
            requestBody,
            signal,
            prompt,
            systemPrompt,
        };
    }

    /**
     * @param {string} term
     * @param {string} sentence
     * @param {string} customPrompt
     * @param {string} [systemPrompt]
     * @param {AbortSignal} [abortSignal]
     * @param {(chunk: string, full: string) => void} [onChunk]
     * @returns {Promise<string>}
     */
    async generateExplanationStream(term, sentence, customPrompt, systemPrompt, abortSignal, onChunk) {
        const request = this._buildRequest(term, sentence, customPrompt, systemPrompt, abortSignal);
        // Set after the custom JSON merge so a stored `"stream": false` cannot disable it.
        request.requestBody.stream = true;

        const text = await this._streamOpenAICompatibleRequest('Custom', request.url, request.headers, request.requestBody, request.signal, onChunk);
        return text.trim() || 'No explanation available.';
    }

    /**
     * @param {string} term
     * @param {string} sentence
     * @param {string} customPrompt
     * @param {string|AbortSignal} [systemPromptOrAbort]
     * @param {AbortSignal} [abortSignal]
     * @returns {Promise<string>}
     */
    async generateExplanation(term, sentence, customPrompt, systemPromptOrAbort, abortSignal) {
        const {url, headers, requestBody, signal} = this._buildRequest(term, sentence, customPrompt, systemPromptOrAbort, abortSignal);
        const response = await fetch(url, {
            method: 'POST',
            headers,
            signal,
            body: JSON.stringify(requestBody),
        });

        if (!response.ok) {
            throw await this._responseError('Custom', response);
        }

        const data = await readResponseJson(response);
        const choice = this._getFirstChoice(data);
        const responseError = isObjectNotArray(data) ? data.error ?? choice?.error : undefined;
        if (typeof responseError !== 'undefined') { throw this._providerError('Custom', responseError); }
        const message = choice !== null && isObjectNotArray(choice.message) ? choice.message : null;
        const plain = this._stringifyMessageContent(message?.content ?? choice?.text ?? '');
        return plain.trim() || 'No explanation available.';
    }
}

/**
 * DeepSeek provider implementation
 */
export class DeepSeekProvider extends AIProvider {
    /**
     * @param {import('settings').AIProviderConfig} config
     */
    constructor(config) {
        super('deepseek', config);
    }

    /**
     * Check if the provider is properly configured
     * @returns {boolean}
     */
    isConfigured() {
        return this._config && !!this._config.apiKey && !!this._config.model;
    }

    /**
     * Whether this provider can deliver a response incrementally.
     * @returns {boolean}
     */
    supportsStreaming() {
        return true;
    }

    /**
     * Builds everything the request needs, shared by the streaming and non-streaming paths.
     * @param {string} term
     * @param {string} sentence
     * @param {string} customPrompt
     * @param {string|AbortSignal} [systemPromptOrAbort]
     * @param {AbortSignal} [abortSignal]
     * @returns {{url: string, headers: Record<string, string>, requestBody: Record<string, unknown>, signal: (AbortSignal|undefined), prompt: string, systemPrompt: string}}
     * @throws {Error} If the provider is not configured.
     */
    _buildRequest(term, sentence, customPrompt, systemPromptOrAbort, abortSignal) {
        if (!this.isConfigured()) {
            throw new Error('DeepSeek provider is not configured. Please set API key and model.');
        }

        const prompt = this._replacePlaceholders(customPrompt, term, sentence);
        const {systemPrompt, signal} = this._resolvePromptArguments(term, sentence, systemPromptOrAbort, abortSignal);

        const messages = systemPrompt ?
[
    {role: 'system', content: systemPrompt},
    {role: 'user', content: prompt},
] :
[
    {role: 'user', content: prompt},
];

        const requestBody = {
            model: this._config.model.trim(),
            messages,
            stream: false,
            temperature: this._getTemperature(),
        };

        this._applyReasoningOptions(requestBody);

        return {
            url: 'https://api.deepseek.com/chat/completions',
            headers: {
                'Content-Type': 'application/json',
                'Authorization': `Bearer ${this._config.apiKey.trim()}`,
            },
            requestBody,
            signal,
            prompt,
            systemPrompt,
        };
    }

    /**
     * @param {string} term
     * @param {string} sentence
     * @param {string} customPrompt
     * @param {string} [systemPrompt]
     * @param {AbortSignal} [abortSignal]
     * @param {(chunk: string, full: string) => void} [onChunk]
     * @returns {Promise<string>}
     */
    async generateExplanationStream(term, sentence, customPrompt, systemPrompt, abortSignal, onChunk) {
        const request = this._buildRequest(term, sentence, customPrompt, systemPrompt, abortSignal);
        request.requestBody.stream = true;

        const text = await this._streamOpenAICompatibleRequest('DeepSeek', request.url, request.headers, request.requestBody, request.signal, onChunk);
        return text.trim() || 'No explanation available.';
    }

    /**
     * @param {string} term
     * @param {string} sentence
     * @param {string} customPrompt
     * @param {string|AbortSignal} [systemPromptOrAbort]
     * @param {AbortSignal} [abortSignal]
     * @returns {Promise<string>}
     */
    async generateExplanation(term, sentence, customPrompt, systemPromptOrAbort, abortSignal) {
        const {url, headers, requestBody, signal} = this._buildRequest(term, sentence, customPrompt, systemPromptOrAbort, abortSignal);
        const response = await fetch(url, {
            method: 'POST',
            headers,
            signal,
            body: JSON.stringify(requestBody),
        });

        if (!response.ok) {
            throw await this._responseError('DeepSeek', response);
        }

        const data = await readResponseJson(response);
        if (isObjectNotArray(data) && typeof data.error !== 'undefined') { throw this._providerError('DeepSeek', data.error); }
        const choice = this._getFirstChoice(data);
        const message = choice !== null && isObjectNotArray(choice.message) ? choice.message : null;
        const plain = this._stringifyMessageContent(message?.content ?? choice?.text ?? '');
        return plain.trim() || 'No explanation available.';
    }
}

/**
 * AI provider factory
 */
export const AIProviderFactory = {
    /**
     * Create an AI provider instance
     * @param {string} providerName The name of the provider ('openai', 'gemini', 'deepseek', or 'custom')
     * @param {import('settings').AIProviderConfig} config Provider configuration
     * @returns {AIProvider}
     * @throws {Error} If the provider name is unknown.
     */
    createProvider(providerName, config) {
        switch (providerName) {
            case 'openai':
                return new OpenAIProvider(config);
            case 'gemini':
                return new GeminiProvider(config);
            case 'deepseek':
                return new DeepSeekProvider(config);
            case 'custom':
                return new CustomProvider(config);
            default:
                throw new Error(`Unknown AI provider: ${providerName}`);
        }
    },

    /**
     * Get list of available providers
     * @returns {string[]}
     */
    getAvailableProviders() {
        return ['openai', 'gemini', 'deepseek', 'custom'];
    },

    /**
     * Get default models for each provider
     * @returns {{[key: string]: string[]}}
     */
    getDefaultModels() {
        return {
            openai: ['gpt-4o-mini', 'gpt-4o', 'gpt-3.5-turbo'],
            gemini: [
                'gemini-2.5-flash',
                'gemini-2.5-flash-lite',
                'gemini-2.5-flash-lite-preview-09-2025',
                'gemini-2.5-pro',
                'gemini-2.0-flash',
                'gemini-3.1-flash-lite-preview',
                'gemini-3.1-flash-lite-preview-vertex',
                'gemini-3.1-pro-preview',
                'gemini-3.1-pro-preview-vertex',
                'gemini-3-flash-preview-vertex',
                'gemini-3-flash-preview',
            ],
            deepseek: [],
            custom: [],
        };
    },
};
