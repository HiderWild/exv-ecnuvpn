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
    fs.copyFileSync(sourcePath, targetPath)
  }
}

const exeCandidates = [
  process.env.EXV_PATH,
  ...(process.platform === 'win32'
  ? [
      path.join(root, 'build', 'Release', 'exv.exe'),
      path.join(root, 'build', 'exv.exe'),
      path.join(root, 'build-desktop', 'exv.exe'),
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

// On Windows the native exv binary depends on a handful of MinGW runtime DLLs.
// They live next to the build output, so we look in the same directories.
const MINGW_RUNTIME_DLLS = [
  'libgcc_s_seh-1.dll',
  'libstdc++-6.dll',
  'libwinpthread-1.dll',
]

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
