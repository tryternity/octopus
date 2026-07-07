import { createRoot } from 'react-dom/client'
import './index.css'
import App from './App.tsx'
import { applyThemeFromConfig } from './lib/theme'

// 截图窗口提前设暗色背景（在 React render 前执行，避免白屏闪烁）
try {
  const w = window as any
  const label = w.__TAURI_INTERNALS__?.metadata?.currentWindow?.label || ''
  if (typeof label === 'string' && label.startsWith('screenshot_')) {
    document.body.style.background = 'rgba(0,0,0,0.5)'
  }
} catch {}

// 主题在 render 前异步加载——第一次 render 可能是默认 light，加载后 CSS 变量切换、
// React 下次重渲染即跟随。透明窗口在此期间背景透明，不会白闪。
applyThemeFromConfig()

createRoot(document.getElementById('root')!).render(<App />)
