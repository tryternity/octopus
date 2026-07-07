import { createRoot } from 'react-dom/client'
import './index.css'
import App from './App.tsx'
import { restoreCachedTheme } from './lib/theme'

// 从 localStorage 同步恢复主题（零 IPC，微秒级）
restoreCachedTheme()

// 白名单：只有常规非透明窗口（settings/compact_editor）设 html 背景色防白屏。
// 所有透明窗口（result/clipboard/screenshot）不设——
// result/clipboard 靠 transparent:true + body transparent 实现穿透，
// screenshot 靠 React 组件画选区外遮罩（选区内全透明看桌面），任何 html/body 背景都会破坏。
try {
  const w = window as any
  const label = w.__TAURI_INTERNALS__?.metadata?.currentWindow?.label || ''
  if (label === 'settings_window' || label === 'compact_editor_window') {
    document.documentElement.style.backgroundColor = 'var(--color-background)'
  }
} catch {}

createRoot(document.getElementById('root')!).render(<App />)
