import { describe, it } from 'node:test'
import assert from 'node:assert/strict'
import { readFileSync, readdirSync, statSync } from 'node:fs'
import { join } from 'node:path'

const webuiRoot = process.cwd()

function filesUnder(dir: string): string[] {
  const out: string[] = []
  for (const entry of readdirSync(dir)) {
    const path = join(dir, entry)
    if (path.includes(`${join('host', '__tests__')}`)) {
      continue
    }
    if (statSync(path).isDirectory()) out.push(...filesUnder(path))
    else if (/\.(ts|vue)$/.test(path)) out.push(path)
  }
  return out
}

describe('neutral host boundary', () => {
  it('keeps renderer and neutral host code free of Electron imports', () => {
    const checked = [
      ...filesUnder(join(webuiRoot, 'src')),
      ...filesUnder(join(webuiRoot, 'host')),
    ]

    for (const file of checked) {
      const text = readFileSync(file, 'utf8')
      assert.doesNotMatch(text, /from ['"]electron['"]/)
      assert.doesNotMatch(text, /require\(['"]electron['"]\)/)
      assert.doesNotMatch(text, /\bipcRenderer\b/)
      assert.doesNotMatch(text, /\bcontextBridge\b/)
      assert.doesNotMatch(text, /['"]\.\.\/api\/desktop['"]/)
    }
  })
})
