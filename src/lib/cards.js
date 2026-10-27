// Saved-word cards: built from a lookup in the popup, listed in the Words tab, sent to Anki.

import {invoke} from '@tauri-apps/api/core';
import {api} from './api.js';
import {sentenceAudioFile} from './audio.js';
import {locateSelection, selectedSpan} from './ai/selection.js';

/**
 * @param {object} lookup popup state: payload, aiCard, explanation, contextTranslation, glossaries
 * @param {object} settings
 */
export function cardFromLookup(lookup, settings) {
    const ai = lookup.aiCard || {};
    return {
        word: lookup.payload.word,
        dictionaryForm: ai.headword || lookup.payload.entries?.[0]?.headword || lookup.payload.word,
        translation: ai.translation || '',
        definition: ai.definition || '',
        explanation: lookup.explanation || '',
        contextTranslation: lookup.contextTranslation || '',
        sentence: lookup.payload.sentence || '',
        context: lookup.payload.context || '',
        ipa: ai.ipa || '',
        cefr: ai.cefr || '',
        partOfSpeech: ai.partOfSpeech || [],
        labels: ai.labels || [],
        reading: ai.reading || '',
        transliteration: ai.transliteration || '',
        form: ai.form || [],
        ocrFix: ai.ocrFix || '',
        synonyms: ai.synonyms || [],
        components: ai.components || [],
        // The longer unit the AI card described ("Socrates Awards", "turned … off") and where it
        // is in `context`, so the saved card marks it as the pop-up did.
        selection: ai.selection || null,
        contextMarks: lookup.payload.context ? locateSelection(lookup.payload.context, lookup.payload.contextAt, lookup.payload.word, ai.selection) : null,
        language: settings.ocrLanguage,
        windowTitle: lookup.payload.app?.title || '',
        executable: lookup.payload.app?.executable || '',
        glossaries: lookup.glossaries || [],
    };
}

/**
 * Anki note context for a saved card.
 * @param {object} card
 * @param {string[]} [dictionaryOrder] see glossaryDictionaryNames
 * @param {object} [settings] the app settings, for the voice of {sentence-audio}
 */
export function noteContextFromCard(card, dictionaryOrder = [], settings = {}) {
    return {
        dictionaryOrder,
        word: card.word,
        headword: card.dictionaryForm,
        clozeWord: card.context && Array.isArray(card.contextMarks) ? selectedSpan(card.context, card.contextMarks) : '',
        sentence: card.sentence,
        context: card.context,
        glossaries: card.glossaries || [],
        aiCard: {
            ipa: card.ipa,
            cefr: card.cefr,
            partOfSpeech: card.partOfSpeech,
            labels: card.labels,
            definition: card.definition,
            translation: card.translation,
            reading: card.reading,
            transliteration: card.transliteration,
            form: card.form,
            ocrFix: card.ocrFix,
            synonyms: card.synonyms,
            components: card.components,
        },
        aiExplanation: card.explanation,
        contextTranslation: card.contextTranslation,
        title: card.windowTitle,
        query: card.word,
        screenshot: async () => (card.image ? await api.wordImage(card.image, card.imageCrop) : null),
        sentenceAudio: () => sentenceAudioFile(card.sentence, settings.audio, card.language || settings.ocrLanguage, {invoke}),
    };
}

function csvCell(v) {
    const s = Array.isArray(v) ? v.join(', ') : String(v ?? '');
    return /[",\n\r;]/.test(s) ? `"${s.replace(/"/g, '""')}"` : s;
}

export const CSV_COLUMNS = [
    ['word', 'Palavra'], ['dictionaryForm', 'Forma de dicionário'], ['translation', 'Tradução'],
    ['definition', 'Definição'], ['sentence', 'Frase'], ['contextTranslation', 'Tradução da frase'],
    ['ipa', 'IPA'], ['cefr', 'CEFR'], ['partOfSpeech', 'Classe gramatical'], ['explanation', 'Explicação'],
    ['windowTitle', 'Janela'], ['createdAt', 'Data'],
];

export function toCsv(cards) {
    const head = CSV_COLUMNS.map(([, label]) => csvCell(label)).join(',');
    const rows = cards.map((c) => CSV_COLUMNS.map(([key]) => csvCell(key === 'createdAt' ? new Date(c.createdAt).toISOString() : c[key])).join(','));
    return [head, ...rows].join('\r\n');
}

/** Quizlet import format: term<TAB>definition, one card per line. */
export function toQuizlet(cards) {
    return cards
        .map((c) => `${(c.word || '').replace(/\s+/g, ' ')}\t${[c.translation, c.definition].filter(Boolean).join(' — ').replace(/\s+/g, ' ')}`)
        .join('\n');
}
