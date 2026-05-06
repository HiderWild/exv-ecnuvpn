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
      name: 'auth',
      component: () => import('../pages/AuthPage.vue'),
    },
    {
      path: '/routes',
      name: 'routes',
      component: () => import('../pages/RoutesPage.vue'),
    },
    {
      path: '/service',
      name: 'service',
      component: () => import('../pages/ServicePage.vue'),
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
