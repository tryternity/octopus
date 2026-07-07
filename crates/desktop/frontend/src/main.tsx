import { createRoot } from 'react-dom/client'
import './index.css'
import App from './App.tsx'
import { restoreCachedTheme } from './lib/theme'

// 从 localStorage 同步恢复主题（零 IPC，微秒级）
restoreCachedTheme()

// 背景色已由 index.html <head> 脚本从 URL bg 参数注入（裸 hex，零 CSS 依赖）。
// 不在此处设背景色——避免与 index.html 逻辑冲突。

createRoot(document.getElementById('root')!).render(<App />)
