/* 官网只做导航、链接和命令复制；页面内容无需外部 API，GitHub Pages 可直接托管。 */
const menuButton = document.querySelector('.menu-button')
const navigation = document.querySelector('#site-navigation')

menuButton?.addEventListener('click', () => {
  const expanded = menuButton.getAttribute('aria-expanded') === 'true'
  menuButton.setAttribute('aria-expanded', String(!expanded))
  navigation?.classList.toggle('open', !expanded)
})

navigation?.querySelectorAll('a').forEach((link) => {
  link.addEventListener('click', () => {
    navigation.classList.remove('open')
    menuButton?.setAttribute('aria-expanded', 'false')
  })
})

// 仓库地址由 Actions 的 github.repository 注入；本地预览保留页内锚点。
const repository = window.LINUX_PILOT_REPOSITORY
if (typeof repository === 'string' && /^https:\/\/github\.com\/[\w.-]+\/[\w.-]+$/.test(repository)) {
  document.querySelectorAll('.repo-link').forEach((link) => {
    link.setAttribute('href', repository)
    link.setAttribute('target', '_blank')
    link.setAttribute('rel', 'noopener noreferrer')
  })
}

document.querySelectorAll('.copy-button').forEach((button) => {
  button.addEventListener('click', async () => {
    try {
      await navigator.clipboard.writeText(button.getAttribute('data-copy') || '')
      const original = button.textContent
      button.textContent = '已复制'
      window.setTimeout(() => { button.textContent = original }, 1800)
    } catch {
      button.textContent = '复制失败'
    }
  })
})
