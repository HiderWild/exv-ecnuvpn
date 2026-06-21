// Generated from distribution/ecnu.json.

export const distributionConfig = {
  id: 'ecnu',
  appName: 'EXV',
  brandSubtitle: 'for ECNU',
  author: 'HiderWild',
  repository: {
    label: 'HiderWild/exv-ecnuvpn',
    url: 'https://github.com/HiderWild/exv-ecnuvpn',
  },
  defaultVpnServer: 'vpn-cn.ecnu.edu.cn',
  vpnServers: [
    { label: 'ECNU CN', value: 'vpn-cn.ecnu.edu.cn' },
    { label: 'ECNU CT', value: 'vpn-ct.ecnu.edu.cn' },
    { label: 'ECNU LT', value: 'vpn-lt.ecnu.edu.cn' },
  ],
  defaultRoutes: [
    '49.52.4.0/25',
    '59.78.176.0/20',
    '59.78.199.0/21',
    '58.198.176.128/25',
    '219.228.60.69',
    '59.78.189.128/25',
    '219.228.63.0/21',
    '202.120.80.0/20',
    '222.66.117.0/24',
  ],
  defaultUserAgents: {
    windows: 'AnyConnect Win_x86_64 4.10.05095',
    macos: 'AnyConnect Darwin_x86_64 4.10.05095',
    linux: 'AnyConnect Linux_x86_64 4.10.05095',
  },
} as const

export type DistributionConfig = typeof distributionConfig
