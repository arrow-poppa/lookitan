// Runs one AI explanation with the user's provider, streaming when enabled. Mirrors the request
// flow of yomitan-byok-ai's AIExplanationGenerator: stream first, retry once without streaming
// when nothing arrived.

import {AIProviderFactory} from './ai-provider.js';

const REQUEST_TIMEOUT_MS = 60000;

/**
 * @param {ReturnType<import('./defaults.js').defaultAiOptions>} ai
 */
export function isConfigured(ai) {
    try {
        return createProvider(ai).isConfigured();
    } catch (_e) {
        return false;
    }
}

function createProvider(ai) {
    const providerConfig = {
        ...(ai[ai.provider] || {apiKey: '', model: ''}),
        temperature: typeof ai.temperature === 'number' ? ai.temperature : 0.7,
    };
    return AIProviderFactory.createProvider(ai.provider, providerConfig);
}

/**
 * @param {object} ai AI options
 * @param {string} term
 * @param {string} sentence
 * @param {{signal: AbortSignal, prompt?: string, onChunk?: (delta: string, full: string) => void}} options
 * @returns {Promise<string>}
 */
export async function explain(ai, term, sentence, {signal, prompt, onChunk} = {}) {
    if (signal?.aborted) { throw signal.reason || new DOMException('Request cancelled', 'AbortError'); }
    const provider = createProvider(ai);
    if (!provider.isConfigured()) {
        throw new Error('Configure an AI provider in Settings › AI.');
    }
    const template = prompt || ai.customPrompt || '';
    const system = typeof ai.systemPrompt === 'string' ? ai.systemPrompt : '';
    const timeout = new AbortController();
    const timer = setTimeout(() => timeout.abort(new DOMException('AI request timed out', 'TimeoutError')), REQUEST_TIMEOUT_MS);
    const abort = () => timeout.abort(signal.reason);
    signal?.addEventListener('abort', abort, {once: true});
    try {
        if (ai.streamResponse === true && provider.supportsStreaming()) {
            let received = false;
            try {
                const text = await provider.generateExplanationStream(term, sentence, template, system, timeout.signal, (delta, full) => {
                    timeout.signal.throwIfAborted();
                    received = true;
                    onChunk?.(delta, full);
                });
                timeout.signal.throwIfAborted();
                if (received) { return text; }
            } catch (error) {
                if (received || timeout.signal.aborted || [401, 403, 429].includes(error?.status)) {
                    throw error;
                }
            }
        }
        const text = await provider.generateExplanation(term, sentence, template, system, timeout.signal);
        timeout.signal.throwIfAborted();
        onChunk?.(text, text);
        return text;
    } finally {
        clearTimeout(timer);
        signal?.removeEventListener('abort', abort);
    }
}

export const PROVIDERS = AIProviderFactory.getAvailableProviders();
export const DEFAULT_MODELS = AIProviderFactory.getDefaultModels();
