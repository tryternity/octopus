# Launch 功能点详解与优先级排序

> 2026-07-15 · 基于 launch 功能深度调研，逐项分析每个功能点的作用、实现方案和优先级
> 关联文档：[`2026-07-15-launch-features-deep-survey.md`](./2026-07-15-launch-features-deep-survey.md)

---

## 优先级总览

| 优先级 | 功能点 | 核心价值 | 实现成本 |
|:------:|--------|---------|:--------:|
| **P0** | Silent Query Hotkey（静默热键直达） | 最高频场景一键执行 | 低 |
| **P0** | Run And Paste（AI 处理→原位粘贴） | 选中文本→AI→光标替换 | 低 |
| **P0** | 模糊搜索/拼音匹配优化 | action bar 搜索体验 | 低 |
| **P1** | Deep Link（`octopus://` URL Scheme） | 外部集成入口 | 低 |
| **P1** | 应用启动 | 搜索+启动已安装应用 | 中 |
| **P1** | Quicklinks（带变量快捷链接） | 快速打开带参数的 URL | 中 |
| **P1** | 计算器 | 输入即算 | 低 |
| **P1** | 单位/货币换算 | 日常实用 | 低 |
| **P2** | 文件搜索 | 全盘文件索引+搜索 | 高 |
| **P2** | 浏览器书签搜索 | 书签快速定位 | 中 |
| **P2** | 系统命令 | 关机/重启等 | 低 |
| **P2** | Shell/终端集成 | 终端预览+执行 | 中 |
| **P2** | Action Panel（二级操作面板） | 结果上 Tab 展开更多操作 | 中 |
| **P2** | AI Command Store（远程命令商店） | 社区共享 prompt 模板 | 中 |
| **P3** | Result Drag（拖拽导出） | 结果拖到文件夹/应用 | 低 |
| **P3** | Query Hotkey 多实例 | 不同热键唤起不同面板 | 中 |
| **P3** | Snippet 自动展开 | 打字关键词展开 | 高 |
| **P3** | 可视化 Workflow 编排 | 零代码拖拽编排 | 极高 |

---

## P0：高价值低成本（立即做）

### 1. Silent Query Hotkey（静默热键直达）

**作用**：选中文本后按一个热键（如 `⌘⇧T`），**不弹 action bar 面板**，直接执行绑定的命令（如翻译）。绕过"选中→唤起面板→选菜单→执行"四步，变成"选中→按热键"一步。

**典型场景**：
- `⌘⇧T` = 直接翻译选中文本 → 浮窗显示结果
- `⌘⇧P` = 直接润色 → 结果粘贴到光标
- `⌘⇧S` = 直接搜索 → 浏览器打开 Google

**实现方案**：
- octopus **已有** `action_bar_items.shortcut` 字段（DB v24 迁移加的）
- 当前 shortcut 字段已用于托盘/面板内的快捷键，但**未实现全局热键静默执行**
- 实现：注册全局热键时，检查菜单项是否有 shortcut → 有则注册 `GlobalShortcut`，handler 中直接执行该 action（不调 `show_action_bar`，而是 `trigger_action_bar` 的 silent 变体）
- 代码量：~100 行 Rust（热键注册 + silent handler）+ 前端 shortcut 编辑 UI 已有

**参考**：Wox 的 `Query Hotkey` + `AI Command silent mode`；VoxFlow 的 `⌘⇧J/K/L/P` 一键直达

---

### 2. Run And Paste（AI 处理→原位粘贴）

**作用**：选中文本 → AI 处理（翻译/润色/摘要）→ 结果**直接粘贴回原光标位置**，替换选中文本。无需手动复制结果。

**典型场景**：
- 选中英文 → `⌘⇧T` → 翻译后中文直接替换原英文
- 选中粗糙文本 → `⌘⇧P` → 润色后文本直接替换

**实现方案**：
- octopus **已有** action bar AI 命令（翻译/润色/摘要），当前结果在浮窗显示，用户需手动复制
- 增加 `paste_back` 模式：AI 返回结果后，模拟 `⌘V` 粘贴到原光标位置
- 实际上 octopus 的录音 paste 逻辑已经实现了"写入剪贴板 + 模拟粘贴"，可复用
- 代码量：~50 行（AI 结果写入剪贴板 + 触发 paste）

**参考**：Wox 的 `Run And Paste`（AI Command 的核心特性）

---

### 3. 模糊搜索/拼音匹配优化

**作用**：action bar 搜索框输入时，用模糊匹配 + 拼音首字母匹配菜单项。如输入 "fy" 匹配 "翻译"，输入 "rs" 匹配 "润色"。

**当前状态**：octopus action bar 的搜索是**前缀精确匹配**（`title.contains(query)`），无模糊/拼音

**实现方案**：
- 引入 fzf 算法或 `nucleo-matcher` crate（Rust 高性能模糊匹配库）
- 拼音：首字母索引（预计算菜单项的拼音首字母序列）
- 优先级：前缀匹配 > 拼音首字母 > 模糊匹配
- 代码量：~80 行（拼音预计算 + 匹配排序逻辑）

**参考**：Wox 的 fzf + 拼音 + 短文本对齐；Alfred 的缩写匹配（gc→Chrome）

---

## P1：中价值（第二批）

### 4. Deep Link（`octopus://` URL Scheme）

**作用**：其他应用/脚本/浏览器可以通过 URL Scheme 触发 octopus 命令。

**典型场景**：
- 浏览器书签 `octopus://translate?text=hello` → 直接翻译
- Shell 脚本 `open "octopus://polish?text=xxx"` → 自动润色
- Raycast/Alfred Workflow 调用 `octopus://` 做联动

**实现方案**：
- Tauri 2 原生支持 `tauri-plugin-deep-link`
- 注册 `octopus://` scheme（macOS Info.plist / Windows registry）
- 前端监听 deep-link 事件，解析 path + params，路由到对应 action
- 代码量：~60 行（scheme 注册 + 路由分发）

**参考**：Wox `wox://`；Alfred `alfred://`

---

### 5. 应用启动

**作用**：在 action bar 搜索框中输入应用名，直接启动应用（不限于 octopus 自己的菜单项）。

**实现方案**：
- macOS：扫描 `/Applications/`、`~/Applications/`、`/System/Applications/` 下的 `.app`
- 索引：应用名 + bundle name（本地化名）+ 图标缓存
- 输入 "chr" → 匹配 Google Chrome → 回车启动
- 代码量：~150 行（扫描 + 索引 + 搜索 + 图标提取）
- 可用 `core-foundation` + `objc` crate 访问 LaunchServices

**参考**：Wox 全平台应用索引；Alfred 缩写匹配（gc→Chrome）

---

### 6. Quicklinks（带变量快捷链接）

**作用**：预定义带 `{query}` 占位符的 URL 模板，输入关键词 + 内容直接在浏览器打开。

**典型场景**：
- 关键词 `gg` → `https://google.com/search?q={query}`
- 关键词 `tr` → `https://translate.google.com/?text={query}`
- 关键词 `gh` → `https://github.com/search?q={query}`

**与现有 action bar 的区别**：
- 现有 `url` 类型菜单项：选中文本 → action bar → 点菜单项 → 浏览器打开
- Quicklinks：**在搜索框直接输入** `tr hello` → 回车 → 浏览器打开翻译页面
- 更轻量，不需要选中操作

**实现方案**：
- DB `action_bar_items` 加 `trigger_keyword` 字段
- action bar 搜索框检测 `<keyword> <query>` 模式 → 匹配 Quicklink → 替换 `{query}` → 打开 URL
- 代码量：~40 行（关键词匹配 + URL 模板替换）

**参考**：Raycast Quicklinks（核心功能）；Alfred 自定义 Web Search

---

### 7. 计算器

**作用**：action bar 搜索框输入数学表达式 → 直接显示结果。

**典型场景**：
- 输入 `1+2*3` → 显示 `7`
- 输入 `(100-20)/4` → 显示 `20`

**实现方案**：
- 检测输入是否为合法数学表达式（regex 或 `evalexpr` crate）
- 在 action bar 结果列表顶部插入一个"计算结果"虚拟项
- 回车 → 复制结果到剪贴板
- 代码量：~30 行（表达式检测 + 计算）

**参考**：Wox（含历史、千分位、幂运算）；Alfred（`= ` 前缀 + 三角/对数）

---

### 8. 单位/货币换算

**作用**：输入 `100usd to cny` → 显示汇率换算结果。

**实现方案**：
- 正则匹配 `<数字><单位> to <单位>` 模式
- 长度/重量/温度/存储/时间：内置换算表（纯计算，无网络）
- 货币：调汇率 API（如 `exchangerate-api.com`），缓存 1 小时
- 代码量：~100 行（解析 + 换算表 + API 调用）

**参考**：Wox Converter（6 类单位 + 货币）

---

## P2：实用但非核心（按需做）

### 9. 文件搜索

**作用**：全盘文件搜索，输入文件名快速定位文件。

**实现方案**：
- macOS：用 `mdfind`（Spotlight metadata）做后端，无需自建索引
- 搜索结果在 action bar 列表中展示（文件名 + 路径 + 图标）
- 回车打开 / `⌘↵` 在 Finder 中显示
- 代码量：~200 行（mdfind 调用 + 结果格式化 + 图标缓存）
- 难点：性能控制（mdfind 可能返回大量结果）、权限

**参考**：Wox 原生数据库索引（不依赖 Spotlight）；Alfred File Search

---

### 10. 浏览器书签搜索

**作用**：搜索 Safari/Chrome/Edge/Firefox 书签。

**实现方案**：
- Safari：读 `~/Library/Safari/Bookmarks.plist`（需 Full Disk Access）
- Chrome/Edge：读 `~/Library/Application Support/Google/Chrome/Default/Bookmarks`（JSON）
- 索引书签标题 + URL，搜索后回车打开
- 代码量：~150 行（多浏览器解析 + 索引）

**参考**：Wox 支持 4 浏览器；Alfred 支持 Safari/Chrome（含阅读列表）

---

### 11. 系统命令

**作用**：在 action bar 中快速执行系统操作。

**实现方案**：
- macOS：`osascript -e 'tell app "System Events" to shut down'`
- 支持：关机/重启/休眠/锁屏/清空废纸篓/弹出磁盘/暗色模式切换
- 代码量：~50 行（AppleScript 执行 + 菜单项注册）

**参考**：Wox（关机/重启/休眠/音量/Task View）；Alfred（锁屏/睡眠/关机/重启/清回收站）

---

### 12. Shell/终端集成

**作用**：在 action bar 中输入 Shell 命令并执行。

**典型场景**：
- 输入 `> ls -la` → 执行并显示结果
- 输入 `> pip install xxx` → 后台执行

**实现方案**：
- 检测 `>` 前缀 → 后续作为 shell 命令
- `std::process::Command::new("sh").arg("-c").arg(cmd)` 执行
- 结果显示在 action bar 预览区
- 代码量：~80 行（命令解析 + 执行 + 结果展示）

**参考**：Wox（终端预览/搜索全屏/后台执行/history）；Alfred（`>` 执行）

---

### 13. Action Panel（二级操作面板）

**作用**：搜索结果上按 Tab 键展开二级操作菜单。

**典型场景**：
- 搜索到"翻译"结果 → 按 Tab → 展开子操作：翻译到中文/翻译到英文/翻译到日语/复制结果
- 搜索到文件 → 按 Tab → 展开：打开/复制路径/在终端打开/重命名

**实现方案**：
- action bar 前端监听 Tab 键 → 当前选中项展开二级面板
- 二级操作来源：菜单项 `action_bar_items` 的子项，或动态生成（如翻译的目标语言列表）
- 代码量：~120 行（Tab 监听 + 面板渲染 + 键盘导航）

**参考**：Wox Action Panel；Raycast Action Panel；Alfred File Actions

---

### 14. AI Command Store（远程命令商店）

**作用**：在线 prompt 模板库，用户一键安装。

**典型场景**：
- 打开商店 → 浏览"翻译/润色/摘要/解释/改写/语法修正"等模板
- 一键安装 → 自动创建 `action_bar_items` 行 + 配置 prompt
- 社区贡献：用户上传自己写的 prompt 模板

**实现方案**：
- 后端：GitHub raw / 简单 API 服务托管 JSON 模板列表
- 前端：action bar 设置页加"商店"入口，展示模板列表
- 安装：将模板 JSON 转为 `action_bar_items` INSERT
- 代码量：~200 行（远程拉取 + 模板渲染 + 一键安装）

**参考**：Wox AI Command Store（3 个在线商店之一）

---

## P3：远期/探索性

### 15. Result Drag（拖拽导出）

**作用**：action bar 结果项可直接拖拽到 Finder 文件夹或其他应用。

**实现方案**：
- 前端结果项加 `draggable` 属性
- 拖拽时设置剪贴板/拖拽板内容（文本/文件路径）
- 代码量：~30 行

**参考**：Wox Result Drag（原生文件拖拽）

---

### 16. Query Hotkey 多实例

**作用**：不同热键唤起不同位置/不同初始查询的 action bar。

**典型场景**：
- `⌘⇧Space` = 唤起标准 action bar（屏幕中央）
- `⌘⇧A` = 唤起 AI 专用面板（预填 AI 类菜单项）
- `⌘⇧F` = 唤起文件操作面板（仅显示 file accepts 菜单项）

**实现方案**：
- 每个 Query Hotkey 配置：`{ hotkey, position, query_filter, width }`
- 热键 handler 中根据配置过滤可见菜单项 + 定位面板
- 代码量：~100 行（多热键注册 + 菜单过滤 + 位置配置）

**参考**：Wox Query Hotkey（per-hotkey 配置 position/query/toolbar/width）

---

### 17. Snippet 自动展开

**作用**：全局键盘监听，打字时自动匹配关键词展开。

**典型场景**：
- 输入 `;date` → 自动替换为 `2026-07-15`
- 输入 `;email` → 自动替换为 `user@example.com`
- 输入 `;sig` → 自动展开为完整签名

**实现方案**：
- 全局键盘事件监听（macOS: `CGEventTap`，需 Accessibility 权限）
- 维护关键词→展开文本映射（DB 存储）
- 检测到关键词结束时触发删除+替换
- 代码量：~200 行（键盘监听 + 关键词匹配 + 替换）
- 难点：性能（高频键盘事件）、与输入法兼容、权限

**参考**：Raycast Snippets（关键词展开 + 动态变量）；Alfred Snippets（集合/富文本/占位符/安全字段）

---

### 18. 可视化 Workflow 编排

**作用**：拖拽式画布编辑器，连接节点编排自动化流程。

**典型场景**：
- 节点：选中文件 → OCR → LLM 翻译 → 写入剪贴板 → 粘贴
- 节点：定时 → 录音 → ASR → 润色 → 发送邮件

**实现方案**：
- 前端：React Flow（开源 DAG 画布库）
- 节点类型：输入（选中文本/文件/时间）、处理（ASR/OCR/LLM/Shell）、输出（粘贴/复制/通知/文件）
- 后端：DAG 执行引擎（拓扑排序 + 条件分支 + 变量传递）
- 代码量：~2000+ 行（画布编辑器 + 执行引擎 + 节点 SDK）
- 这是实现成本最高的功能，但差异化也最强

**参考**：Alfred Workflow 画布（拖拽/Prefabs/Automation Tasks/条件分支/变量系统/User Configuration）

---

## 附录：功能依赖关系

```
P0: Silent Query Hotkey ──┐
   Run And Paste ──────────┤
   模糊搜索优化 ───────────┤
                           ▼
P1: Deep Link ─────────────┤
   应用启动 ────────────────┤── 需要: 模糊搜索优化（P0）
   Quicklinks ─────────────┤
   计算器 ──────────────────┤
   单位换算 ────────────────┤
                           ▼
P2: 文件搜索 ──────────────┤── 需要: 应用启动（P1）的基础设施
   浏览器书签 ──────────────┤
   系统命令 ────────────────┤
   Shell 集成 ──────────────┤
   Action Panel ───────────┤── 需要: 模糊搜索优化（P0）
   AI Command Store ───────┤── 需要: 远程拉取基础设施
                           ▼
P3: Result Drag ───────────┤
   Query Hotkey 多实例 ────┤── 需要: Silent Query Hotkey（P0）
   Snippet 自动展开 ───────┤── 独立（需全局键盘监听）
   Workflow 编排 ──────────┘── 独立（最复杂）
```
