import { createRoot } from 'react-dom/client'
import './index.css'
import App from './App.tsx'
import { restoreCachedTheme } from './lib/theme'
import { restoreCachedLocale, initI18n } from './lib/i18n'

// 启动时同步恢复本地状态（零 IPC，微秒级）：主题 + locale。
// 与 lib/theme.ts 的 restoreCachedTheme 同范式——避免 main.tsx 等 get_config IPC
// resolve 才 render 导致截图窗口白屏 ~10-50ms。
restoreCachedTheme()
restoreCachedLocale()

// 先渲染：locale 已从 localStorage 同步恢复，首屏立即可见正确语言。
const root = createRoot(document.getElementById('root')!)
root.render(<App />)

// 后台 IPC 校正 locale（DB 改了语言时同步到前端），不阻塞渲染。
// 与缓存不一致时 setLocale 会触发 useT 订阅的组件重渲染。
initI18n().catch(() => {})
