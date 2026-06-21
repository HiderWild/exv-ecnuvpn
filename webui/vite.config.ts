import { defineConfig } from 'vite'
import vue from '@vitejs/plugin-vue'
import tailwindcss from '@tailwindcss/vite'

export default defineConfig({
  plugins: [vue(), tailwindcss()],
  base: './',
  build: {
    outDir: process.env.EXV_WEBUI_OUT_DIR ?? 'dist',
    emptyOutDir: true,
  },
  server: {
    host: '127.0.0.1',
  },
})
