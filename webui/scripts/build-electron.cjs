const fs = require('node:fs')
const path = require('node:path')
const { spawnSync } = require('node:child_process')

const { getBuildLayout } = require('./build-layout.cjs')

const layout = getBuildLayout()
const tscBin = path.join(
  layout.webuiRoot,
  'node_modules',
  '.bin',
  process.platform === 'win32' ? 'tsc.cmd' : 'tsc',
)

fs.rmSync(layout.electronOutDir, { recursive: true, force: true })
fs.mkdirSync(layout.electronOutDir, { recursive: true })

const command = process.platform === 'win32'
  ? `"${tscBin}" -p tsconfig.electron.json --outDir "${layout.electronOutDir}"`
  : tscBin

const args = process.platform === 'win32'
  ? []
  : ['-p', 'tsconfig.electron.json', '--outDir', layout.electronOutDir]

const result = spawnSync(command, args, {
  cwd: layout.webuiRoot,
  stdio: 'inherit',
  env: process.env,
  shell: process.platform === 'win32',
})

if (result.error) {
  throw result.error
}

if (typeof result.status === 'number' && result.status !== 0) {
  process.exit(result.status)
}