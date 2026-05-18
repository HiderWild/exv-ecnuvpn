/*
 * Stage the native exv binary and its runtime dependencies into
 * webui/native/bin/ for electron-builder to pick up.
 *
 * The script is intentionally strict about WHAT it copies:
 *  - It does NOT copy libssl/libcrypto: we now use Windows BCrypt on Win32
 *    and CommonCrypto on macOS. Linux desktop bundles continue to ship
 *    OpenSSL because libopenconnect on Linux already provides its own.
 *  - It does NOT copy debug/dev artifacts like .lib, .pdb, .exp, .a.
 *  - It DOES copy wintun.dll and any TAP installer assets staged under
 *    runtime/<platform>-<arch>/.
 */
const fs = require('fs')
const path = require('path')

const root = path.resolve(__dirname, '..', '..')
const outDir = path.resolve(__dirname, '..', 'native', 'bin')
fs.rmSync(outDir, { recursive: true, force: true })
fs.mkdirSync(outDir, { recursive: true })

// Filenames we never want in the desktop bundle.
const DENY_EXACT = new Set([
  'libssl-3-x64.dll',
  'libssl-3.dll',
  'libcrypto-3-x64.dll',
  'libcrypto-3.dll',
])
const DENY_PATTERNS = [/^libssl-/i, /^libcrypto-/i]
const DENY_EXTS = new Set(['.lib', '.pdb', '.exp', '.a', '.map'])
const MINGW_RUNTIME_DLLS = [
  'libgcc_s_seh-1.dll',
  'libstdc++-6.dll',
  'libwinpthread-1.dll',
]

function isAllowed(name) {
  if (DENY_EXACT.has(name)) return false
  if (DENY_EXTS.has(path.extname(name).toLowerCase())) return false
  for (const pattern of DENY_PATTERNS) {
    if (pattern.test(name)) return false
  }
  return true
}

function copyRecursive(source, target) {
  fs.mkdirSync(target, { recursive: true })
  for (const entry of fs.readdirSync(source, { withFileTypes: true })) {
    const sourcePath = path.join(source, entry.name)
    const targetPath = path.join(target, entry.name)
    if (entry.isDirectory()) {
      copyRecursive(sourcePath, targetPath)
      continue
    }
    if (!isAllowed(entry.name)) {
      console.log(`Skipping ${entry.name}`)
      continue
    }
    if (MINGW_RUNTIME_DLLS.includes(entry.name) && fs.existsSync(targetPath)) {
      console.log(`Keeping staged native runtime DLL: ${entry.name}`)
      continue
    }
    fs.copyFileSync(sourcePath, targetPath)
  }
}

const exeCandidates = [
  process.env.EXV_PATH,
  ...(process.platform === 'win32'
  ? [
      path.join(root, 'build', 'exv.exe'),
      path.join(root, 'build', 'Release', 'exv.exe'),
      path.join(root, 'build-desktop', 'exv.exe'),
      path.join(root, 'build-desktop', 'Release', 'exv.exe'),
    ]
  : [
      path.join(root, 'build', 'exv'),
      path.join(root, 'build-desktop', 'exv'),
    ]),
].filter(Boolean)

const exeSource = exeCandidates.find((candidate) => fs.existsSync(candidate))
if (!exeSource) {
  throw new Error(
    `Native exv binary not found. Build it first with CMake. Checked:\n  - ${exeCandidates.join('\n  - ')}`,
  )
}

const target = path.join(outDir, process.platform === 'win32' ? 'exv.exe' : 'exv')
fs.copyFileSync(exeSource, target)
if (process.platform !== 'win32') {
  fs.chmodSync(target, 0o755)
}
console.log(`Copied native binary: ${exeSource} -> ${target}`)

if (process.platform === 'win32') {
  const helperCandidates = [
    process.env.EXV_HELPER_PATH,
    path.join(path.dirname(exeSource), 'exv-helper.exe'),
    path.join(root, 'build', 'exv-helper.exe'),
    path.join(root, 'build', 'Release', 'exv-helper.exe'),
    path.join(root, 'build-desktop', 'exv-helper.exe'),
    path.join(root, 'build-desktop', 'Release', 'exv-helper.exe'),
  ].filter(Boolean)
  const helperSource = helperCandidates.find((candidate) => fs.existsSync(candidate))
  if (!helperSource) {
    throw new Error(
      `Native exv-helper binary not found. Build it first with CMake. Checked:\n  - ${helperCandidates.join('\n  - ')}`,
    )
  }
  fs.copyFileSync(helperSource, path.join(outDir, 'exv-helper.exe'))
  console.log(`Copied native helper: ${helperSource} -> ${path.join(outDir, 'exv-helper.exe')}`)
}

if (process.platform === 'win32') {
  const exeDir = path.dirname(exeSource)
  for (const dll of MINGW_RUNTIME_DLLS) {
    const source = path.join(exeDir, dll)
    if (fs.existsSync(source)) {
      fs.copyFileSync(source, path.join(outDir, dll))
      console.log(`Copied runtime DLL: ${dll}`)
    }
  }
}

const runtimeCandidates = [
  process.env.ECNUVPN_RUNTIME_DIR,
  path.join(root, 'runtime', `${process.platform}-${process.arch}`),
  path.join(root, 'runtime', process.platform),
].filter(Boolean)

const runtimeSource = runtimeCandidates.find((candidate) => fs.existsSync(candidate))
if (runtimeSource) {
  copyRecursive(runtimeSource, outDir)
  const bundledOpenconnect = path.join(
    outDir,
    process.platform === 'win32' ? 'openconnect.exe' : 'openconnect',
  )
  if (process.platform !== 'win32' && fs.existsSync(bundledOpenconnect)) {
    fs.chmodSync(bundledOpenconnect, 0o755)
  }
  console.log(`Copied bundled runtime assets: ${runtimeSource} -> ${outDir}`)
} else {
  console.warn(
    `No bundled runtime assets found. Checked:\n  - ${runtimeCandidates.join('\n  - ')}`,
  )
}

// ---------------------------------------------------------------------------
// Post-copy validation: ensure critical runtime files are present before
// packaging. Missing files result in clear warnings so developers know the
// packaged app will not be fully functional.
// ---------------------------------------------------------------------------

const nativeExeName = process.platform === 'win32' ? 'exv.exe' : 'exv'
const nativeExePath = path.join(outDir, nativeExeName)
if (!fs.existsSync(nativeExePath)) {
  // This should never happen — we copied it above — but guard anyway.
  throw new Error(
    `Validation failed: native binary not found at ${nativeExePath}. ` +
    'The desktop app will not work without it.',
  )
}
console.log(`[validation] Native binary present: ${nativeExeName}`)

if (process.platform === 'win32') {
  const openconnectPath = path.join(outDir, 'openconnect.exe')
  if (!fs.existsSync(openconnectPath)) {
    console.warn(
      '[validation] WARNING: openconnect.exe not found in output directory.\n' +
      '  The packaged desktop app will NOT be able to connect to VPN without\n' +
      '  the bundled OpenConnect runtime. Stage it first with:\n' +
      '    powershell -File scripts/stage-openconnect-runtime-win.ps1 -SourceDir <dir>',
    )
  } else {
    console.log('[validation] OpenConnect runtime present: openconnect.exe')
  }

  const wintunPath = path.join(outDir, 'wintun.dll')
  if (!fs.existsSync(wintunPath)) {
    console.warn(
      '[validation] WARNING: wintun.dll not found in output directory.\n' +
      '  Wintun tunnel mode will not work in the packaged app.\n' +
      '  Provide it via: stage-openconnect-runtime-win.ps1 -WintunDllPath <path>',
    )
  } else {
    console.log('[validation] Wintun DLL present: wintun.dll')
  }
}
