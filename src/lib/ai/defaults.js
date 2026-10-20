// Default AI options: the `ai` object of yomitan-byok-ai's options schema, plus Lookitan's prompts
// for the word card and the context translator.

import {CARD_PROMPT, CONTEXT_PROMPT, EXPLAIN_PROMPT, LEGACY_TRANSLATION_FIELD, TRANSLATION_FIELD, WORD_TRANSLATION_PROMPT} from './word-card.js';
import {IMAGE_QUERY_PROMPT, IMAGE_QUERY_SYSTEM_PROMPT} from '../images.js';

/** Default for the explanation prompt (the fork's `customPrompt`), used by "Explain". */
export const DEFAULT_PROMPT = EXPLAIN_PROMPT;

export function defaultAiOptions() {
    return {
        provider: 'openai',
        customPrompt: DEFAULT_PROMPT,
        cardPrompt: CARD_PROMPT,
        wordTranslationPrompt: WORD_TRANSLATION_PROMPT,
        contextPrompt: CONTEXT_PROMPT,
        // The "Imagens" dictionary: its own system and user prompts for the search query (as
        // Migaku's "Image query"), shown on click unless autoShowImages is on.
        imageQuerySystemPrompt: IMAGE_QUERY_SYSTEM_PROMPT,
        imageQueryPrompt: IMAGE_QUERY_PROMPT,
        imageQueryUseAi: true,
        autoShowImages: false,
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
    for (const key of ['autoExplain', 'autoShowImages', 'imageQueryUseAi']) {
        if (typeof out[key] !== 'boolean') { out[key] = base[key]; }
    }
    for (const key of ['openai', 'gemini', 'deepseek', 'custom']) {
        out[key] = {...base[key], ...(typeof stored[key] === 'object' && stored[key] !== null ? stored[key] : {})};
    }
    for (const key of ['customPrompt', 'cardPrompt', 'contextPrompt', 'wordTranslationPrompt', 'imageQuerySystemPrompt', 'imageQueryPrompt']) {
        if (typeof out[key] !== 'string' || out[key].trim() === '') { out[key] = base[key]; }
    }
    // Card prompts saved before the translation had its own field: move the default translation
    // clause to the {{translationField}} marker and keep the rest of the user's edits.
    if (!out.cardPrompt.includes(TRANSLATION_FIELD) && out.cardPrompt.includes(LEGACY_TRANSLATION_FIELD)) {
        out.cardPrompt = out.cardPrompt.replace(LEGACY_TRANSLATION_FIELD, TRANSLATION_FIELD);
    }
    return out;
}
