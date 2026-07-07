import { createRoot } from 'react-dom/client'
import './index.css'
import App from './App.tsx'
import { restoreCachedTheme } from './lib/theme'

// 从 localStorage 同步恢复主题（零 IPC，微秒级）
restoreCachedTheme()

// 设置非透明窗口的背景底色——main.tsx 执行时 __TAURI_INTERNALS__ 已就绪
// （index.html <head> 脚本里读不到，因为 Tauri 桥接层此时尚未注入）。
// 防 React 渲染前的白屏：body 是 transparent，必须给 html 设底色。
try {
  const w = window as any
  const label = w.__TAURI_INTERNALS__?.metadata?.currentWindow?.label || ''
  if (typeof label === 'string') {
    if (label.startsWith('screenshot_')) {
      document.body.style.background = 'rgba(0,0,0,0.5)'
    } else if (label !== 'result_window' && label !== 'clipboard_window') {
      // 非透明窗口（settings/compact_editor）设 html 背景色防白屏
      document.documentElement.style.backgroundColor = 'var(--color-background)'
    }
  }
} catch {}

createRoot(document.getElementById('root')!).render(<App />)
