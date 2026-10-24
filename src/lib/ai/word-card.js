// Prompts and parsing for the two built-in AI "dictionaries": the word card (word in context, with
// IPA, CEFR level, part of speech, the grammatical features of the form on screen, definition,
// translation and synonyms; for Chinese and Japanese the reading, in other scripts than Latin the
// transliteration, and, when switched on, the word's components) and the context translator.

/** Default instruction for the card's "translation" key (the "Prompt da tradução da palavra"). */
export const WORD_TRANSLATION_PROMPT = 'the translation of the word in this context into {{translationLanguage}}';

/** Where the card prompt receives the translation key; empty when the word translation is off. */
export const TRANSLATION_FIELD = '{{translationField}}';

/** Where the card prompt receives the reading (pinyin, kana), for Chinese and Japanese only. */
export const READING_FIELD = '{{readingField}}';

/** Where the card prompt receives the components; empty when they are switched off. */
export const COMPONENTS_FIELD = '{{componentsField}}';

/** Where the card prompt receives the form on screen (its grammatical features and, in a script
 * other than Latin, its transliteration). */
export const FORM_FIELD = '{{formField}}';

/** The synonyms key of the card prompt, for every language. */
export const SYNONYMS_KEY = ', "synonyms": [up to three synonyms of the word in this sense, in the same language, or []]';

/** What was added to the card prompt after the translation's marker, for migration. */
export const SYNONYMS_READING_AND_COMPONENTS = SYNONYMS_KEY + READING_FIELD + COMPONENTS_FIELD;

/** The translation clause of the card prompt before it had its own field (for migration). */
export const LEGACY_TRANSLATION_FIELD = `, "translation": ${WORD_TRANSLATION_PROMPT}`;

export const CARD_PROMPT =
    "Word: '{{target}}'\n" +
    "Context: '{{sentence}}'\n\n" +
    'Describe the word exactly as it is used in this context. Reply with JSON only (no markdown, no code fence) with these keys:\n' +
    '{"headword": the dictionary form of the word, ' +
    '"ipa": IPA transcription without slashes, ' +
    '"cefr": CEFR level (A1, A2, B1, B2, C1 or C2) or "", ' +
    '"partOfSpeech": ["noun", ...], ' +
    '"labels": [usage, register or domain labels, e.g. "informal", "programming"], ' +
    '"definition": a one or two sentence definition in {{definitionLanguage}}' +
    TRANSLATION_FIELD + SYNONYMS_READING_AND_COMPONENTS + FORM_FIELD + '}';

/** Default instruction for the card's "reading" key (the "Prompt da leitura"). */
export const READING_PROMPT = 'the reading of the headword in {{readingSystem}}';

/** Default instruction for the card's "components" key (the "Prompt dos componentes"). */
export const COMPONENTS_PROMPT =
    '[the parts the word is built from, in order: {{componentParts}}; [] for a word of one part. ' +
    'Each is {"part": the part as written, {{componentReading}}"meaning": its short meaning in {{definitionLanguage}}}]';

/** Default instruction for the card's "form" key (the "Prompt da forma"). */
export const FORM_PROMPT =
    '[the grammatical features of the word as written in the context, in {{definitionLanguage}}, e.g. person, number, gender, ' +
    'tense, mood, case or definiteness; [] when it is the dictionary form]';

/** Default instruction for the card's "transliteration" key (the "Prompt da transliteração"). */
export const TRANSLITERATION_PROMPT = 'the word exactly as written in the context, transliterated into Latin letters';

/** Languages written in a script other than Latin, whose card gets a transliteration. Chinese and
 * Japanese have their reading instead. */
const TRANSLITERATED = new Set(['ar', 'fa', 'ur', 'ps', 'he', 'yi', 'ru', 'uk', 'be', 'bg', 'mk', 'sr', 'kk', 'ky', 'mn', 'tg',
    'el', 'ko', 'th', 'lo', 'km', 'my', 'hi', 'mr', 'ne', 'bn', 'pa', 'gu', 'ta', 'te', 'kn', 'ml', 'si', 'ka', 'hy', 'am']);

/** Default note that ends the card prompt (the "Prompt de erros do OCR"): the word and its context
 * come from OCR, which misreads characters in ways typical of each script ({{ocrErrors}}), so the
 * card describes the word the page meant and says what it corrected in "ocrFix". */
export const OCR_ERRORS_PROMPT =
    'The word and the context were read from the screen by OCR, which may misread characters: {{ocrErrors}}. ' +
    'If the word as given is not a real word or does not fit the context, use the context to find the word the text most likely has, ' +
    'describe that word (its dictionary form as the headword, and its form as it is in the text), ' +
    'and add the key "ocrFix": "<the word as given> → <the word you described>". ' +
    'Do not correct a word that fits the context, such as a rare word, slang or a name; then leave "ocrFix" out.';

/** {{ocrErrors}}: how OCR misreads each script. */
const ARABIC_ERRORS = 'in Arabic script it often drops letters, such as a prefix (ال، ل، ب، ك، و، ف) or a small letter inside the word ' +
    '(e.g. "تابة" in "واجهة بسيطة تابة النص العربي" is "لكتابة")';
const CYRILLIC_ERRORS = 'it confuses similar Cyrillic letters, or Cyrillic and Latin ones (e.g. и/н, ш/щ, е/ё, р/p), and may drop or add a letter';
const OCR_ERRORS = {
    ar: ARABIC_ERRORS, fa: ARABIC_ERRORS, ur: ARABIC_ERRORS, ps: ARABIC_ERRORS, ug: ARABIC_ERRORS, sd: ARABIC_ERRORS,
    zh: 'it confuses similar characters (e.g. 進/盡, 己/已, 未/末, 人/入), mixes simplified and traditional forms, ' +
        'and may cut the word in the wrong place (e.g. 進 alone where the text has 更進)',
    ja: 'it confuses similar characters (e.g. 己/已, 未/末, シ/ツ, ソ/ン, カ/力, ロ/口, ー/一) and may cut the word in the wrong place',
    ko: 'it confuses similar syllables (e.g. 을/올, 는/눈, 이/어) and may drop or misplace spaces',
    ru: CYRILLIC_ERRORS, uk: CYRILLIC_ERRORS, be: CYRILLIC_ERRORS, bg: CYRILLIC_ERRORS, mk: CYRILLIC_ERRORS, sr: CYRILLIC_ERRORS, kk: CYRILLIC_ERRORS,
    el: 'it confuses Greek letters with similar Latin ones (e.g. ν/v, ρ/p, η/n) and drops accents',
};
/** For a script other than Latin without examples of its own. */
const OTHER_SCRIPT_ERRORS = 'it confuses similar letters and drops or misplaces marks';
/** For the Latin script. */
const LATIN_ERRORS = 'it confuses similar letters (e.g. rn/m, cl/d, l/I/1, O/0) and drops or changes accents (e.g. "nao" for "não")';

/** {{readingSystem}}: the languages with a reading, and how it is written. */
const READING_SYSTEMS = {
    zh: 'pinyin with tone marks, e.g. "qīxǔ"',
    ja: 'hiragana',
};

/** {{componentParts}}: what the parts of a word are in each language. */
const COMPONENT_PARTS = {
    zh: 'each character',
    ja: 'each character',
    ko: 'each syllable for a word of Chinese origin (Sino-Korean), otherwise its stem and endings',
};
const OTHER_PARTS = 'its roots, affixes or the words of a compound';

/** {{componentReading}}: the key each part gets besides its meaning in each language. */
const COMPONENT_READINGS = {
    zh: '"reading": its pinyin reading, ',
    ja: '"reading": its kana reading, ',
    ko: '"hanja": the hanja of a Sino-Korean syllable, otherwise "", ',
};

/** A user's prompt, or the default when it is empty. */
function promptOr(value, fallback) {
    return typeof value === 'string' && value.trim() !== '' ? value.trim() : fallback;
}

/** The base language of a BCP 47 tag ("zh-Hant-TW" -> "zh"). */
function baseLanguage(tag) {
    return String(tag || '').split('-')[0].toLowerCase();
}

// The instructions come first and the lookup's text last, each variable on a line of its own: the
// provider then sends the instructions as the system message, the same for every lookup, which
// its prompt cache reuses (see `_splitPrompts` in ai-provider.js).
export const CONTEXT_PROMPT =
    'Translate the text into {{translationLanguage}}. ' +
    'Wrap the translation of the word in double asterisks, like **this**. ' +
    'Reply with the translation only.\n\n' +
    'Text: {{sentence}}\n' +
    "Word: '{{target}}'";

export const EXPLAIN_PROMPT =
    'Explain the word as it is used in the context. ' +
    'Write in {{explanationLanguage}}. Cover the meaning here, nuance and register, and give two short example sentences.\n\n' +
    "Context: '{{sentence}}'\n" +
    "Word: '{{target}}'";

/** The default explanation prompt before the explanation had its own language (for migration). */
export const TRANSLATION_LANGUAGE_EXPLAIN_PROMPT = EXPLAIN_PROMPT.replace('{{explanationLanguage}}', '{{translationLanguage}}');

/** The default prompts before they put the lookup's text last (for migration). */
export const LEGACY_CONTEXT_PROMPT =
    'Translate the text below into {{translationLanguage}}. ' +
    "Wrap the translation of '{{target}}' in double asterisks, like **this**. " +
    'Reply with the translation only.\n\n{{sentence}}';
export const LEGACY_EXPLAIN_PROMPT =
    "Explain '{{target}}' as it is used in: '{{sentence}}'. " +
    'Write in {{translationLanguage}}. Cover the meaning here, nuance and register, and give two short example sentences.';

/** English name of a BCP 47 tag, for prompts ("pt-BR" -> "Portuguese (Brazil)"). */
export function languageName(tag) {
    try {
        return new Intl.DisplayNames(['en'], {type: 'language'}).of(tag) || tag;
    } catch (_e) {
        return tag;
    }
}

/** Fills the language placeholders; {{target}} and {{sentence}} are filled by the provider. */
export function fillLanguages(template, settings) {
    return template
        .replaceAll('{{definitionLanguage}}', languageName(settings.definitionLanguage || 'en-US'))
        .replaceAll('{{translationLanguage}}', languageName(settings.translationLanguage || 'en-US'))
        .replaceAll('{{explanationLanguage}}', languageName(settings.explanationLanguage || settings.translationLanguage || 'en-US'));
}

/**
 * The word-card prompt as sent. {{translationField}} becomes the "translation" key with the word
 * translation prompt, or nothing when the word translation is turned off, so the model spends no
 * tokens on it. Likewise {{readingField}} gets the reading prompt only in Chinese and Japanese,
 * {{componentsField}} the components prompt only when they are switched on, and {{formField}} the
 * form prompt, plus the transliteration prompt in a script other than Latin; their variables follow
 * the OCR language. The OCR errors note follows a prompt with these markers, unless
 * "Corrigir erros de OCR com IA" (`cardOcrFix`) is off, on a paragraph of its own, with the examples of the OCR language's script. A card prompt without a marker is sent as
 * written.
 * @param {string} template
 * @param {{showWordTranslation?: boolean, ocrLanguage?: string}} settings
 * @param {{wordTranslationPrompt?: string, readingPrompt?: string, componentsPrompt?: string, formPrompt?: string, transliterationPrompt?: string, ocrErrorsPrompt?: string, cardComponents?: boolean, cardOcrFix?: boolean}} [ai]
 */
export function cardPrompt(template, settings, ai = {}) {
    const field = settings.showWordTranslation === false ? '' : `, "translation": ${promptOr(ai.wordTranslationPrompt, WORD_TRANSLATION_PROMPT)}`;
    const language = baseLanguage(settings.ocrLanguage);
    const reading = READING_SYSTEMS[language] ?
        `, "reading": ${promptOr(ai.readingPrompt, READING_PROMPT).replaceAll('{{readingSystem}}', READING_SYSTEMS[language])}` : '';
    const components = ai.cardComponents === false ? '' : `, "components": ${promptOr(ai.componentsPrompt, COMPONENTS_PROMPT)
        .replaceAll('{{componentParts}}', COMPONENT_PARTS[language] || OTHER_PARTS)
        .replaceAll('{{componentReading}}', COMPONENT_READINGS[language] || '')}`;
    const form = `, "form": ${promptOr(ai.formPrompt, FORM_PROMPT)}` +
        (TRANSLITERATED.has(language) ? `, "transliteration": ${promptOr(ai.transliterationPrompt, TRANSLITERATION_PROMPT)}` : '');
    const errors = OCR_ERRORS[language] || (TRANSLITERATED.has(language) ? OTHER_SCRIPT_ERRORS : LATIN_ERRORS);
    const note = ai.cardOcrFix !== false && /\{\{\w+Field\}\}/.test(template) ?
        `\n\n${promptOr(ai.ocrErrorsPrompt, OCR_ERRORS_PROMPT).replaceAll('{{ocrErrors}}', errors)}` : '';
    return fillLanguages(template
        .replaceAll(TRANSLATION_FIELD, field)
        .replaceAll(READING_FIELD, reading)
        .replaceAll(COMPONENTS_FIELD, components)
        .replaceAll(FORM_FIELD, form) + note, settings);
}

/** The components of a reply: objects with a part, its reading or hanja and its meaning. */
function componentList(value) {
    if (!Array.isArray(value)) { return []; }
    return value
        .filter((c) => typeof c === 'object' && c !== null && c.part)
        .map((c) => ({part: String(c.part), reading: String(c.reading || c.hanja || ''), meaning: String(c.meaning || '')}));
}

/**
 * Parses the word-card reply. Returns null when the model did not answer with the JSON object.
 * @param {string} text
 */
export function parseCard(text) {
    if (!text) { return null; }
    const start = text.indexOf('{');
    const end = text.lastIndexOf('}');
    if (start < 0 || end <= start) { return null; }
    try {
        const obj = JSON.parse(text.slice(start, end + 1));
        const list = (v) => (Array.isArray(v) ? v.map(String).filter(Boolean) : (typeof v === 'string' && v ? [v] : []));
        return {
            headword: String(obj.headword || ''),
            ipa: String(obj.ipa || '').replace(/^\/+|\/+$/g, ''),
            cefr: String(obj.cefr || '').toUpperCase().trim(),
            partOfSpeech: list(obj.partOfSpeech),
            labels: list(obj.labels),
            definition: String(obj.definition || ''),
            translation: String(obj.translation || ''),
            reading: String(obj.reading || ''),
            transliteration: String(obj.transliteration || ''),
            form: list(obj.form),
            ocrFix: String(obj.ocrFix || ''),
            synonyms: list(obj.synonyms),
            components: componentList(obj.components),
        };
    } catch (_e) {
        return null;
    }
}

/** Reads usable fields while a JSON word card is still arriving, without showing raw JSON. */
export function parseCardProgress(text) {
    const complete = parseCard(text);
    if (complete) { return complete; }
    const start = text.indexOf('{');
    if (start < 0) { return null; }
    let prefix = text.slice(start);
    const closers = [];
    let inString = false;
    let escaped = false;
    for (let i = 0; i < prefix.length; i++) {
        const c = prefix[i];
        if (inString) {
            if (escaped) {
                if (c === 'u') {
                    if (!/^[0-9a-f]{4}$/i.test(prefix.slice(i + 1, i + 5))) {
                        prefix = prefix.slice(0, i - 1);
                        escaped = false;
                        break;
                    }
                    i += 4;
                }
                escaped = false;
            } else if (c === '\\') { escaped = true; }
            else if (c === '"') { inString = false; }
        } else if (c === '"') { inString = true; }
        else if (c === '{') { closers.push('}'); }
        else if (c === '[') { closers.push(']'); }
        else if (c === '}' || c === ']') {
            if (closers.pop() !== c) { return null; }
        }
    }
    if (escaped) { prefix = prefix.slice(0, -1); }
    if (inString) { prefix += '"'; }
    prefix = prefix.replace(/,\s*$/, '');
    const suffix = closers.reverse().join('');
    // A stream may stop in a key, immediately after a colon, or inside a string/list.
    for (const filler of ['', 'null', ':null']) {
        const card = parseCard(prefix + filler + suffix);
        if (card && (card.headword || card.ipa || card.cefr || card.definition || card.translation || card.reading ||
            card.transliteration || card.form.length || card.partOfSpeech.length || card.labels.length || card.synonyms.length || card.components.length)) { return card; }
    }
    return null;
}
