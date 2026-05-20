import type { DesktopPlatformRunner } from './base.js'
import darwinRunner from './darwin.js'
import linuxRunner from './linux.js'
import win32Runner from './win32.js'

function resolvePlatformRunner(): DesktopPlatformRunner {
  switch (process.platform) {
    case 'win32':
      return win32Runner
    case 'darwin':
      return darwinRunner
    default:
      return linuxRunner
  }
}

export const platformRunner = resolvePlatformRunner()