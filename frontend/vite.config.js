import { defineConfig } from 'vite';
import { svelte } from '@sveltejs/vite-plugin-svelte';

export default defineConfig({
  plugins: [
    svelte({
      // Принудительно выключаем руны: компоненты используют синтаксис Svelte 4.
      compilerOptions: {
        runes: false,
      },
    }),
  ],
});
