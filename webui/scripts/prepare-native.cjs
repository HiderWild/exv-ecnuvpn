/*
 * Stage the native exv binary and its runtime dependencies into
 * webui/native/bin/ for electron-builder to pick up.
 *
 * The script is intentionally strict about WHAT it copies:
 *  - It does NOT copy libssl/libcrypto: we now use Windows BCrypt on Win32
 *    and CommonCrypto on macOS.
 *  - It does NOT copy OpenConnect/GnuTLS assets for production packaging.
 *    That runtime is legacy diagnostic-only and must be explicitly gated.
 *  - It does NOT copy debug/dev artifacts like .lib, .pdb, .exp, .a.
 *  - It DOES copy allowed native runtime assets such as wintun.dll.
 */
const fs = require('fs')
const path = require('path')

const { getBuildLayout } = require('./build-layout.cjs')

const root = path.resolve(__dirname, '..', '..')
const layout = getBuildLayout()
const outDir = layout.nativeBinDir
const LEGACY_OPENCONNECT_ENV = 'ECNUVPN_LEGACY_OPENCONNECT_RUNTIME'
const LEGACY_OPENCONNECT_RUNTIME_DIR_ENV = 'ECNUVPN_LEGACY_OPENCONNECT_RUNTIME_DIR'
const legacyOpenconnectRuntime = process.env[LEGACY_OPENCONNECT_ENV] === '1'
const productionRuntimeCandidates = [
  process.env.ECNUVPN_RUNTIME_DIR,
  path.join(root, 'runtime', `${process.platform}-${process.arch}`),
  path.join(root, 'runtime', process.platform),
].filter(Boolean)
const legacyOpenconnectRuntimeCandidates = [
  process.env[LEGACY_OPENCONNECT_RUNTIME_DIR_ENV],
  path.join(root, 'runtime', 'legacy-openconnect', `${process.platform}-${process.arch}`),
  path.join(root, 'runtime', 'legacy-openconnect', process.platform),
].filter(Boolean)

function sleepSync(ms) {
  const signal = new Int32Array(new SharedArrayBuffer(4))
  Atomics.wait(signal, 0, 0, ms)
}

function resetOutputDirectory(target) {
  if (!fs.existsSync(target)) {
    return
  }

  let removeError = null
  for (let attempt = 0; attempt < 4; attempt += 1) {
    try {
      fs.rmSync(target, { recursive: true, force: true })
      return
    } catch (error) {
      removeError = error
      if (!['EPERM', 'EBUSY', 'ENOTEMPTY'].includes(error.code) || attempt === 3) {
        break
      }
      sleepSync(150)
    }
  }

  const parent = path.dirname(target)
  const base = path.basename(target)
  let renameError = null
  for (let attempt = 0; attempt < 4; attempt += 1) {
    const lockedTarget = path.join(parent, `${base}.locked-${Date.now()}-${process.pid}-${attempt}`)
    try {
      fs.renameSync(target, lockedTarget)
      console.warn(
        `Native staging directory was locked; moved it aside for a clean stage: ${lockedTarget}`,
      )
      return
    } catch (error) {
      renameError = error
      sleepSync(150)
    }
  }

  throw new Error(
    [
      `Unable to reset native staging directory: ${target}`,
      `Remove failed: ${removeError ? removeError.message : 'not attempted'}`,
      `Rename fallback failed: ${renameError ? renameError.message : 'not attempted'}`,
      'Stop any running exv-helper/exv/Electron process that uses this build directory and retry.',
    ].join('\n'),
  )
}

resetOutputDirectory(outDir)
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
const ALLOWED_NATIVE_RUNTIME_ASSETS = new Set(['wintun.dll'])

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

function copyLegacyRuntimeAssets(source, target) {
  console.warn(
    `[legacy diagnostic] ${LEGACY_OPENCONNECT_ENV}=1; copying legacy OpenConnect runtime assets from ${source}`,
  )
  copyRecursive(source, target)
  const bundledOpenconnect = path.join(
    target,
    process.platform === 'win32' ? 'openconnect.exe' : 'openconnect',
  )
  if (process.platform !== 'win32' && fs.existsSync(bundledOpenconnect)) {
    fs.chmodSync(bundledOpenconnect, 0o755)
  }
}

function copyAllowedNativeRuntimeAssets(source, target) {
  const copied = []
  for (const assetName of ALLOWED_NATIVE_RUNTIME_ASSETS) {
    const sourcePath = path.join(source, assetName)
    if (!fs.existsSync(sourcePath)) {
      continue
    }

    const targetPath = path.join(target, assetName)
    fs.copyFileSync(sourcePath, targetPath)
    copied.push(assetName)
  }

  if (copied.length > 0) {
    console.log(`Copied native runtime asset(s): ${copied.join(', ')} from ${source}`)
  } else {
    console.log(`No allowed native runtime assets found in ${source}`)
  }
}

const exeCandidates = [
  process.env.EXV_PATH,
  ...(process.platform === 'win32'
  ? [
      path.join(layout.cppBuildDir, 'exv.exe'),
      path.join(layout.cppBuildDir, 'Release', 'exv.exe'),
      path.join(root, 'build', 'exv.exe'),
      path.join(root, 'build', 'Release', 'exv.exe'),
      path.join(root, 'build-desktop', 'exv.exe'),
      path.join(root, 'build-desktop', 'Release', 'exv.exe'),
    ]
  : [
      path.join(layout.cppBuildDir, 'exv'),
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

if (process.platform === 'win32' || process.platform === 'darwin') {
  const helperName = process.platform === 'win32' ? 'exv-helper.exe' : 'exv-helper'
  const helperCandidates = [
    process.env.EXV_HELPER_PATH,
    path.join(layout.cppBuildDir, helperName),
    path.join(layout.cppBuildDir, 'Release', helperName),
    path.join(path.dirname(exeSource), helperName),
    path.join(root, 'build', helperName),
    path.join(root, 'build', 'Release', helperName),
    path.join(root, 'build-desktop', helperName),
    path.join(root, 'build-desktop', 'Release', helperName),
  ].filter(Boolean)

  const helperSource = helperCandidates.find((candidate) => fs.existsSync(candidate))
  if (!helperSource) {
    throw new Error(
      `Native exv-helper binary not found. Build it first with CMake. Checked:\n  - ${helperCandidates.join('\n  - ')}`,
    )
  }

  const helperTarget = path.join(outDir, helperName)
  fs.copyFileSync(helperSource, helperTarget)
  if (process.platform !== 'win32') {
    fs.chmodSync(helperTarget, 0o755)
  }
  console.log(`Copied native helper: ${helperSource} -> ${helperTarget}`)
}

// On Windows the native exv binary depends on a handful of MinGW runtime DLLs.
// They live next to the build output, so we look in the same directories.
if (process.platform === 'win32') {
  const exeDir = path.dirname(exeSource)
  const runtimeSearchDirs = [
    exeDir,
    ...process.env.PATH.split(path.delimiter),
    path.join(root, 'build'),
    path.join(root, 'build', 'Release'),
    path.join(root, 'build-desktop'),
    path.join(root, 'build-desktop', 'Release'),
  ]
  const uniqueRuntimeSearchDirs = [...new Set(runtimeSearchDirs.filter(Boolean))]
  const missingRuntimeDlls = []

  for (const dll of MINGW_RUNTIME_DLLS) {
    const source = uniqueRuntimeSearchDirs
      .map((dir) => path.join(dir, dll))
      .find((candidate) => fs.existsSync(candidate))
    if (source) {
      fs.copyFileSync(source, path.join(outDir, dll))
      console.log(`Copied runtime DLL: ${dll}`)
    } else {
      missingRuntimeDlls.push(dll)
    }
  }

  if (missingRuntimeDlls.length > 0) {
    throw new Error(
      `Missing MinGW runtime DLL(s): ${missingRuntimeDlls.join(', ')}. ` +
      `Checked:\n  - ${uniqueRuntimeSearchDirs.join('\n  - ')}`,
    )
  }
}

const productionRuntimeSource = productionRuntimeCandidates.find((candidate) => fs.existsSync(candidate))
if (!productionRuntimeSource) {
  console.warn(
    `No optional native runtime assets found. Checked:\n  - ${productionRuntimeCandidates.join('\n  - ')}`,
  )
} else {
  copyAllowedNativeRuntimeAssets(productionRuntimeSource, outDir)
}

if (legacyOpenconnectRuntime) {
  const legacyRuntimeSource = legacyOpenconnectRuntimeCandidates.find((candidate) =>
    fs.existsSync(candidate),
  )
  if (!legacyRuntimeSource) {
    throw new Error(
      `[legacy diagnostic] ${LEGACY_OPENCONNECT_ENV}=1 but no legacy OpenConnect runtime assets were found. ` +
      `Checked:\n  - ${legacyOpenconnectRuntimeCandidates.join('\n  - ')}`,
    )
  }

  copyLegacyRuntimeAssets(legacyRuntimeSource, outDir)
  console.log(`Copied legacy diagnostic runtime assets: ${legacyRuntimeSource} -> ${outDir}`)
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
  const wintunPath = path.join(outDir, 'wintun.dll')
  if (!fs.existsSync(wintunPath)) {
    console.warn(
      '[validation] WARNING: wintun.dll not found in output directory.\n' +
      '  Wintun tunnel mode will not work in the packaged app.\n' +
      '  Provide it via ECNUVPN_RUNTIME_DIR or runtime/win32-x64/wintun.dll.',
    )
  } else {
    console.log('[validation] Wintun DLL present: wintun.dll')
  }
}
