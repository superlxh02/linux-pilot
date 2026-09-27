import { createApp } from 'vue'
import { createPinia } from 'pinia'
import { createRouter, createWebHistory } from 'vue-router'
import App from './App.vue'
import './style.css'

const router = createRouter({
  history: createWebHistory(),
  routes: [
    { path: '/', redirect: '/k8s' },
    { path: '/login', component: () => import('./pages/OverviewPage.vue') },
    { path: '/overview', component: () => import('./pages/OverviewPage.vue') },
    { path: '/hosts', component: () => import('./pages/HostsPage.vue') },
    { path: '/cluster', redirect: '/k8s' },
    { path: '/k8s', component: () => import('./pages/ClusterPage.vue') },
    { path: '/services', component: () => import('./pages/ServicesPage.vue') },
    { path: '/nodes', component: () => import('./pages/HostsPage.vue') },
    { path: '/node-overview', component: () => import('./pages/OverviewPage.vue') },
    { path: '/standalone', component: () => import('./pages/StandalonePage.vue') },
    { path: '/topology', component: () => import('./pages/TopologyConfigPage.vue') },
    { path: '/metrics', component: () => import('./pages/MetricsPage.vue') },
    { path: '/processes', component: () => import('./pages/ProcessesPage.vue') },
    { path: '/scores', component: () => import('./pages/ScoresPage.vue') },
    { path: '/profiles', component: () => import('./pages/ProfilesPage.vue') },
    { path: '/alerts', component: () => import('./pages/AlertsPage.vue') },
    { path: '/users', component: () => import('./pages/UsersPage.vue') }
  ]
})

router.beforeEach((to) => {
  const mode = localStorage.getItem('pilot-deployment-mode') === 'standalone' ? 'standalone' : 'kubernetes'
  if (mode === 'kubernetes' && ['/processes', '/scores', '/profiles', '/hosts', '/overview'].includes(to.path)) return '/k8s'
  if (mode === 'standalone' && ['/k8s', '/services', '/nodes', '/node-overview'].includes(to.path)) return '/standalone'
})

createApp(App)
  .use(createPinia())
  .use(router)
  .mount('#app')
