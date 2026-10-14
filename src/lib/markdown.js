// HTML escaping and the small Markdown subset of AI answers. No imports, so the pop-up, the
// settings window and the Anki note builder can all share it.

export function escapeHtml(s) {
    return String(s).replace(/[&<>"']/g, (c) => ({'&': '&amp;', '<': '&lt;', '>': '&gt;', '"': '&quot;', "'": '&#39;'})[c]);
}

/** Small, safe Markdown subset for AI answers: paragraphs, lists, bold, italic, code. */
export function renderMarkdown(text) {
    const lines = escapeHtml(text).split(/\r?\n/);
    const out = [];
    let list = null;
    const inline = (s) => s
        .replace(/`([^`]+)`/g, '<code>$1</code>')
        .replace(/\*\*([^*]+)\*\*/g, '<strong>$1</strong>')
        .replace(/(^|[^*])\*([^*\s][^*]*)\*/g, '$1<em>$2</em>');
    for (const raw of lines) {
        const line = raw.trimEnd();
        const m = /^\s*(?:[-*•]|\d+[.)])\s+(.*)$/.exec(line);
        if (m) {
            if (!list) { list = []; }
            list.push(`<li>${inline(m[1])}</li>`);
            continue;
        }
        if (list) { out.push(`<ul>${list.join('')}</ul>`); list = null; }
        const h = /^#{1,4}\s+(.*)$/.exec(line);
        if (h) { out.push(`<p><strong>${inline(h[1])}</strong></p>`); continue; }
        if (line.trim() === '') { continue; }
        out.push(`<p>${inline(line)}</p>`);
    }
    if (list) { out.push(`<ul>${list.join('')}</ul>`); }
    return out.join('');
}

/** The same answer as plain text: Markdown marks removed, one line per paragraph or list item. */
export function markdownToText(text) {
    return String(text)
        .split(/\r?\n/)
        .map((line) => line.trim()
            .replace(/^#{1,4}\s+/, '')
            .replace(/^(?:[-*•]|\d+[.)])\s+/, '• ')
            .replace(/`([^`]+)`/g, '$1')
            .replace(/\*\*([^*]+)\*\*/g, '$1')
            .replace(/(^|[^*])\*([^*\s][^*]*)\*/g, '$1$2'))
        .filter((line) => line !== '')
        .join('\n');
}
