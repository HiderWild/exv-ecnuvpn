import path from 'node:path'
import { createRequire } from 'node:module'

import { defineConfig } from 'vite'
import vue from '@vitejs/plugin-vue'
import tailwindcss from '@tailwindcss/vite'

const require = createRequire(import.meta.url)
const { getBuildLayout } = require('./scripts/build-layout.cjs') as {
  getBuildLayout: () => { rendererOutDir: string }
}

function defaultOutDir() {
  return path.resolve(getBuildLayout().rendererOutDir)
}

export default defineConfig({
  plugins: [vue(), tailwindcss()],
  base: './',
  build: {
    outDir: process.env.ECNUVPN_WEBUI_OUT_DIR ?? defaultOutDir(),
    emptyOutDir: true,
  },
  server: {
    host: '127.0.0.1', // Bind to IPv4 only to avoid Windows IPv6 permission issues
  },
})
