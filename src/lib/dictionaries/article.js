// Runs inside each dictionary iframe, without access to Tauri commands.
(function () {
    'use strict';
    const dictionary = location.pathname.split('/')[2];
    let playing = null;
    const decode = (text) => { try { return decodeURIComponent(text); } catch { return text; } };
    const post = (message) => parent.postMessage({...message, lookitan: true}, '*');
    const resource = (path) => `/d/${dictionary}/${decode(path).replace(/\\/g, '/').replace(/^\/+/, '').split('/').map(encodeURIComponent).join('/')}`;

    function stopAudio() {
        if (playing) { playing.pause(); playing = null; }
    }
    function scrollTo(fragment) {
        const name = decode(fragment.replace(/^#/, ''));
        const target = document.getElementById(name) || document.getElementsByName(name)[0];
        target?.scrollIntoView();
    }
    document.addEventListener('click', (event) => {
        const anchor = event.target?.closest?.('a');
        if (!anchor) { return; }
        const href = anchor.getAttribute('href') || '';
        if (/^sound:\/\//i.test(href) || /\.(mp3|wav|ogg|oga|spx|m4a|aac|flac|opus)(?:[?#]|$)/i.test(href)) {
            event.preventDefault();
            stopAudio();
            let path = href.replace(/^sound:\/\//i, '');
            const prefix = `/d/${dictionary}/`;
            if (path.startsWith(prefix)) { path = path.slice(prefix.length); }
            playing = new Audio(/^https?:\/\//i.test(path) ? path : resource(path));
            void playing.play().catch(() => {});
        } else if (/^(entry|bword):\/\//i.test(href)) {
            event.preventDefault();
            const value = href.replace(/^(entry|bword):\/\//i, '');
            const hash = value.indexOf('#');
            const word = decode(hash < 0 ? value : value.slice(0, hash));
            const fragment = hash < 0 ? '' : decode(value.slice(hash + 1));
            if (!word && fragment) { scrollTo(fragment); }
            else if (word) { post({type: 'lookup', text: word, fragment}); }
        } else if (/^https?:\/\//i.test(href)) {
            event.preventDefault();
            post({type: 'open', url: href});
        }
    }, true);
    document.addEventListener('play', (event) => {
        if (event.target !== playing) { stopAudio(); playing = event.target; }
    }, true);
    document.addEventListener('selectionchange', () => {
        post({type: 'selection', text: String(window.getSelection() || '')});
    });
    window.addEventListener('message', (event) => {
        if (event.source === parent && event.data?.lookitan === true && event.data.type === 'stop-audio') { stopAudio(); }
    });
    const report = () => post({type: 'height', height: Math.ceil(document.documentElement.scrollHeight)});
    window.addEventListener('load', () => { if (location.hash) { scrollTo(location.hash); } report(); });
    try { new ResizeObserver(report).observe(document.documentElement); } catch { /* old WebViews */ }
    setTimeout(report, 60);
})();
