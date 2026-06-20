import type { RouteRecordRaw } from 'vue-router'

export const routeRecords: RouteRecordRaw[] = [
  {
    path: '/',
    name: 'dashboard',
    component: () => import('../pages/DashboardPage.vue'),
  },
  {
    path: '/auth',
    redirect: { path: '/settings', hash: '#settings-auth' },
  },
  {
    path: '/routes',
    redirect: { path: '/settings', hash: '#settings-routes' },
  },
  {
    path: '/service',
    redirect: { path: '/settings', hash: '#settings-system' },
  },
  {
    path: '/connection',
    redirect: { path: '/settings', hash: '#settings-connection' },
  },
  {
    path: '/logs',
    name: 'logs',
    component: () => import('../pages/LogsPage.vue'),
  },
  {
    path: '/about',
    name: 'about',
    component: () => import('../pages/AboutPage.vue'),
  },
  {
    path: '/settings',
    name: 'settings',
    component: () => import('../pages/SettingsPage.vue'),
  },
  {
    path: '/modal/service-install',
    name: 'modal-service-install',
    component: () => import('../pages/ServiceInstallPromptModal.vue'),
  },
  {
    path: '/modal/password',
    name: 'modal-password',
    component: () => import('../pages/ServiceInstallPromptModal.vue'),
  },
  {
    path: '/modal/confirm',
    name: 'modal-confirm',
    component: () => import('../pages/ServiceInstallPromptModal.vue'),
  },
  {
    path: '/modal/close-app',
    name: 'modal-close-app',
    component: () => import('../pages/ServiceInstallPromptModal.vue'),
  },
]
