import { createRoot } from 'react-dom/client'
import './index.css'
import App from './App.tsx'

// 截图窗口提前设暗色背景（在 React render 前执行，避免白屏闪烁）
try {
  const w = window as any
  const label = w.__TAURI_INTERNALS__?.metadata?.currentWindow?.label || ''
  if (typeof label === 'string' && label.startsWith('screenshot_')) {
    document.body.style.background = 'rgba(0,0,0,0.5)'
  }
} catch {}

createRoot(document.getElementById('root')!).render(<App />)
