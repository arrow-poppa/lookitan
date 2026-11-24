import {readFileSync} from 'node:fs';
import {resolve} from 'node:path';
import {defineConfig} from 'vite';

const {version} = JSON.parse(readFileSync(resolve(import.meta.dirname, 'package.json'), 'utf8'));

// Two pages: the settings window (index.html) and the lookup popup (popup.html).
export default defineConfig({
    clearScreen: false,
    // The app's version, for the User-Agent Wikimedia asks clients to send (src/lib/audio.js).
    define: {__LOOKITAN_VERSION__: JSON.stringify(version)},
    server: {
        port: 1420,
        strictPort: true,
    },
    build: {
        target: 'es2022',
        outDir: 'dist',
        emptyOutDir: true,
        rollupOptions: {
            input: {
                main: resolve(import.meta.dirname, 'index.html'),
                popup: resolve(import.meta.dirname, 'popup.html'),
                overlay: resolve(import.meta.dirname, 'overlay.html'),
            },
        },
    },
});
