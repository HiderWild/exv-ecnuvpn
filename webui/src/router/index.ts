import { createRouter, createWebHashHistory } from 'vue-router'

const router = createRouter({
  history: createWebHashHistory(),
  routes: [
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
      path: '/logs',
      name: 'logs',
      component: () => import('../pages/LogsPage.vue'),
    },
    {
      path: '/settings',
      name: 'settings',
      component: () => import('../pages/SettingsPage.vue'),
    },
  ],
})

export default router
