// Minimal stand-in for Yomitan's ExtensionError (an Error with attached data).

export class ExtensionError extends Error {
    /**
     * @param {string} message
     */
    constructor(message) {
        super(message);
        this.name = 'ExtensionError';
        this._data = void 0;
    }

    get data() { return this._data; }
    set data(value) { this._data = value; }
}
