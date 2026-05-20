import path from 'node:path'
import { fileURLToPath } from 'node:url'

import { defineConfig } from 'vite'
import vue from '@vitejs/plugin-vue'
import tailwindcss from '@tailwindcss/vite'

const __dirname = path.dirname(fileURLToPath(import.meta.url))

function defaultBuildPlatform() {
  switch (process.env.ECNUVPN_BUILD_PLATFORM ?? process.platform) {
    case 'win32':
    case 'windows':
      return 'windows'
    case 'darwin':
    case 'mac':
    case 'macos':
      return 'macos'
    case 'linux':
      return 'linux'
    default:
      return process.env.ECNUVPN_BUILD_PLATFORM ?? process.platform
  }
}

function defaultOutDir() {
  return path.resolve(
    __dirname,
    '..',
    'build',
    defaultBuildPlatform(),
    'electron',
    'dist',
  )
}

export default defineConfig({
  plugins: [vue(), tailwindcss()],
  base: './',
  build: {
    outDir: process.env.ECNUVPN_WEBUI_OUT_DIR ?? defaultOutDir(),
    emptyOutDir: true,
  },
})
