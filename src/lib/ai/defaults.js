// Default AI options: the `ai` object of yomitan-byok-ai's options schema, plus Lookitan's prompts
// for the word card and the context translator.

import {CARD_PROMPT, CONTEXT_PROMPT, EXPLAIN_PROMPT} from './word-card.js';

/** Default for the explanation prompt (the fork's `customPrompt`), used by "Explain". */
export const DEFAULT_PROMPT = EXPLAIN_PROMPT;

export function defaultAiOptions() {
    return {
        provider: 'openai',
        customPrompt: DEFAULT_PROMPT,
        cardPrompt: CARD_PROMPT,
        contextPrompt: CONTEXT_PROMPT,
        systemPrompt: '',
        autoGenerateOnLookup: true,
        autoExplain: false,
        streamResponse: true,
        cancelPendingRequests: true,
        unknownWordFallback: true,
        temperature: 0.7,
        openai: {apiKey: '', model: 'gpt-4o-mini'},
        gemini: {apiKey: '', model: 'gemini-2.5-flash', thinkingLevel: ''},
        deepseek: {apiKey: '', model: '', thinkingMode: '', thinkingIntensity: ''},
        custom: {
            apiKey: '',
            model: '',
            endpoint: '',
            providerRoutingMode: '',
            providerSlugs: '',
            providerAllowFallbacks: true,
            thinkingMode: '',
            thinkingIntensity: '',
            customThinkingIntensity: '',
            requestBodyJson: '',
        },
    };
}

/**
 * Fills missing keys of stored options with defaults (stored values win).
 * @param {unknown} stored
 */
export function normalizeAiOptions(stored) {
    const base = defaultAiOptions();
    if (typeof stored !== 'object' || stored === null || Array.isArray(stored)) {
        return base;
    }
    const out = {...base, ...stored};
    if (typeof out.autoExplain !== 'boolean') { out.autoExplain = base.autoExplain; }
    for (const key of ['openai', 'gemini', 'deepseek', 'custom']) {
        out[key] = {...base[key], ...(typeof stored[key] === 'object' && stored[key] !== null ? stored[key] : {})};
    }
    for (const key of ['customPrompt', 'cardPrompt', 'contextPrompt']) {
        if (typeof out[key] !== 'string' || out[key].trim() === '') { out[key] = base[key]; }
    }
    return out;
}
