const fs = require('node:fs')
const path = require('node:path')

const { getBuildLayout } = require('./scripts/build-layout.cjs')

const layout = getBuildLayout()
const buildResourcesDir = path.join(__dirname, 'build-resources')
const rendererIndex = path.join(layout.rendererOutDir, 'index.html')
const electronMain = path.join(layout.electronOutDir, 'main', 'index.js')
const nativeExe = path.join(
  layout.nativeBinDir,
  process.platform === 'win32' ? 'exv.exe' : 'exv',
)

function relativeToWebui(target) {
  return path.relative(__dirname, target).split(path.sep).join('/')
}

function buildResource(name) {
  return path.posix.join('build-resources', name)
}

function hasBuildResource(name) {
  return fs.existsSync(path.join(buildResourcesDir, name))
}

for (const [label, file] of [
  ['renderer bundle', rendererIndex],
  ['Electron main bundle', electronMain],
  ['native exv binary', nativeExe],
]) {
  if (!fs.existsSync(file)) {
    throw new Error(
      `Missing ${label}: ${file}. Run the platform build script or npm run build, npm run build:electron, and npm run prepare:native before packaging.`,
    )
  }
}

const installerInclude = hasBuildResource('installer.nsh')
  ? buildResource('installer.nsh')
  : undefined
const macIcon = hasBuildResource('icon.icns')
  ? buildResource('icon.icns')
  : undefined
const winIcon = hasBuildResource('icon.ico')
  ? buildResource('icon.ico')
  : undefined
const pngIcon = hasBuildResource('icon.png')
  ? buildResource('icon.png')
  : undefined
const macEntitlements = hasBuildResource('entitlements.mac.plist')
  ? buildResource('entitlements.mac.plist')
  : undefined
const macEntitlementsInherit = hasBuildResource('entitlements.mac.inherit.plist')
  ? buildResource('entitlements.mac.inherit.plist')
  : undefined

module.exports = {
  appId: 'cn.edu.ecnu.vpn',
  productName: 'ECNU VPN',
  directories: {
    output: relativeToWebui(layout.releaseDir),
    buildResources: 'build-resources',
  },
  asar: true,
  compression: 'maximum',
  files: [
    'package.json',
    {
      from: relativeToWebui(layout.rendererOutDir),
      to: 'dist',
      filter: ['**/*'],
    },
    {
      from: relativeToWebui(layout.electronOutDir),
      to: 'dist-electron',
      filter: ['**/*'],
    },
  ],
  extraResources: [
    {
      from: relativeToWebui(layout.nativeBinDir),
      to: 'bin',
      filter: [
        '**/*',
        '!*.lib',
        '!*.pdb',
        '!*.exp',
        '!*.a',
        '!libssl-*.dll',
        '!libcrypto-*.dll',
      ],
    },
    ...(pngIcon
      ? [{
          from: pngIcon,
          to: 'icon.png',
        }]
      : []),
  ],
  win: {
    target: [
      {
        target: 'nsis',
        arch: ['x64'],
      },
      {
        target: 'portable',
        arch: ['x64'],
      },
    ],
    executableName: 'ECNU-VPN',
    signAndEditExecutable: false,
    ...(winIcon ? { icon: winIcon } : {}),
  },
  nsis: {
    oneClick: false,
    perMachine: true,
    allowToChangeInstallationDirectory: true,
    createDesktopShortcut: true,
    createStartMenuShortcut: true,
    shortcutName: 'ECNU VPN',
    deleteAppDataOnUninstall: false,
    ...(winIcon ? { installerIcon: winIcon, uninstallerIcon: winIcon } : {}),
    ...(installerInclude ? { include: installerInclude } : {}),
  },
  portable: {
    artifactName: 'ECNU-VPN-${version}-portable.${ext}',
  },
  mac: {
    target: [
      {
        target: 'dmg',
        arch: ['x64', 'arm64'],
      },
    ],
    category: 'public.app-category.utilities',
    hardenedRuntime: true,
    gatekeeperAssess: false,
    artifactName: 'ECNU-VPN-${version}-mac-${arch}.${ext}',
    ...(macIcon ? { icon: macIcon } : {}),
    ...(macEntitlements ? { entitlements: macEntitlements } : {}),
    ...(macEntitlementsInherit
      ? { entitlementsInherit: macEntitlementsInherit }
      : {}),
  },
  dmg: {
    contents: [
      { x: 130, y: 220 },
      { x: 410, y: 220, type: 'link', path: '/Applications' },
    ],
  },
}
