// Default AI options: the `ai` object of yomitan-byok-ai's options schema, plus Lookitan's prompts
// for the word card and the context translator.

import {CARD_PROMPT, COMPONENTS_FIELD, COMPONENTS_PROMPT, CONTEXT_OCR_PROMPT, CONTEXT_PROMPT, EXPLAIN_PROMPT, EXPRESSIONS_PROMPT, FORM_FIELD, FORM_PROMPT, LEGACY_CARD_PROMPT, UNIT_CARD_PROMPT, CONTEXT_CARD_PROMPT, DISPLAY_CARD_PROMPT, LEGACY_CONTEXT_PROMPT, LEGACY_EXPLAIN_PROMPT, LEGACY_OCR_ERRORS_PROMPT, LEGACY_TRANSLATION_FIELD, LEGACY_TRANSLITERATION_PROMPT, LEGACY_WORD_TRANSLATION_PROMPT, OCR_ERRORS_PROMPT, SENTENCE_CONTEXT_PROMPT, SINGLE_WORD_TRANSLATION_PROMPT, READING_PROMPT, SYNONYMS_READING_AND_COMPONENTS, TRANSLATION_FIELD, TRANSLATION_LANGUAGE_EXPLAIN_PROMPT, TRANSLITERATION_PROMPT, WORD_TRANSLATION_PROMPT} from './word-card.js';
import {IMAGE_QUERY_PROMPT, IMAGE_QUERY_SYSTEM_PROMPT} from '../images.js';

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
export const SECTION_PROVIDERS = ['ai-context', 'ai-images'];

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

/** The AI options one dictionary's requests use: the shared prompts with its own provider fields. */
export function aiOptionsFor(ai, dictId) {
    const own = ai?.sectionProviders?.[dictId];
    return isObject(own) ? {...ai, ...own} : ai;
}

export function defaultAiOptions() {
    const options = baseAiOptions();
    options.sectionProviders = Object.fromEntries(SECTION_PROVIDERS.map((id) => [id, providerFields(options, options)]));
    return options;
}

function baseAiOptions() {
    return {
        provider: 'openai',
        customPrompt: DEFAULT_PROMPT,
        cardPrompt: CARD_PROMPT,
        wordTranslationPrompt: WORD_TRANSLATION_PROMPT,
        readingPrompt: READING_PROMPT,
        componentsPrompt: COMPONENTS_PROMPT,
        formPrompt: FORM_PROMPT,
        transliterationPrompt: TRANSLITERATION_PROMPT,
        ocrErrorsPrompt: OCR_ERRORS_PROMPT,
        expressionsPrompt: EXPRESSIONS_PROMPT,
        contextOcrPrompt: CONTEXT_OCR_PROMPT,
        contextPrompt: CONTEXT_PROMPT,
        // The "Imagens" dictionary: its own system and user prompts for the search query (as
        // Migaku's "Image query"), shown on click unless autoShowImages is on.
        imageQuerySystemPrompt: IMAGE_QUERY_SYSTEM_PROMPT,
        imageQueryPrompt: IMAGE_QUERY_PROMPT,
        imageQueryUseAi: true,
        autoShowImages: false,
        // Migaku's meme mode, switched in the pop-up's "Imagens" section: " meme" ends the search.
        imageMemeMode: false,
        systemPrompt: '',
        // The word card ("IA — palavra no contexto").
        autoGenerateOnLookup: true,
        // The card's "Componentes": the parts of the word, in any language. Off, the prompt does
        // not ask for them.
        cardComponents: true,
        cardOcrFix: true,
        contextOcrFix: true,
        // The pictures search the word the AI card corrected from the OCR.
        imageUseCardWord: true,
        // Bumped when markers are added to the card prompt, so a saved prompt gets them once.
        cardPromptVersion: 3,
        // The context translator has its own switch.
        autoTranslateContext: true,
        // Blurs the context translation until it is clicked, like hideWordTranslation.
        hideContextTranslation: false,
        autoExplain: false,
        streamResponse: true,
        cancelPendingRequests: true,
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
    for (const key of ['autoExplain', 'autoShowImages', 'imageQueryUseAi', 'imageMemeMode', 'autoGenerateOnLookup', 'hideContextTranslation', 'cardComponents', 'cardOcrFix', 'contextOcrFix', 'imageUseCardWord']) {
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
    out.sectionProviders = Object.fromEntries(SECTION_PROVIDERS.map((id) => [id, providerFields(isObject(own[id]) ? own[id] : out, base)]));
    for (const key of ['customPrompt', 'cardPrompt', 'contextPrompt', 'wordTranslationPrompt', 'readingPrompt', 'componentsPrompt', 'formPrompt', 'transliterationPrompt', 'ocrErrorsPrompt', 'expressionsPrompt', 'contextOcrPrompt', 'imageQuerySystemPrompt', 'imageQueryPrompt']) {
        // An empty prompt is the user's choice: its instructions are not sent (see `promptText`).
        // Only a missing or broken one gets the default; "Restaurar padrão" writes it back.
        if (typeof out[key] !== 'string') { out[key] = base[key]; }
    }
    // "Reconhecer expressões" (`cardExpressions`) was a switch until 2026-10-27; an empty
    // "Prompt das expressões" does its work now. Switched off, the prompt becomes empty, so the
    // note stays off; a prompt the user had written is kept in `expressionsPromptBackup`, never
    // sent, for the "Recuperar o meu prompt" button.
    if (stored.cardExpressions === false) {
        const own = out.expressionsPrompt.trim();
        if (own && own !== EXPRESSIONS_PROMPT) { out.expressionsPromptBackup = out.expressionsPrompt; }
        out.expressionsPrompt = '';
    }
    delete out.cardExpressions;
    if (typeof out.expressionsPromptBackup !== 'string' || !out.expressionsPromptBackup.trim()) { delete out.expressionsPromptBackup; }
    // The old default explanation and context prompts mixed the word into their instructions; the
    // new ones put the lookup's text last, so the instructions can be cached.
    if (out.customPrompt.trim() === LEGACY_EXPLAIN_PROMPT) { out.customPrompt = EXPLAIN_PROMPT; }
    // The default explanation prompt then wrote in the translation language; it has its own now.
    if (out.customPrompt.trim() === TRANSLATION_LANGUAGE_EXPLAIN_PROMPT) { out.customPrompt = EXPLAIN_PROMPT; }
    if (out.contextPrompt.trim() === LEGACY_CONTEXT_PROMPT) { out.contextPrompt = CONTEXT_PROMPT; }
    // The default context prompt then translated the sentence alone; it translates the paragraph
    // now, keeping the sentence's marks, so a prompt still on it moves to the new one.
    if (out.contextPrompt.trim() === SENTENCE_CONTEXT_PROMPT) { out.contextPrompt = CONTEXT_PROMPT; }
    // The first default transliteration prompt sometimes got the word back in its own script.
    if (out.transliterationPrompt.trim() === LEGACY_TRANSLITERATION_PROMPT) { out.transliterationPrompt = TRANSLITERATION_PROMPT; }
    // Card prompts saved before the translation had its own field: move the default translation
    // clause to the {{translationField}} marker and keep the rest of the user's edits.
    if (!out.cardPrompt.includes(TRANSLATION_FIELD) && out.cardPrompt.includes(LEGACY_TRANSLATION_FIELD)) {
        out.cardPrompt = out.cardPrompt.replace(LEGACY_TRANSLATION_FIELD, TRANSLATION_FIELD);
    }
    // Card prompts saved before the synonyms, reading and components: they go right after the
    // translation's marker, once, so a user who later takes them out keeps them out.
    const version = Number(stored.cardPromptVersion) || 1;
    if (version < 2 && !out.cardPrompt.includes(COMPONENTS_FIELD) && out.cardPrompt.includes(TRANSLATION_FIELD)) {
        out.cardPrompt = out.cardPrompt.replace(TRANSLATION_FIELD, TRANSLATION_FIELD + SYNONYMS_READING_AND_COMPONENTS);
    }
    // Then the form on screen (its grammatical features and transliteration), after the components.
    if (version < 3 && !out.cardPrompt.includes(FORM_FIELD) && out.cardPrompt.includes(COMPONENTS_FIELD)) {
        out.cardPrompt = out.cardPrompt.replace(COMPONENTS_FIELD, COMPONENTS_FIELD + FORM_FIELD);
    }
    out.cardPromptVersion = 3;
    // The defaults of 2026-10-27 describe the longer unit a word belongs to in every key, ask in the
    // translation for up to two other translations of the same meaning and correct the OCR only
    // when sure; a prompt still on an earlier default moves to it, an edited one stays as written.
    // The later default of that day, before the meaning in context came first (the intended one,
    // for an anomalous wording) and the synonyms followed that meaning, moves to it the same way.
    // So does the one of the same day before the headword's spelling and {{displayField}}; an
    // edited prompt gets no display form unless the user adds the marker or asks for the key. And
    // the one with them, before the definition explained the word rather than the context and the
    // synonyms were told to stay in the text's language.
    if ([LEGACY_CARD_PROMPT, UNIT_CARD_PROMPT, CONTEXT_CARD_PROMPT, DISPLAY_CARD_PROMPT].includes(out.cardPrompt.trim())) { out.cardPrompt = CARD_PROMPT; }
    if ([LEGACY_WORD_TRANSLATION_PROMPT, SINGLE_WORD_TRANSLATION_PROMPT].includes(out.wordTranslationPrompt.trim())) {
        out.wordTranslationPrompt = WORD_TRANSLATION_PROMPT;
    }
    if (out.ocrErrorsPrompt.trim() === LEGACY_OCR_ERRORS_PROMPT) { out.ocrErrorsPrompt = OCR_ERRORS_PROMPT; }
    // The Arabic-only OCR note of 2026-10-24 became the OCR errors note of every language.
    delete out.arabicOcrPrompt;
    // The pictures' own OCR note of 2026-10-24 gave way to the word the AI card corrected.
    delete out.imageOcrFix;
    delete out.imageOcrPrompt;
    return out;
}
