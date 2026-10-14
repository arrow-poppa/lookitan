// Prompts and parsing for the two built-in AI "dictionaries": the word card (word in context, with
// IPA, CEFR level, part of speech, definition and translation) and the context translator.

/** The translation key of CARD_PROMPT, left out when the word translation is turned off. */
export const CARD_TRANSLATION_FIELD = ', "translation": the translation of the word in this context into {{translationLanguage}}';

export const CARD_PROMPT =
    "Word: '{{target}}'\n" +
    "Context: '{{sentence}}'\n\n" +
    'Describe the word exactly as it is used in this context. Reply with JSON only (no markdown, no code fence) with these keys:\n' +
    '{"headword": the dictionary form of the word, ' +
    '"ipa": IPA transcription without slashes, ' +
    '"cefr": CEFR level (A1, A2, B1, B2, C1 or C2) or "", ' +
    '"partOfSpeech": ["noun", ...], ' +
    '"labels": [usage, register or domain labels, e.g. "informal", "programming"], ' +
    '"definition": a one or two sentence definition in {{definitionLanguage}}, ' +
    '"translation": the translation of the word in this context into {{translationLanguage}}}';

export const CONTEXT_PROMPT =
    'Translate the text below into {{translationLanguage}}. ' +
    "Wrap the translation of '{{target}}' in double asterisks, like **this**. " +
    'Reply with the translation only.\n\n{{sentence}}';

export const EXPLAIN_PROMPT =
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
        .replaceAll('{{translationLanguage}}', languageName(settings.translationLanguage || 'en-US'));
}

/**
 * The word-card prompt as sent: languages filled in and, when the word translation is turned off,
 * without the translation key so the model does not spend tokens on it. A custom prompt that
 * does not contain that key is left as it is.
 */
export function cardPrompt(template, settings) {
    const prompt = settings.showWordTranslation === false ? template.replace(CARD_TRANSLATION_FIELD, '') : template;
    return fillLanguages(prompt, settings);
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
        if (card && (card.headword || card.ipa || card.cefr || card.definition || card.translation ||
            card.partOfSpeech.length || card.labels.length)) { return card; }
    }
    return null;
}
