import { createRoot } from 'react-dom/client'
import './index.css'
import App from './App.tsx'
import { restoreCachedTheme } from './lib/theme'

// 从 localStorage 同步恢复主题（零 IPC，微秒级）
restoreCachedTheme()

// 按窗口 label 设背景色——此时 CSS 已加载（var 有值）+ 桥接层已就绪。
// 透明窗口（result/clipboard/screenshot）不设——它们靠 transparent:true
// + body transparent 实现穿透/遮罩，设 html 背景会"显形"。
// 非透明窗口（settings/compact_editor）设底色防 React 渲染前白屏。
try {
  const w = window as any
  const label = w.__TAURI_INTERNALS__?.metadata?.currentWindow?.label || ''
  if (typeof label === 'string') {
    if (label.startsWith('screenshot_')) {
      document.body.style.background = 'rgba(0,0,0,0.5)'
    } else if (label !== 'result_window' && label !== 'clipboard_window') {
      document.documentElement.style.backgroundColor = 'var(--color-background)'
    }
  }
} catch {}

createRoot(document.getElementById('root')!).render(<App />)
