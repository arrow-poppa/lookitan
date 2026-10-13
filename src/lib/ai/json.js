// From Yomitan (GPL-3.0-or-later), ext/js/core/json.js.

/**
 * @param {string} value
 * @returns {unknown}
 */
export function parseJson(value) {
    return JSON.parse(value);
}

/**
 * @param {Response} response
 * @returns {Promise<unknown>}
 */
export async function readResponseJson(response) {
    return await response.json();
}
