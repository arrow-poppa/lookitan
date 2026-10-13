// From Yomitan (GPL-3.0-or-later), ext/js/core/object-utilities.js.

/**
 * @param {unknown} value
 * @returns {value is Record<string, unknown>}
 */
export function isObjectNotArray(value) {
    return typeof value === 'object' && value !== null && !Array.isArray(value);
}
