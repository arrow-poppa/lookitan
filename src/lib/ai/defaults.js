// Default AI options, matching the `ai` object of yomitan-byok-ai's options schema.

export const DEFAULT_PROMPT =
    "Explain the meaning of '{{target}}' in the following sentence: '{{sentence}}'. " +
    "Provide a concise explanation focusing on the word's usage and meaning in this specific context.";

export function defaultAiOptions() {
    return {
        provider: 'openai',
        customPrompt: DEFAULT_PROMPT,
        systemPrompt: '',
        autoGenerateOnLookup: true,
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
    for (const key of ['openai', 'gemini', 'deepseek', 'custom']) {
        out[key] = {...base[key], ...(typeof stored[key] === 'object' && stored[key] !== null ? stored[key] : {})};
    }
    return out;
}
