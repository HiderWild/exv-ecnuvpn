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

function hasSetModeCallWithRequest(text: string) {
  return collect(sourceFile('source.ts', text), (node) => {
    if (!ts.isCallExpression(node)) return undefined
    const expression = node.expression
    if (!ts.isPropertyAccessExpression(expression) || expression.name.text !== 'setMode') return false
    return node.arguments.length >= 2 &&
      ts.isIdentifier(node.arguments[0]) &&
      node.arguments[0].text === 'mode' &&
      ts.isIdentifier(node.arguments[1]) &&
      node.arguments[1].text === 'request'
  }).some(Boolean)
}

function hasIdentifier(text: string, name: string) {
  return collect(sourceFile('source.ts', text), (node) =>
    ts.isIdentifier(node) && node.text === name ? true : undefined,
  ).some(Boolean)
}

function hasPrefixIncrement(text: string, name: string) {
  return collect(sourceFile('source.ts', text), (node) =>
    ts.isPrefixUnaryExpression(node) &&
    node.operator === ts.SyntaxKind.PlusPlusToken &&
    ts.isIdentifier(node.operand) &&
    node.operand.text === name ? true : undefined,
  ).some(Boolean)
}

function hasInequality(text: string, leftName: string, rightName: string) {
  return collect(sourceFile('source.ts', text), (node) => {
    if (!ts.isBinaryExpression(node)) return undefined
    const isInequality =
      node.operatorToken.kind === ts.SyntaxKind.ExclamationEqualsEqualsToken ||
      node.operatorToken.kind === ts.SyntaxKind.ExclamationEqualsToken
    return isInequality &&
      ts.isIdentifier(node.left) &&
      ts.isIdentifier(node.right) &&
      node.left.text === leftName &&
      node.right.text === rightName
  }).some(Boolean)
}

function hasObjectKeysLengthZeroReturn(text: string, objectName: string) {
  return collect(sourceFile('source.ts', text), (node) => {
    if (!ts.isIfStatement(node)) return undefined
    if (!ts.isReturnStatement(node.thenStatement)) return false
    const expression = node.expression
    if (!ts.isBinaryExpression(expression)) return false
    if (expression.operatorToken.kind !== ts.SyntaxKind.EqualsEqualsEqualsToken) return false
    if (!ts.isNumericLiteral(expression.right) || expression.right.text !== '0') return false
    const left = expression.left
    if (!ts.isPropertyAccessExpression(left) || left.name.text !== 'length') return false
    const call = left.expression
    if (!ts.isCallExpression(call)) return false
    const callee = call.expression
    if (!ts.isPropertyAccessExpression(callee) || callee.name.text !== 'keys') return false
    if (!ts.isIdentifier(callee.expression) || callee.expression.text !== 'Object') return false
    return call.arguments.length === 1 &&
      ts.isIdentifier(call.arguments[0]) &&
      call.arguments[0].text === objectName
  }).some(Boolean)
}

function hasSwitchCase(text: string, literal: string) {
  return collect(sourceFile('source.ts', text), (node) =>
    ts.isCaseClause(node) &&
    ts.isStringLiteralLike(node.expression) &&
    node.expression.text === literal ? true : undefined,
  ).some(Boolean)
}

function hasApiPostToLiteral(text: string, path: string) {
  return collect(sourceFile('source.ts', text), (node) => {
    if (!ts.isCallExpression(node)) return undefined
    const expression = node.expression
    if (!ts.isPropertyAccessExpression(expression) || expression.name.text !== 'post') return false
    if (!ts.isIdentifier(expression.expression) || expression.expression.text !== 'api') return false
    const first = node.arguments[0]
    return first != null && ts.isStringLiteralLike(first) && first.text === path
  }).some(Boolean)
}

function hasGuardedSetError(text: string, skippedErrorType: string) {
  return collect(sourceFile('source.ts', text), (node) => {
    if (!ts.isIfStatement(node)) return undefined
    const expression = node.expression
    if (!ts.isBinaryExpression(expression)) return false
    const isInequality =
      expression.operatorToken.kind === ts.SyntaxKind.ExclamationEqualsEqualsToken ||
      expression.operatorToken.kind === ts.SyntaxKind.ExclamationEqualsToken
    if (!isInequality) return false
    if (!ts.isStringLiteralLike(expression.right) || expression.right.text !== skippedErrorType) return false
    return collect(node.thenStatement, (child) => {
      if (!ts.isCallExpression(child)) return undefined
      return ts.isIdentifier(child.expression) && child.expression.text === 'setError'
    }).some(Boolean)
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

function interfacePropertyNames(text: string, interfaceName: string) {
  const names = new Set<string>()
  collect(sourceFile('source.ts', text), (node) => {
    if (!ts.isInterfaceDeclaration(node) || node.name.text !== interfaceName) return undefined
    for (const member of node.members) {
      if (!ts.isPropertySignature(member) || !member.name) continue
      if (ts.isIdentifier(member.name) || ts.isStringLiteral(member.name)) {
        names.add(member.name.text)
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
    assert.ok(hasObjectKeysLengthZeroReturn(configStoreText, 'remoteSettings'))
  })

  it('suppresses stale asynchronous window mode writes after rapid toggles', () => {
    const frameText = readSource('src', 'components', 'AppWindowFrame.vue')
    assert.ok(frameText.includes('windowModeRequest'))
    assert.ok(frameText.includes('resizeForMode'))
    assert.ok(frameText.includes('transitionPhase'))
    assert.ok(frameText.includes('native-resize-before-animation'))
    assert.ok(frameText.includes('native-resize-after-animation'))
    assert.ok(frameText.includes('settling'))
    assert.ok(frameText.includes('MODE_TRANSITION_MS'))
    assert.ok(frameText.includes('POST_RESIZE_SETTLE_MS'))
  })
})

describe('connection failure presentation contract', () => {
  it('maps asynchronous failed status snapshots into the user-facing error dialog', () => {
    const statusFields = interfacePropertyNames(vpnStoreText, 'VpnStatus')
    assert.ok(statusFields.has('error'))
    assert.ok(statusFields.has('error_code'))
    assert.ok(statusFields.has('error_recoverable'))
    assert.match(vpnStoreText, /function isTerminalConnectStatus\(nextStatus: VpnStatus\)/)
    assert.match(vpnStoreText, /connectInFlight\.value && isTerminalConnectStatus\(nextStatus\)/)
    assert.match(vpnStoreText, /nextStatus\.connected/)
    assert.match(vpnStoreText, /nextStatus\.error_code/)
    assert.match(vpnStoreText, /nextStatus\.phase === 'failed'/)
    assert.doesNotMatch(vpnStoreText, /connectInFlight\.value && \(nextStatus\.connected \|\| nextStatus\.process_running === false\)/)
    assert.match(vpnStoreText, /nextStatus\.error_code[\s\S]*setError\(normalizeError/)
    assert.match(vpnStoreText, /lastFailedConnectMode\.value = 'helper'/)
  })

  it('keeps backend connection failures visible while suppressing user-cancelled errors', () => {
    const errorTypes = unionStringMembers(vpnStoreText, 'VpnErrorType')
    assert.ok(errorTypes.has('connection_failed'))
    assert.ok(errorTypes.has('connection_attempt_active'))
    assert.ok(errorTypes.has('user_cancelled'))

    const contractErrors = objectLiteralPropertyNames(vpnStoreText, 'contractErrorMap')
    assert.ok(contractErrors.has('connection_failed'))
    assert.ok(contractErrors.has('connection_attempt_active'))

    assert.ok(hasGuardedSetError(vpnStoreText, 'user_cancelled'))
  })

  it('routes the in-progress yellow button to cancellation instead of a second connect', () => {
    assert.ok(hasSwitchCase(vpnStoreText, 'elevated connecting'))
    assert.ok(hasCallNamed(vpnStoreText, 'cancelConnect'))
    assert.ok(hasApiPostToLiteral(vpnStoreText, '/disconnect'))

    const dashboardScript = vueScriptSetup(dashboardPageText)
    const minimalScript = vueScriptSetup(minimalModeViewText)
    assert.ok(stringLiterals(dashboardScript).includes('取消连接'))
    assert.ok(hasPropertyCall(dashboardScript, 'cancelConnect'))
    assert.ok(hasPropertyCall(minimalScript, 'cancelConnect'))
  })

  it('does not treat a hidden service install checkbox as an install request', () => {
    const dashboardScript = vueScriptSetup(dashboardPageText)
    assert.match(dashboardScript, /vpn\.connectFromDashboard\(\s*showInstallServiceChoice\.value && installServiceBeforeConnect\.value\s*\)/)

    const connectFromDashboardStart = vpnStoreText.indexOf('async function connectFromDashboard')
    assert.notEqual(connectFromDashboardStart, -1)
    const disconnectElevatedStart = vpnStoreText.indexOf('async function disconnectElevated', connectFromDashboardStart)
    assert.notEqual(disconnectElevatedStart, -1)
    const connectFromDashboardBlock = vpnStoreText.slice(connectFromDashboardStart, disconnectElevatedStart)

    assert.match(connectFromDashboardBlock, /const shouldInstallService = installServiceFirst && !serviceInstalled\.value && !serviceAvailable\.value/)
    assert.match(connectFromDashboardBlock, /if \(shouldInstallService\)/)
    assert.doesNotMatch(connectFromDashboardBlock, /if \(installServiceFirst\)/)
  })
})
