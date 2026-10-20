// Default AI options: the `ai` object of yomitan-byok-ai's options schema, plus Lookitan's prompts
// for the word card and the context translator.

import {CARD_PROMPT, CONTEXT_PROMPT, EXPLAIN_PROMPT, LEGACY_TRANSLATION_FIELD, TRANSLATION_FIELD, WORD_TRANSLATION_PROMPT} from './word-card.js';
import {IMAGE_QUERY_PROMPT, IMAGE_QUERY_SYSTEM_PROMPT} from '../images.js';
import {IMAGE_GEN_DEFAULT_MODELS, IMAGE_GEN_PROMPT, IMAGE_GEN_PROVIDERS, OPENROUTER_IMAGE_MODEL, imageGenCount, isOpenRouterEndpoint} from '../image-gen.js';

/** Default for the explanation prompt (the fork's `customPrompt`), used by "Explain". */
export const DEFAULT_PROMPT = EXPLAIN_PROMPT;

const PROVIDER_ACCOUNTS = ['openai', 'gemini', 'deepseek', 'custom'];

/**
 * What each AI dictionary chooses for itself: the provider and its accounts (key, model, thinking,
 * routing), temperature, streaming, cancelling and the system prompt. "IA — palavra no contexto"
 * keeps its own at the top of the AI options; the other AI dictionaries keep theirs in
 * `sectionProviders[<dictionary id>]`, so a change in one tab never changes another.
 */
export const PROVIDER_FIELDS = ['provider', ...PROVIDER_ACCOUNTS, 'temperature', 'streamResponse', 'cancelPendingRequests', 'systemPrompt'];

/** The AI dictionaries with a provider of their own besides the word card ("ai-word"). */
export const SECTION_PROVIDERS = ['ai-context', 'ai-images', 'ai-image-gen'];

const isObject = (value) => typeof value === 'object' && value !== null && !Array.isArray(value);

/** A dictionary's provider fields taken from `source`, with defaults for whatever is missing. */
function providerFields(source, base) {
    const out = {};
    for (const key of PROVIDER_FIELDS) {
        const value = source[key];
        if (PROVIDER_ACCOUNTS.includes(key)) { out[key] = {...base[key], ...(isObject(value) ? value : {})}; }
        else if (key === 'temperature') { out[key] = typeof value === 'number' && Number.isFinite(value) ? value : base[key]; }
        else if (key === 'provider' || key === 'systemPrompt') { out[key] = typeof value === 'string' ? value : base[key]; }
        else { out[key] = typeof value === 'boolean' ? value : base[key]; }
    }
    return out;
}

/**
 * "Imagens IA"'s provider block. Its first copy of the word card's keeps the keys and the endpoint
 * but takes image models (OpenRouter's endpoint gets OPENROUTER_IMAGE_MODEL), and leaves the text
 * model's thinking and provider routing behind: the providers that serve a text model rarely serve
 * an image one. A provider that cannot make pictures (DeepSeek) becomes OpenAI.
 */
function imageGenFields(source, base, copied) {
    const out = providerFields(source, base);
    if (copied) {
        for (const id of IMAGE_GEN_PROVIDERS) { out[id] = {...out[id], model: IMAGE_GEN_DEFAULT_MODELS[id]}; }
        out.custom = {...out.custom, thinkingMode: '', thinkingIntensity: '', customThinkingIntensity: '', providerRoutingMode: '', providerSlugs: '', providerAllowFallbacks: true,
            model: isOpenRouterEndpoint(out.custom.endpoint) ? OPENROUTER_IMAGE_MODEL : ''};
    }
    if (!IMAGE_GEN_PROVIDERS.includes(out.provider)) { out.provider = 'openai'; }
    return out;
}

/** A dictionary's own provider block: its stored one, or a copy of the word card's. */
function sectionFields(id, stored, card, base) {
    const own = isObject(stored) ? stored : null;
    return id === 'ai-image-gen' ? imageGenFields(own || card, base, !own) : providerFields(own || card, base);
}

/** The AI options one dictionary's requests use: the shared prompts with its own provider fields. */
export function aiOptionsFor(ai, dictId) {
    const own = ai?.sectionProviders?.[dictId];
    return isObject(own) ? {...ai, ...own} : ai;
}

export function defaultAiOptions() {
    const options = baseAiOptions();
    options.sectionProviders = Object.fromEntries(SECTION_PROVIDERS.map((id) => [id, sectionFields(id, null, options, options)]));
    return options;
}

function baseAiOptions() {
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
        // Migaku's meme mode, switched in the pop-up's "Imagens" section: " meme" ends the search.
        imageMemeMode: false,
        // "Imagens IA": pictures made by an image model from this prompt, on a click unless
        // autoGenerateImages is on; imageGenCount per click.
        imageGenPrompt: IMAGE_GEN_PROMPT,
        imageGenCount: 1,
        imageGenAspect: '1:1',
        imageGenQuality: 'low',
        autoGenerateImages: false,
        systemPrompt: '',
        // The word card ("IA — palavra no contexto").
        autoGenerateOnLookup: true,
        // The context translator has its own switch.
        autoTranslateContext: true,
        // Blurs the context translation until it is clicked, like hideWordTranslation.
        hideContextTranslation: false,
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
    const base = baseAiOptions();
    if (typeof stored !== 'object' || stored === null || Array.isArray(stored)) {
        return defaultAiOptions();
    }
    const out = {...base, ...stored};
    for (const key of ['autoExplain', 'autoShowImages', 'imageQueryUseAi', 'imageMemeMode', 'autoGenerateOnLookup', 'hideContextTranslation', 'autoGenerateImages']) {
        if (typeof out[key] !== 'boolean') { out[key] = base[key]; }
    }
    // One switch used to start both the word card and the context translation: until the
    // translator's own switch is set, it keeps what that switch said.
    if (typeof stored.autoTranslateContext !== 'boolean') { out.autoTranslateContext = out.autoGenerateOnLookup; }
    for (const key of PROVIDER_ACCOUNTS) {
        out[key] = {...base[key], ...(isObject(stored[key]) ? stored[key] : {})};
    }
    // The context translator and the pictures had no provider of their own before: until they get
    // one, they start from a copy of the word card's, so they keep working as they did.
    const own = isObject(stored.sectionProviders) ? stored.sectionProviders : {};
    out.sectionProviders = Object.fromEntries(SECTION_PROVIDERS.map((id) => [id, sectionFields(id, own[id], out, base)]));
    out.imageGenCount = imageGenCount(out.imageGenCount);
    for (const key of ['imageGenAspect', 'imageGenQuality']) {
        if (typeof out[key] !== 'string' || !out[key]) { out[key] = base[key]; }
    }
    for (const key of ['customPrompt', 'cardPrompt', 'contextPrompt', 'wordTranslationPrompt', 'imageQuerySystemPrompt', 'imageQueryPrompt', 'imageGenPrompt']) {
        if (typeof out[key] !== 'string' || out[key].trim() === '') { out[key] = base[key]; }
    }
    // Card prompts saved before the translation had its own field: move the default translation
    // clause to the {{translationField}} marker and keep the rest of the user's edits.
    if (!out.cardPrompt.includes(TRANSLATION_FIELD) && out.cardPrompt.includes(LEGACY_TRANSLATION_FIELD)) {
        out.cardPrompt = out.cardPrompt.replace(LEGACY_TRANSLATION_FIELD, TRANSLATION_FIELD);
    }
    return out;
}
