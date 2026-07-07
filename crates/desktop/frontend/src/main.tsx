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
  // 白名单：只有常规非透明窗口设背景色。透明窗口（result/clipboard/screenshot）
  // 和 label 为空（桥接层未就绪）都不设——宁可白屏也不破坏透明穿透。
  if (label === 'settings_window' || label === 'compact_editor_window') {
    document.documentElement.style.backgroundColor = 'var(--color-background)'
  } else if (label.startsWith('screenshot_')) {
    document.body.style.background = 'rgba(0,0,0,0.5)'
  }
} catch {}

createRoot(document.getElementById('root')!).render(<App />)
