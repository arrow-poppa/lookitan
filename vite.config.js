import {resolve} from 'node:path';
import {defineConfig} from 'vite';

// Two pages: the settings window (index.html) and the lookup popup (popup.html).
export default defineConfig({
    clearScreen: false,
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
