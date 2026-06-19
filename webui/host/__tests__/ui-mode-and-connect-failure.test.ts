import { describe, it } from 'node:test'
import assert from 'node:assert/strict'
import { readFileSync } from 'node:fs'
import { join } from 'node:path'
import { createRequire } from 'node:module'
import type * as TypeScript from 'typescript'

const webuiRoot = process.cwd()
const require = createRequire(join(webuiRoot, 'package.json'))
const ts = require('typescript') as typeof TypeScript

function readSource(...parts: string[]) {
  return readFileSync(join(webuiRoot, ...parts), 'utf8')
}

const configStoreText = readSource('src', 'stores', 'config.ts')
const appText = readSource('src', 'App.vue')
const vpnStoreText = readSource('src', 'stores', 'vpn.ts')
const dashboardPageText = readSource('src', 'pages', 'DashboardPage.vue')
const minimalModeViewText = readSource('src', 'components', 'MinimalModeView.vue')

function vueScriptSetup(text: string) {
  const match = text.match(/<script setup[^>]*>([\s\S]*?)<\/script>/)
  assert.ok(match, 'Vue file should contain <script setup>')
  return match[1]
}

function sourceFile(name: string, text: string) {
  return ts.createSourceFile(name, text, ts.ScriptTarget.Latest, true, ts.ScriptKind.TS)
}

function collect<T>(
  node: TypeScript.Node,
  predicate: (node: TypeScript.Node) => T | undefined,
): T[] {
  const out: T[] = []
  function visit(current: TypeScript.Node) {
    const value = predicate(current)
    if (value !== undefined) out.push(value)
    ts.forEachChild(current, visit)
  }
  visit(node)
  return out
}

function stringLiterals(text: string) {
  return collect(sourceFile('source.ts', text), (node) =>
    ts.isStringLiteralLike(node) ? node.text : undefined,
  )
}

function functionNames(text: string) {
  return new Set(collect(sourceFile('source.ts', text), (node) =>
    ts.isFunctionDeclaration(node) && node.name ? node.name.text : undefined,
  ))
}

function deletePropertyNames(text: string) {
  return new Set(collect(sourceFile('source.ts', text), (node) => {
    if (!ts.isDeleteExpression(node)) return undefined
    const target = node.expression
    if (ts.isPropertyAccessExpression(target)) return target.name.text
    return undefined
  }))
}

function hasCallNamed(text: string, name: string) {
  return collect(sourceFile('source.ts', text), (node) => {
    if (!ts.isCallExpression(node)) return undefined
    const expression = node.expression
    if (ts.isIdentifier(expression)) return expression.text === name
    if (ts.isPropertyAccessExpression(expression)) return expression.name.text === name
    return false
  }).some(Boolean)
}

function hasPropertyCall(text: string, propertyName: string) {
  return collect(sourceFile('source.ts', text), (node) => {
    if (!ts.isCallExpression(node)) return undefined
    const expression = node.expression
    return ts.isPropertyAccessExpression(expression) && expression.name.text === propertyName
  }).some(Boolean)
}

function unionStringMembers(text: string, aliasName: string) {
  const values = new Set<string>()
  const file = sourceFile('source.ts', text)
  collect(file, (node) => {
    if (!ts.isTypeAliasDeclaration(node) || node.name.text !== aliasName) return undefined
    collect(node.type, (member) => {
      if (
        ts.isLiteralTypeNode(member) &&
        ts.isStringLiteral(member.literal)
      ) {
        values.add(member.literal.text)
      }
      return undefined
    })
    return true
  })
  return values
}

function objectLiteralPropertyNames(text: string, objectName: string) {
  const names = new Set<string>()
  collect(sourceFile('source.ts', text), (node) => {
    if (!ts.isVariableDeclaration(node) || node.name.getText() !== objectName) return undefined
    const initializer = node.initializer
    if (!initializer || !ts.isObjectLiteralExpression(initializer)) return undefined
    for (const property of initializer.properties) {
      if (ts.isPropertyAssignment(property)) {
        const name = property.name
        if (ts.isIdentifier(name) || ts.isStringLiteral(name)) names.add(name.text)
      }
    }
    return true
  })
  return names
}

describe('frontend-owned UI mode state', () => {
  it('keeps minimal mode and first-run service prompt state in renderer localStorage', () => {
    const literals = stringLiterals(configStoreText)
    assert.ok(literals.includes('ecnu-vpn:minimal-mode'))
    assert.ok(literals.includes('ecnu-vpn:service-install-prompt-seen'))
    assert.ok(hasPropertyCall(configStoreText, 'getItem'))
    assert.ok(hasPropertyCall(configStoreText, 'setItem'))

    const names = functionNames(configStoreText)
    assert.ok(names.has('applyFrontendLocalSettings'))
    assert.ok(names.has('persistFrontendLocalSettings'))
  })

  it('does not send renderer-only settings back to core config storage', () => {
    const deleted = deletePropertyNames(configStoreText)
    assert.ok(deleted.has('minimal_mode'))
    assert.ok(deleted.has('service_install_prompt_seen'))
    assert.match(configStoreText, /Object\.keys\(remoteSettings\)\.length\s*===\s*0/)
  })

  it('suppresses stale asynchronous window mode writes after rapid toggles', () => {
    const script = vueScriptSetup(appText)
    assert.match(script, /\bwindowModeRequest\b/)
    assert.match(script, /\+\+\s*windowModeRequest/)
    assert.match(script, /request\s*!==\s*windowModeRequest/)
    assert.ok(hasCallNamed(script, 'watch'))
    assert.ok(hasPropertyCall(script, 'setMode'))
  })
})

describe('connection failure presentation contract', () => {
  it('keeps backend connection failures visible while suppressing user-cancelled errors', () => {
    const errorTypes = unionStringMembers(vpnStoreText, 'VpnErrorType')
    assert.ok(errorTypes.has('connection_failed'))
    assert.ok(errorTypes.has('connection_attempt_active'))
    assert.ok(errorTypes.has('user_cancelled'))

    const contractErrors = objectLiteralPropertyNames(vpnStoreText, 'contractErrorMap')
    assert.ok(contractErrors.has('connection_failed'))
    assert.ok(contractErrors.has('connection_attempt_active'))

    assert.match(vpnStoreText, /error_type\s*!==\s*'user_cancelled'[\s\S]{0,80}setError/)
  })

  it('routes the in-progress yellow button to cancellation instead of a second connect', () => {
    assert.match(vpnStoreText, /case\s+'elevated connecting':/)
    assert.ok(hasCallNamed(vpnStoreText, 'cancelConnect'))
    assert.match(vpnStoreText, /api\.post[\s\S]{0,120}'\/disconnect'/)

    const dashboardScript = vueScriptSetup(dashboardPageText)
    const minimalScript = vueScriptSetup(minimalModeViewText)
    assert.ok(stringLiterals(dashboardScript).includes('取消连接'))
    assert.ok(hasPropertyCall(dashboardScript, 'cancelConnect'))
    assert.ok(hasPropertyCall(minimalScript, 'cancelConnect'))
  })
})
