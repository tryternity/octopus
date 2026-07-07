import { createRoot } from 'react-dom/client'
import './index.css'
import App from './App.tsx'
import { restoreCachedTheme } from './lib/theme'

// 从 localStorage 同步恢复主题（零 IPC，微秒级）
restoreCachedTheme()

// 截图窗口设半透明黑底（遮罩层）——其余窗口的背景色已由 index.html 无条件设置
try {
  const w = window as any
  const label = w.__TAURI_INTERNALS__?.metadata?.currentWindow?.label || ''
  if (typeof label === 'string' && label.startsWith('screenshot_')) {
    document.body.style.background = 'rgba(0,0,0,0.5)'
  }
} catch {}

createRoot(document.getElementById('root')!).render(<App />)
