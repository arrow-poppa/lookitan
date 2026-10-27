// Saved-word cards: built from a lookup in the popup, listed in the Words tab, sent to Anki.

import {invoke} from '@tauri-apps/api/core';
import {api} from './api.js';
import {sentenceAudioFile, wordAudioFile} from './audio.js';
import {clozeRange, selectedSpan} from './ai/selection.js';
import {validDisplayHeadword, validDisplaySurface} from './ai/word-card.js';

/** A saved word's display form of its headword, checked again against its dictionary form and
 * language (an edit in the Palavras tab may have changed either); '' when it no longer fits. */
export function savedDisplayHeadword(card) {
    return validDisplayHeadword({headword: card.dictionaryForm, displayHeadword: card.displayHeadword}, card.language);
}

/** A saved word's display form of the word on screen, checked again against `word`; '' when it no
 * longer fits. */
export function savedDisplaySurface(card) {
    return validDisplaySurface(card, card.word, card.language);
}

/**
 * @param {object} lookup popup state: payload, aiCard, explanation, contextTranslation, glossaries
 * @param {object} settings
 */
export function cardFromLookup(lookup, settings) {
    const ai = lookup.aiCard || {};
    return {
        word: lookup.payload.word,
        dictionaryForm: ai.headword || lookup.payload.entries?.[0]?.headword || lookup.payload.word,
        // The headword as shown (stress, macrons; see validDisplayHeadword), when the card had a
        // valid one; `dictionaryForm` stays the form looked up and matched.
        displayHeadword: validDisplayHeadword(ai, settings.ocrLanguage),
        // The word on screen with its stress (or vowel length) marked, when valid; `word` stays as read.
        displaySurface: validDisplaySurface(ai, lookup.payload.word, settings.ocrLanguage),
        translation: ai.translation || '',
        definition: ai.definition || '',
        explanation: lookup.explanation || '',
        contextTranslation: lookup.contextTranslation || '',
        // The translation of the word's sentence alone, cut from the paragraph's (see
        // splitContextTranslation); words saved before 2026-10-27 have only `contextTranslation`,
        // which was the sentence's.
        sentenceTranslation: lookup.sentenceTranslation || '',
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
        // The longer unit the AI card proposed ("Socrates Awards", "turned … off"), and where it was
        // found in `context` (null when it was not), so the saved card marks it as the pop-up did.
        // Only the marks place anything; `contextAt` places the word for the cloze.
        selection: ai.selection || null,
        contextMarks: lookup.contextMarks || null,
        contextAt: Number.isInteger(lookup.payload.contextAt) ? lookup.payload.contextAt : null,
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
        displayHeadword: savedDisplayHeadword(card),
        displaySurface: savedDisplaySurface(card),
        clozeWord: card.context && Array.isArray(card.contextMarks) ? selectedSpan(card.context, card.contextMarks) : '',
        clozeAt: card.context ? clozeRange(card.context, card.contextAt, card.word, card.contextMarks) : null,
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
        sentenceTranslation: card.sentenceTranslation || '',
        title: card.windowTitle,
        query: card.word,
        screenshot: async () => (card.image ? await api.wordImage(card.image, card.imageCrop) : null),
        sentenceAudio: () => sentenceAudioFile(card.sentence, settings.audio, card.language || settings.ocrLanguage, {invoke}),
        // {audio}: the word as saved from the screen first, its dictionary form's recordings after.
        audio: () => wordAudioFile(settings.audio, card.word, [card.dictionaryForm].filter(Boolean), card.language || settings.ocrLanguage, {invoke}),
    };
}

function csvCell(v) {
    const s = Array.isArray(v) ? v.join(', ') : String(v ?? '');
    return /[",\n\r;]/.test(s) ? `"${s.replace(/"/g, '""')}"` : s;
}

export const CSV_COLUMNS = [
    ['word', 'Palavra'], ['dictionaryForm', 'Forma de dicionário'], ['translation', 'Tradução'],
    ['definition', 'Definição'], ['sentence', 'Frase'], ['sentenceTranslation', 'Tradução da frase'],
    ['ipa', 'IPA'], ['cefr', 'CEFR'], ['partOfSpeech', 'Classe gramatical'], ['explanation', 'Explicação'],
    ['windowTitle', 'Janela'], ['createdAt', 'Data'], ['displayHeadword', 'Forma de dicionário exibida'], ['displaySurface', 'Palavra exibida'],
];

export function toCsv(cards) {
    const head = CSV_COLUMNS.map(([, label]) => csvCell(label)).join(',');
    // The sentence's translation goes with the sentence; an older word has only its contextTranslation.
    const cell = (c, key) => (key === 'createdAt' ? new Date(c.createdAt).toISOString() :
        key === 'sentenceTranslation' ? String(c.sentenceTranslation || c.contextTranslation || '').replace(/\*\*/g, '') :
            key === 'displayHeadword' ? savedDisplayHeadword(c) : key === 'displaySurface' ? savedDisplaySurface(c) : c[key]);
    const rows = cards.map((c) => CSV_COLUMNS.map(([key]) => csvCell(cell(c, key))).join(','));
    return [head, ...rows].join('\r\n');
}

/** Quizlet import format: term<TAB>definition, one card per line. */
export function toQuizlet(cards) {
    return cards
        .map((c) => `${(c.word || '').replace(/\s+/g, ' ')}\t${[c.translation, c.definition].filter(Boolean).join(' — ').replace(/\s+/g, ' ')}`)
        .join('\n');
}
