const path = require('node:path')

function normalizeBuildPlatform(input = process.env.ECNUVPN_BUILD_PLATFORM || process.platform) {
  switch ((input || '').toLowerCase()) {
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
      return input || process.platform
  }
}

function getBuildLayout() {
  const repoRoot = path.resolve(__dirname, '..', '..')
  const webuiRoot = path.resolve(__dirname, '..')
  const buildPlatform = normalizeBuildPlatform()
  const buildRoot = path.join(repoRoot, 'build', buildPlatform)
  const electronRoot = path.join(buildRoot, 'electron')

  return {
    repoRoot,
    webuiRoot,
    buildPlatform,
    buildRoot,
    cppBuildDir: path.join(buildRoot, 'cpp'),
    electronRoot,
    rendererOutDir: path.join(electronRoot, 'dist'),
    electronOutDir: path.join(electronRoot, 'dist-electron'),
    nativeBinDir: path.join(electronRoot, 'native', 'bin'),
    releaseDir: path.join(electronRoot, 'release'),
  }
}

module.exports = {
  getBuildLayout,
  normalizeBuildPlatform,
}