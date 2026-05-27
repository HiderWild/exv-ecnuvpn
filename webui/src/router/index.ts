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
      redirect: { path: '/settings', query: { section: 'auth' } },
    },
    {
      path: '/routes',
      redirect: { path: '/settings', query: { section: 'routes' } },
    },
    {
      path: '/service',
      redirect: { path: '/settings', query: { section: 'system' } },
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
