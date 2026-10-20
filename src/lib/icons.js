// Lookitan's own outline icons (24x24 viewBox), drawn for this project.

const P = {
    bookmark: '<path d="M7 4h10v16l-5-3.6L7 20z"/>',
    bookmarkFilled: '<path d="M7 4h10v16l-5-3.6L7 20z" fill="currentColor"/>',
    sparkle: '<path d="M12 4l1.6 4.4L18 10l-4.4 1.6L12 16l-1.6-4.4L6 10l4.4-1.6z"/><path d="M18.5 15.5l.7 1.8 1.8.7-1.8.7-.7 1.8-.7-1.8-1.8-.7 1.8-.7z"/>',
    speaker: '<path d="M5 10h3l4-3.5v11L8 14H5z"/><path d="M15.5 9.5a3.5 3.5 0 010 5M17.8 7.2a6.8 6.8 0 010 9.6"/>',
    chevronDown: '<path d="M6 9.5l6 6 6-6"/>',
    chevronUp: '<path d="M6 14.5l6-6 6 6"/>',
    plus: '<path d="M12 5v14M5 12h14"/>',
    close: '<path d="M6 6l12 12M18 6L6 18"/>',
    check: '<path d="M5 12.5l4.5 4.5L19 7.5"/>',
    image: '<rect x="4" y="5" width="16" height="14" rx="2"/><circle cx="9" cy="10" r="1.5"/><path d="M5 17.5l4.5-4.5 3 3 2.5-2.5 4 4"/>',
    chevronLeft: '<path d="M14.5 6l-6 6 6 6"/>',
    chevronRight: '<path d="M9.5 6l6 6-6 6"/>',
    open: '<path d="M14 5h5v5M19 5l-8 8M17 14v5H5V7h5"/>',
    pin: '<path d="M9 4h6l-1 5 3 3H7l3-3zM12 12v8"/>',
    search: '<circle cx="10.5" cy="10.5" r="5.5"/><path d="M15 15l5 5"/>',
    keyboard: '<rect x="3" y="6.5" width="18" height="11" rx="2"/><path d="M6.5 10h1M10 10h1M13.5 10h1M17 10h.5M6.5 13.5h11"/>',
    mouse: '<rect x="7" y="3.5" width="10" height="17" rx="5"/><path d="M12 7v3"/>',
    gamepad: '<path d="M7.5 8h9a4.5 4.5 0 014.4 5.4l-.7 3.4a2 2 0 01-3.4 1l-2.3-2.3H9.5l-2.3 2.3a2 2 0 01-3.4-1l-.7-3.4A4.5 4.5 0 017.5 8z"/><path d="M8 11v3M6.5 12.5h3M15.5 12h.01M17.5 13.5h.01"/>',
    scan: '<path d="M4 8V5h3M17 5h3v3M20 16v3h-3M7 19H4v-3M7 12h10"/>',
    swap: '<path d="M5 8h13l-3-3M19 16H6l3 3"/>',
    translate: '<path d="M4 6h9M8.5 4v2c0 4-2.2 7-5 8.5M6 9.5c1 2 3 3.6 5.5 4.5"/><path d="M13 20l3.5-8 3.5 8M14.2 17.3h4.6"/>',
    eye: '<path d="M3.5 12S7 6 12 6s8.5 6 8.5 6-3.5 6-8.5 6-8.5-6-8.5-6z"/><circle cx="12" cy="12" r="2.5"/>',
    eyeOff: '<path d="M4 4l16 16M10.6 6.2A8.9 8.9 0 0112 6c5 0 8.5 6 8.5 6a14.6 14.6 0 01-3 3.5M6.6 8.3A14.4 14.4 0 003.5 12S7 18 12 18a8.4 8.4 0 003.4-.7"/><path d="M10 10.2a2.5 2.5 0 003.8 3.3"/>',
    book: '<path d="M5 5.5A1.5 1.5 0 016.5 4H18v14H6.5A1.5 1.5 0 005 19.5z"/><path d="M5 19.5A1.5 1.5 0 006.5 21H18v-3"/>',
    cloud: '<path d="M7.5 18h9.5a3.5 3.5 0 00.4-7 5 5 0 00-9.7-1.2A4 4 0 007.5 18z"/>',
    list: '<path d="M9 7h11M9 12h11M9 17h11M4.5 7h.01M4.5 12h.01M4.5 17h.01"/>',
    power: '<path d="M12 4v7"/><path d="M7.2 7a7 7 0 109.6 0"/>',
    trash: '<path d="M5 7h14M10 7V5h4v2M7 7l1 13h8l1-13"/>',
    refresh: '<path d="M18.5 9A7 7 0 006 7.5L4.5 9M5.5 15A7 7 0 0018 16.5l1.5-1.5"/><path d="M4.5 5v4h4M19.5 19v-4h-4"/>',
    alert: '<circle cx="12" cy="12" r="8.5"/><path d="M12 8v4.5M12 16h.01"/>',
    cards: '<rect x="4" y="7" width="12" height="13" rx="2"/><path d="M8 4h10a2 2 0 012 2v11"/>',
    sync: '<path d="M19 12a7 7 0 01-12.2 4.7M5 12a7 7 0 0112.2-4.7"/><path d="M17.5 4v3.5H14M6.5 20v-3.5H10"/>',
    export: '<path d="M4 12h12M12 8l4 4-4 4M20 5v14"/>',
    settings: '<circle cx="12" cy="12" r="3"/><path d="M12 3.5v2.2M12 18.3v2.2M3.5 12h2.2M18.3 12h2.2M6 6l1.6 1.6M16.4 16.4L18 18M6 18l1.6-1.6M16.4 7.6L18 6"/>',
    play: '<path d="M8 5.5v13l10-6.5z"/>',
    pause: '<path d="M8 5.5v13M16 5.5v13"/>',
    focus: '<path d="M4 9V5h4M16 5h4v4M20 15v4h-4M8 19H4v-4"/><circle cx="12" cy="12" r="2.5"/>',
    drag: '<path d="M9 6h.01M15 6h.01M9 12h.01M15 12h.01M9 18h.01M15 18h.01"/>',
    folder: '<path d="M4 7.5A1.5 1.5 0 015.5 6H10l2 2h6.5A1.5 1.5 0 0120 9.5v8a1.5 1.5 0 01-1.5 1.5h-13A1.5 1.5 0 014 17.5z"/>',
    copy: '<rect x="8" y="8" width="11" height="11" rx="2"/><path d="M16 8V6a2 2 0 00-2-2H6a2 2 0 00-2 2v8a2 2 0 002 2h2"/>',
    zoomIn: '<circle cx="10.5" cy="10.5" r="5.5"/><path d="M15 15l5 5M8.5 10.5h4M10.5 8.5v4"/>',
    zoomOut: '<circle cx="10.5" cy="10.5" r="5.5"/><path d="M15 15l5 5M8.5 10.5h4"/>',
};

/**
 * @param {keyof typeof P} name
 * @param {string} [cls] extra class: 'sm' or 'lg'
 */
export function icon(name, cls = '') {
    return `<svg class="i ${cls}" viewBox="0 0 24 24" aria-hidden="true">${P[name] || ''}</svg>`;
}

/** Replaces every <i data-icon="name"> placeholder in the document with its SVG. */
export function hydrateIcons(root = document) {
    for (const el of root.querySelectorAll('i[data-icon]')) {
        el.outerHTML = icon(el.dataset.icon, el.dataset.size || '');
    }
}
