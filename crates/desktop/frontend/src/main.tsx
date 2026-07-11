import { createRoot } from 'react-dom/client'
import './index.css'
import App from './App.tsx'
import { restoreCachedTheme } from './lib/theme'
import { initI18n } from './lib/i18n'

// 从 localStorage 同步恢复主题（零 IPC，微秒级）
restoreCachedTheme()

// 初始化 i18n（从后端 config 读 ui_language），完成后渲染
initI18n().finally(() => {
  createRoot(document.getElementById('root')!).render(<App />)
})
