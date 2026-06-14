#!/usr/bin/env node
const fs = require('node:fs')
const os = require('node:os')
const path = require('node:path')
const { spawnSync } = require('node:child_process')

const projectRoot = path.resolve(__dirname, '..')
const tests = process.argv.slice(2)

if (tests.length === 0) {
  console.error('usage: node scripts/run-electron-test.cjs <desktop/test.ts> [...]')
  process.exit(2)
}

const tscBin = path.join(
  projectRoot,
  'node_modules',
  'typescript',
  'bin',
  'tsc',
)

if (!fs.existsSync(tscBin)) {
  console.error('TypeScript compiler not found. Run pnpm install first.')
  process.exit(1)
}

const outDir = path.join(os.tmpdir(), `ecnu-vpn-electron-tests-${process.pid}`)
fs.rmSync(outDir, { recursive: true, force: true })

try {
  const compile = spawnSync(
    process.execPath,
    [tscBin, '-p', 'tsconfig.electron.json', '--outDir', outDir],
    { cwd: projectRoot, stdio: 'inherit' },
  )
  if (compile.status !== 0) {
    process.exit(compile.status ?? 1)
  }

  for (const test of tests) {
    const normalized = test.replace(/\\/g, '/')
    if (!normalized.startsWith('desktop/') || !normalized.endsWith('.ts')) {
      console.error(`electron test path must be a desktop/*.ts file: ${test}`)
      process.exit(2)
    }
    const compiled = path.join(
      outDir,
      normalized.slice('desktop/'.length).replace(/\.ts$/, '.js'),
    )
    const run = spawnSync(process.execPath, [compiled], {
      cwd: projectRoot,
      stdio: 'inherit',
    })
    if (run.status !== 0) {
      process.exit(run.status ?? 1)
    }
  }
} finally {
  fs.rmSync(outDir, { recursive: true, force: true })
}
