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
     * The prompts as sent, laid out for the providers' prefix caches (DeepSeek, OpenAI, Gemini,
     * OpenRouter): what is the same for every lookup comes first, the lookup's own text last. A
     * line holding {{target}} or {{sentence}} is the lookup's: it moves to the end of the user
     * message, the lines with the sentence before those with only the word, since several words are
     * often looked up in one paragraph. Every other line, of the system prompt and then of the
     * prompt, makes the system message. A line repeated in both is sent once. A prompt without
     * either variable is sent as written.
     * @param {string} term
     * @param {string} sentence
     * @param {string} customPrompt
     * @param {string|AbortSignal} [systemPromptOrAbort]
     * @param {AbortSignal} [abortSignal]
     * @returns {{prompt: string, systemPrompt: string, signal: (AbortSignal|undefined)}}
     */
    _splitPrompts(term, sentence, customPrompt, systemPromptOrAbort, abortSignal) {
        const {systemPrompt, signal} = this._resolvePromptArguments(term, sentence, systemPromptOrAbort, abortSignal);
        const template = customPrompt || '';
        if (!template.includes('{{target}}') && !template.includes('{{sentence}}')) {
            return {prompt: this._replacePlaceholders(template, term, sentence), systemPrompt, signal};
        }
        const system = typeof systemPromptOrAbort === 'string' ? systemPromptOrAbort : '';
        const fixed = [];
        const withSentence = [];
        const withTarget = [];
        for (const text of [system, template]) {
            const kept = [];
            for (const line of text.split('\n')) {
                if (line.includes('{{sentence}}')) {
                    withSentence.push(this._replacePlaceholders(line, term, sentence).trim());
                } else if (line.includes('{{target}}')) {
                    withTarget.push(this._replacePlaceholders(line, term, sentence).trim());
                } else {
                    kept.push(line);
                }
            }
            const block = kept.join('\n').replace(/\n{3,}/g, '\n\n').trim();
            if (block) { fixed.push(block); }
        }
        const data = [...new Set([...withSentence, ...withTarget])].filter((line) => line !== '');
        return {prompt: data.join('\n'), systemPrompt: fixed.join('\n\n'), signal};
    }

    /**
     * Logs how much of the prompt the provider read from its cache, when the reply says so
     * (DeepSeek, OpenAI and OpenRouter usage, Gemini usage metadata).
     * @param {unknown} data
     */
    _logCacheUsage(data) {
        if (!isObjectNotArray(data)) { return; }
        const usage = isObjectNotArray(data.usage) ? data.usage : null;
        const meta = isObjectNotArray(data.usageMetadata) ? data.usageMetadata : null;
        const details = usage !== null && isObjectNotArray(usage.prompt_tokens_details) ? usage.prompt_tokens_details : null;
        const prompt = usage?.prompt_tokens ?? meta?.promptTokenCount;
        const cached = usage?.prompt_cache_hit_tokens ?? details?.cached_tokens ?? meta?.cachedContentTokenCount ?? 0;
        if (typeof prompt === 'number') {
            console.info(`AI prompt cache: ${cached} of ${prompt} prompt tokens`);
        }
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
            this._logCacheUsage(parsed);
            const delta = this._extractOpenAIStreamDelta(parsed);
            if (delta) {
                full += delta;
                if (typeof onChunk === 'function') { onChunk(delta, full); }
            }
        }, signal);
        return full;
    }
}

/** The Anthropic Messages API version sent in `anthropic-version`. */
const ANTHROPIC_VERSION = '2023-06-01';
/** Room for the reply (and the thinking before it) in an Anthropic request, which requires it. */
const ANTHROPIC_MAX_TOKENS = 16000;
/** The effort levels the Anthropic API takes in `output_config.effort`. */
const ANTHROPIC_EFFORTS = new Set(['low', 'medium', 'high', 'xhigh', 'max']);

/**
 * The one provider since 2026-11-24 (the user had OpenAI, Gemini and DeepSeek removed): any
 * endpoint the user gives, in one of two formats (`apiFormat`): OpenAI's chat completions
 * (OpenRouter, OpenAI, DeepSeek, Gemini's OpenAI-compatible endpoint, Ollama, LM Studio…) or
 * Anthropic's Messages API (api.anthropic.com, or a gateway that speaks it). Left empty, the
 * format follows the endpoint: a path ending in "/messages" is Anthropic's.
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
     * The request format: "anthropic" for the Messages API, else "openai" (chat completions).
     * @returns {'openai'|'anthropic'}
     */
    apiFormat() {
        const chosen = this._config?.apiFormat;
        if (chosen === 'openai' || chosen === 'anthropic') { return chosen; }
        try {
            return /\/messages\/?$/.test(new URL(this._config?.endpoint || '').pathname) ? 'anthropic' : 'openai';
        } catch (_error) {
            return 'openai';
        }
    }

    /**
     * An Anthropic Messages request: the system prompt as a block marked for the prompt cache
     * (the part every lookup shares), the lookup as the user message, the reply's room
     * (`max_tokens`, required) and the thinking settings. No temperature: the current Claude
     * models reject sampling settings.
     * @param {string} term
     * @param {string} sentence
     * @param {string} customPrompt
     * @param {string|AbortSignal} [systemPromptOrAbort]
     * @param {AbortSignal} [abortSignal]
     * @returns {{url: string, headers: Record<string, string>, requestBody: Record<string, unknown>, signal: (AbortSignal|undefined)}}
     */
    _buildAnthropicRequest(term, sentence, customPrompt, systemPromptOrAbort, abortSignal) {
        const {prompt, systemPrompt, signal} = this._splitPrompts(term, sentence, customPrompt, systemPromptOrAbort, abortSignal);
        const requestBody = {
            model: String(this._config.model || '').trim(),
            max_tokens: ANTHROPIC_MAX_TOKENS,
            messages: [{role: 'user', content: prompt}],
        };
        if (systemPrompt) {
            requestBody.system = [{type: 'text', text: systemPrompt, cache_control: {type: 'ephemeral'}}];
        }
        this._applyAnthropicThinking(requestBody);
        this._applyCustomRequestBodyJson(requestBody);
        requestBody.stream = false;
        const key = this._config.apiKey.trim();
        let anthropicHost = false;
        try { anthropicHost = /(^|\.)anthropic\.com$/.test(new URL(this._config.endpoint).hostname); } catch (_error) { /* checked by isConfigured */ }
        return {
            url: this._config.endpoint,
            headers: {
                'Content-Type': 'application/json',
                'x-api-key': key,
                'anthropic-version': ANTHROPIC_VERSION,
                // Gateways that speak the Messages API (OpenRouter) read the key as a bearer token;
                // Anthropic itself reads a bearer as an OAuth token, so it gets only x-api-key.
                ...(anthropicHost ? {} : {'Authorization': `Bearer ${key}`}),
            },
            requestBody,
            signal,
        };
    }

    /**
     * The thinking settings in the Messages API: "Ligado" asks for adaptive thinking, the effort
     * goes to `output_config.effort` ("minimal" and "none" become "low"; "Personalizada" takes
     * an effort name or JSON for `output_config`). "Desligado" sends nothing: the newest models
     * think adaptively and reject turning it off, so the effort is what lowers it.
     * @param {Record<string, unknown>} requestBody
     */
    _applyAnthropicThinking(requestBody) {
        const mode = this._config?.thinkingMode || '';
        const intensity = this._config?.thinkingIntensity || '';
        if (mode === 'enabled') { requestBody.thinking = {type: 'adaptive'}; }
        if (intensity === 'custom') {
            const value = typeof this._config?.customThinkingIntensity === 'string' ? this._config.customThinkingIntensity.trim() : '';
            if (value.startsWith('{')) {
                try {
                    const parsed = parseJson(value);
                    if (isObjectNotArray(parsed)) { requestBody.output_config = parsed; }
                } catch (_error) { /* an invalid value sends nothing */ }
            } else if (ANTHROPIC_EFFORTS.has(value)) {
                requestBody.output_config = {effort: value};
            }
            return;
        }
        const effort = intensity === 'minimal' || intensity === 'none' ? 'low' : intensity;
        if (ANTHROPIC_EFFORTS.has(effort)) { requestBody.output_config = {effort}; }
    }

    /**
     * The text of a Messages API reply: its text blocks, without thinking. A refusal is an error.
     * @param {unknown} data
     * @returns {string}
     */
    _anthropicText(data) {
        if (!isObjectNotArray(data)) { return ''; }
        if (data.type === 'error' || typeof data.error !== 'undefined') { throw this._providerError('Anthropic', data.error ?? data); }
        if (data.stop_reason === 'refusal') { throw new Error('Anthropic API: the model declined this request (refusal).'); }
        const blocks = Array.isArray(data.content) ? data.content : [];
        return blocks.filter((b) => isObjectNotArray(b) && b.type === 'text' && typeof b.text === 'string').map((b) => b.text).join('');
    }

    /**
     * Streams a Messages API reply: text comes in `content_block_delta` events with a
     * `text_delta`; thinking deltas are skipped; an `error` event or a refusal fails the request.
     * @param {{url: string, headers: Record<string, string>, requestBody: Record<string, unknown>, signal: (AbortSignal|undefined)}} request
     * @param {(chunk: string, full: string) => void} [onChunk]
     * @returns {Promise<string>}
     */
    async _streamAnthropic(request, onChunk) {
        const response = await fetch(request.url, {
            method: 'POST',
            headers: request.headers,
            signal: request.signal,
            body: JSON.stringify(request.requestBody),
        });
        if (!response.ok) { throw await this._responseError('Anthropic', response); }
        let full = '';
        await this._readSseStream(response, (payload) => {
            /** @type {unknown} */
            let event;
            try { event = parseJson(payload); } catch (_error) { return; }
            if (!isObjectNotArray(event)) { return; }
            if (event.type === 'error') { throw this._providerError('Anthropic stream', event.error ?? event); }
            if (event.type === 'message_delta' && isObjectNotArray(event.delta) && event.delta.stop_reason === 'refusal') {
                throw new Error('Anthropic API: the model declined this request (refusal).');
            }
            if (event.type === 'message_start' && isObjectNotArray(event.message)) { this._logAnthropicCache(event.message.usage); }
            if (event.type === 'message_stop') { return false; }
            if (event.type === 'content_block_delta' && isObjectNotArray(event.delta) && event.delta.type === 'text_delta' && typeof event.delta.text === 'string') {
                full += event.delta.text;
                if (typeof onChunk === 'function') { onChunk(event.delta.text, full); }
            }
        }, request.signal);
        return full;
    }

    /** Logs the Messages API's cache use. @param {unknown} usage */
    _logAnthropicCache(usage) {
        if (!isObjectNotArray(usage)) { return; }
        const read = Number(usage.cache_read_input_tokens) || 0;
        const total = read + (Number(usage.cache_creation_input_tokens) || 0) + (Number(usage.input_tokens) || 0);
        if (total > 0) { console.info(`AI prompt cache: ${read} of ${total} prompt tokens`); }
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

        const {prompt, systemPrompt, signal} = this._splitPrompts(term, sentence, customPrompt, systemPromptOrAbort, abortSignal);

        // Build OpenAI v1-style request body
        const messages = systemPrompt ?
[
    {role: 'system', content: systemPrompt},
    {role: 'user', content: prompt},
] :
[
    {role: 'user', content: prompt},
];

        // Through OpenRouter, Claude and Gemini read their cache only up to a marked breakpoint:
        // the end of the system message, the part every lookup shares.
        if (systemPrompt && this._isOpenRouterEndpoint() && /^(anthropic|google)\//.test(String(this._config.model || '').trim())) {
            messages[0] = {role: 'system', content: [{type: 'text', text: systemPrompt, cache_control: {type: 'ephemeral'}}]};
        }

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
        if (this.apiFormat() === 'anthropic') {
            if (!this.isConfigured()) { throw new Error('Custom provider is not configured. Please set endpoint, API key, and model.'); }
            const request = this._buildAnthropicRequest(term, sentence, customPrompt, systemPrompt, abortSignal);
            request.requestBody.stream = true;
            const text = await this._streamAnthropic(request, onChunk);
            return text.trim() || 'No explanation available.';
        }
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
        if (this.apiFormat() === 'anthropic') {
            if (!this.isConfigured()) { throw new Error('Custom provider is not configured. Please set endpoint, API key, and model.'); }
            const request = this._buildAnthropicRequest(term, sentence, customPrompt, systemPromptOrAbort, abortSignal);
            const response = await fetch(request.url, {method: 'POST', headers: request.headers, signal: request.signal, body: JSON.stringify(request.requestBody)});
            if (!response.ok) { throw await this._responseError('Anthropic', response); }
            const data = await readResponseJson(response);
            if (isObjectNotArray(data)) { this._logAnthropicCache(data.usage); }
            return this._anthropicText(data).trim() || 'No explanation available.';
        }
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
        this._logCacheUsage(data);
        const choice = this._getFirstChoice(data);
        const responseError = isObjectNotArray(data) ? data.error ?? choice?.error : undefined;
        if (typeof responseError !== 'undefined') { throw this._providerError('Custom', responseError); }
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
     * The provider for a dictionary. Only the custom one is left (2026-11-24); the name is kept
     * for the settings, and an old name gets it too.
     * @param {string} _providerName
     * @param {import('settings').AIProviderConfig} config Provider configuration
     * @returns {AIProvider}
     */
    createProvider(_providerName, config) {
        return new CustomProvider(config);
    },

    /** @returns {string[]} */
    getAvailableProviders() {
        return ['custom'];
    },

    /** @returns {{[key: string]: string[]}} */
    getDefaultModels() {
        return {custom: []};
    },
};
