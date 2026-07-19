# Finder 文件 → Actionbar → Agent → PPT 制作桥接设计

> **状态**：已实现 ✅（2026-07-19，commit `1e688edc`）
> **日期**：2026-07-19
> **scope**：在 Finder 选中文件/文件夹 → 全局热键弹 actionbar → 点「Agent → 制作 PPT」→ 用户口述需求 → Pi（或 Claude Code）在 Terminal 中读文件 + 选 PPT skill + 生成 PPT + 打印产物路径
> **前置文档**：
> - [`2026-07-12-action-bar-file-agent-design.md`](./2026-07-12-action-bar-file-agent-design.md)（Finder 选中 → agent 桥接，本设计完全复用其链路）
> - [`2026-07-13-action-bar-agent-voice-design.md`](./2026-07-13-action-bar-agent-voice-design.md)（agent × 语音联动，含 `{{task}}` 触发音录）

---

## 1. 背景与动机

### 1.1 需求

用户在 Finder 选中文件或文件夹后，通过全局热键弹出 action bar，召唤外部 agent（Pi / Claude Code）阅读文件并制作 PPT。整个过程要求：

- **开箱即用**：用户装好 octopus 即有一个「制作 PPT」菜单项，不需要自己配 prompt
- **风格可选**：用户能用口述指定风格（瑞士风 / 可编辑 / 暗色 / 瑞士风等），agent 据此选 skill
- **路径可寻**：agent 完成后明确告诉用户产物在哪里

### 1.2 现状

octopus 已经把「Finder 选中文件 → actionbar → 让 agent 处理」链路 100% 建好（见前置文档）：

- `action_bar_items` 表支持 `action_type=agent`，绑定 `agent` 字段（adapter key）
- `agent_adapter.rs` 内置 `claude`（`claude --add-dir {cwd} {prompt}`）+ `pi`（`pi {files_at} {prompt}`）
- Finder 选中捕获 → Files context → 按 `accepts=file` 过滤菜单
- prompt 模板双花括号占位符：`{{files}}`（POSIX 路径列表）+ `{{task}}`（语音转写文本）
- `{{task}}` 自动触发语音录音（v27 迁移）
- `terminal_launcher.rs` 在 Terminal.app 新窗口拉起 agent

**结论**：本次任务不需要新增任何桥接代码。octopus 的角色是「**prompt 模板分发器 + skill 知识载体**」——把"如何让 agent 制作 PPT"这个知识固化成一个开箱即用的菜单项。

### 1.3 PPT Skill 调研

调研 `~/.tolaria/` 笔记，市面上主流 AI PPT skill 分 3 条技术路线：

| 路线 | 代表 skill | 输出 | 可编辑性 |
|---|---|---|---|
| **HTML 网页 PPT** | guizang-ppt-skill（22 版式锁定）、lewislulu/html-ppt-skill（36 主题） | 单文件 HTML | ✗ 需改代码 |
| **原生 PPTX** | ppt-master（python） | `.pptx` | ✅ PowerPoint 可改 |
| **Office DOM** | OfficeCLI（C# 单二进制） | `.pptx` + 渲染 | ✅ 可改 + 自愈 |

详见 [`.tolaria/工具-skills/ai-ppt-skill-全景-六款生成器一个编辑器.md`](../../../../../.tolaria/工具-skills/ai-ppt-skill-全景-六款生成器一个编辑器.md)。

### 1.4 octopus 定位

| octopus 是 | octopus 不是 |
|---|---|
| 桥接器：Finder 选中 → 菜单 → Terminal 拉起 agent | PPT 引擎 |
| Prompt 模板分发器（含 skill 候选清单） | Skill 安装器 |
| Seed 数据载体（外置 .md/.json 文件） | Skill 检测器 |
| 终端启动器 | 产物回收器（agent 在 Terminal 自己打印路径） |

---

## 2. 总体设计（Scope）

### 2.1 核心结论

octopus 是**桥接器**。本次的价值是：

1. 新增 1 个 PPT seed 菜单项（prompt 外置到 .md 文件）
2. **建立「seed 数据外部化」机制**——一举整合 PPT prompt + llm_provider + 润色 prompt 三类长文本 seed，从 db.sql 内联移到 `crates/infra/seeds/` 目录
3. **简化 init_schema**——删除 v17→v37 历史迁移分支（开发期唯一用户，DB 已 ≥v38，全是死代码）
4. **扩展 Quick Execute 支持 Files + agent**——让配了 `global_shortcut` 的 agent 菜单项能"快捷键 → 直接口述 → agent 执行"（详见 § 11）
5. 配套：prompts 复原按钮、用户文档、architecture.md 同步

### 2.2 改动清单

| # | 文件 | 性质 | 备注 | 状态 |
|---|---|---|---|---|
| 1 | `crates/infra/seeds/prompts/default-polish.md` | 新建 | 默认润色 prompt（从 db.sql 抽出） | ✅ |
| 2 | `crates/infra/seeds/prompts/advanced-polish.md` | 新建 | 进阶润色 prompt（从 db.sql 抽出） | ✅ |
| 3 | `crates/infra/seeds/llm_providers.json` | 新建 | 7 个 LLM provider 配置（从 db.sql 抽出） | ✅ |
| 4 | `crates/infra/seeds/agent_actions/make-ppt.prompt.md` | 新建 | PPT 制作 prompt 模板 | ✅ |
| 5 | `crates/infra/src/db.rs` | 修改 | `init_schema` 简化（删历史迁移）+ `load_external_seeds` 加载函数 | ✅ |
| 6 | `crates/infra/src/db.sql` | 修改 | 删除 3 类内联 seed + 保留 schema 和其他短 seed | ✅ |
| 7 | `crates/infra/Cargo.toml` | 修改 | `package.include` 加 `seeds/` 目录（release 打包） | ✅ |
| 8 | `crates/desktop/src/action_bar_commands.rs` | 修改 | 新增 `restore_prompt_from_seed(prompt_id)` Tauri 命令 | ✅ |
| 9 | `crates/desktop/frontend/src/pages/Settings/PromptsPanel.tsx` | 修改 | system prompt 支持编辑 + 「复原默认」按钮 | ✅ |
| 10 | `crates/desktop/src/action_hotkey.rs` | 修改 | `quick_execute` 扩展支持 Files + agent + {{task}} 触发音录（详见 § 11） | ✅ |
| 11 | i18n key（zh-CN.yaml / en.yaml） | 修改 | 「复原默认」+ 相关文案中英 | ✅ |
| 12 | `docs/features/make-ppt.md` | 新建 | 用户向文档 | ✅ |
| 13 | `docs/architecture.md` | 修改 | 同步外置 seed + Agent 菜单 + PPT 桥接 + quick_execute 扩展 | ✅ |
| 14 | `crates/desktop/frontend/vite.config.ts` | 修改（实施期发现） | vite 7→8 breaking：`clearScreen` 从 `server` 子字段迁到顶层；`defineConfig` 从 `vite` 导入（非 `vitest/config`）修复 tsc 编译错误 | ✅ |
| 15 | `crates/infra/src/seeds.rs` | 新建（实施期细化） | 独立模块承载所有 loader 函数 + 测试（spec § 4.4 原描述为 db.rs 内部函数，实施时拆为独立模块更聚焦） | ✅ |

### 2.3 「Agent」主菜单结构

```
Agent（主菜单，accepts=file，icon: bot）
└── 制作 PPT（子菜单，action_type=agent，agent=pi，accepts=file，prompt=make-ppt.prompt.md）
    └── 后续可继续加：「整理这个文件夹」「总结文档」等
```

- 「Agent」主菜单 `accepts=file`（仅 Finder 文件场景可见，不污染文本场景）
- 「制作 PPT」同样 `accepts=file`，默认绑定 agent=`pi`
- 主菜单用 `INSERT OR IGNORE` + title 去重，子菜单 parent_id 由 Rust 端查出来再插

### 2.4 明确不做（YAGNI）

- ❌ octopus 不内置 PPT 引擎、不调 OfficeCLI、不实现 HTML PPT 渲染
- ❌ 不扩展 `Files` context 做目录递归（让 agent 自己 walk）
- ❌ 不在 octopus 内做 skill 安装检测（让 agent 自己探 PATH / `~/.pi/agent/skills/`）
- ❌ 不做产物路径回收（agent 在 Terminal 自己打印绝对路径）
- ❌ 不重试 agent 执行（一次失败就失败，用户在 Terminal 看）
- ❌ 不约束 agent 输出路径（prompt 建议 cwd，不强制）
- ❌ 不引入新前端窗口/组件（除 PromptsPanel 加一个按钮）

---

## 3. PPT Prompt 模板设计

整个方案的核心交付物。agent 拿到的就是这段文本。

### 3.1 结构

prompt 内部分 4 段：

```
[1] 任务说明         ← 你要做什么
[2] skill 候选清单   ← 内联的 PPT skill 列表 + 选择规则
[3] 文件信息         ← {{files}} 注入点（octopus 端替换）
[4] 用户指令         ← {{task}} 注入点（octopus 端替换）
```

### 3.2 Skill 候选清单（4 条主推路线）

不写全部 7 个调研结果，避免 agent 决策疲劳：

| 路线 | skill | 安装命令 | 关键词触发 | 输出 |
|---|---|---|---|---|
| **HTML PPT（瑞士风/版式锁定）** | guizang-ppt-skill | `npx skills add https://github.com/op7418/guizang-ppt-skill --skill guizang-ppt-skill` | 默认 / "专业" "汇报" "正式" | 单文件 HTML |
| **HTML PPT（多主题）** | lewislulu/html-ppt-skill | `npx skills add https://github.com/lewislulu/html-ppt-skill` | "彩色" "霓虹" "科技" "dark" "主题" | 单文件 HTML |
| **原生可编辑 PPTX** | ppt-master（python） | `git clone https://github.com/hugohe3/ppt-master.git && pip install -r requirements.txt` | "可编辑" "PowerPoint" "pptx" "改字" | .pptx |
| **Office DOM（高保真）** | OfficeCLI | `curl -fsSL https://raw.githubusercontent.com/iOfficeAI/OfficeCLI/main/install.sh \| bash` | "office" "dom" "结构化" "高保真" | .pptx + 渲染自愈 |

### 3.3 决策规则（写进 prompt）

```
若用户提到「可编辑 / pptx / 改字 / PowerPoint 共享」→ ppt-master 或 OfficeCLI
若用户提到具体风格（瑞士风/暗色/霓虹/科技感）→ 对应 HTML PPT skill
若用户没说偏好 → 默认 guizang-ppt-skill（版式锁定、质量下限高）
```

### 3.4 未装 skill 的降级策略（写进 prompt）

```
若推荐 skill 都未安装：
  方案 A（首选）：告诉用户需要装哪个 + 给出完整安装命令（npx skills add ... 或 git clone ...）
                 用户装完后让他重新跑这个任务
  方案 B（fallback）：直接用 HTML 手写一份单文件 PPT
                     - 16:9 固定宽高比
                     - 含封面 / 目录 / 章节 / 正文 / 结尾页
                     - 内联 CSS，零依赖，浏览器打开即放映

不要尝试联网搜索其他 PPT skill——只用本 prompt 列出的 4 个。
```

### 3.5 输入约束（写进 prompt）

- 选中文件夹时：**自行 `ls -R` 或 `walk` 递归**，跳过 `.git` / `node_modules` / 二进制文件（图片/视频/可执行文件）
- 选中多个文件时：阅读每个文件后**统一规划 PPT 结构**，而不是每个文件一页
- 仅音频/视频文件时：先 ASR 转写，再用文本生成 PPT

### 3.6 产物路径披露（强制，写进 prompt）

```
# 完成后的强制披露（不可省略）

PPT 生成完成后，你必须在 Terminal 输出的最后一段明确告知用户：

✅ ============================================
✅ PPT 已生成：/Users/xxx/your-path/your-deck.html
✅ 打开方式：在 Finder 中按 Cmd+Shift+G 粘贴路径，或直接 Cmd+点击上方路径
✅ ============================================

要求：
- 路径必须是绝对路径（不要相对路径）
- 优先把产物放在用户当前工作目录下（即第一个选中文件的父目录）
- 文件名要有意义：YYYY-MM-DD-<主题简述>.<扩展名>
- 若有多份产物（HTML + PDF + PPTX），全部列出
- 若中途失败，必须明确说「未生成产物」，不要让用户误以为成功
```

### 3.7 Prompt 不变量

- 只用 `{{task}}` 和 `{{files}}` 两个占位符（octopus 端 `render_agent_prompt` 只替换这两个）
- prompt 内容不依赖任何 octopus 内部状态——纯文本，agent 拿到就能用
- prompt 的安装命令必须是**确定性**的（具体 URL），不能让 agent "去 github 搜"
- prompt 对 agent 中立（pi / claude 都能读），不写 agent 专属语法

---

## 4. 外置 Seed 加载机制

### 4.1 目录结构

```
crates/infra/seeds/
├── prompts/
│   ├── default-polish.md      ← 默认润色 prompt
│   └── advanced-polish.md     ← 进阶润色 prompt
├── llm_providers.json         ← LLM provider 配置（数组）
└── agent_actions/
    └── make-ppt.prompt.md     ← PPT 制作 prompt 模板
```

### 4.2 加载时机（运行期）

**仅 schema 升级时执行一次**，符合 octopus 现有 `PRAGMA user_version` 迁移语义：

| 库状态 | 行为 |
|---|---|
| 全新库（v<17） | 跑 INIT_SQL 建表 → 跑外置 seed → 设 v=39 |
| v<39 旧库 | 跑外置 seed → 设 v=39 |
| v≥39 库 | 直接 return，**完全不读文件**（性能最优） |

### 4.3 init_schema 简化方案

```
当前结构：
  v≥38 return
  v≥17 跑 v32→v38 一大串迁移（200+ 行死代码，开发期唯一用户）
  v<17 全新库：INIT_SQL + fill_manifests

改后结构：
  v≥39 return
  v<17 全新库：INIT_SQL + load_external_seeds + fill_manifests → 设 v=39
  v<39 旧库：load_external_seeds → 设 v=39
```

**删除范围**：v17→v37 的所有历史迁移分支（v32 trigger_keyword/auto_paste、v33-v34 app_index、v35 search_frequency、v36 launcher_index、v37 models 语义重构）。vault 表已在 db.sql 的 `CREATE TABLE IF NOT EXISTS` 里覆盖，对新库是 no-op。

### 4.4 加载函数签名

```rust
// crates/infra/src/db.rs

/// 加载外置 seed 文件，插入 DB。schema 升级时调用一次。
/// 失败时 log::error 并跳过该项，绝不阻塞 schema 升级。
fn load_external_seeds(conn: &Connection) -> Result<()> {
    let seeds_dir = seeds_dir();  // CARGO_MANIFEST_DIR/seeds（dev）或 exe 同级/seeds（release）

    // 1. prompts/ 下所有 .md
    load_prompt_seed(conn, &seeds_dir.join("prompts/default-polish.md"), 1, "默认润色", "voice_text_polish", "默认润色（系统内置）")?;
    load_prompt_seed(conn, &seeds_dir.join("prompts/advanced-polish.md"), 2, "进阶润色（断续纠正）", "voice_text_polish", "进阶版：针对断续纠正、重复修正、同音漂移场景强化的润色 prompt（系统内置）")?;

    // 2. llm_providers.json
    load_llm_providers_seed(conn, &seeds_dir.join("llm_providers.json"))?;

    // 3. Agent 主菜单 + PPT 子菜单
    load_agent_make_ppt_seed(conn, &seeds_dir.join("agent_actions/make-ppt.prompt.md"))?;

    Ok(())
}

/// 暴露给 desktop crate 的复原按钮用
pub fn seed_prompt_path(name: &str) -> Option<std::path::PathBuf> {
    let seeds_dir = seeds_dir();
    let path = seeds_dir.join("prompts").join(format!("{}.md", name));
    if path.exists() { Some(path) } else { None }
}

fn seeds_dir() -> std::path::PathBuf {
    // dev: CARGO_MANIFEST_DIR/seeds
    // release: 通过 Cargo.toml package.include 打包到 binary 同级
    // ...
}
```

### 4.5 失败处理

| 场景 | 处理 |
|---|---|
| seeds 目录整体缺失 | `log::error!` + 跳过所有外置 seed（schema 仍升级） |
| 单个文件缺失 | `log::error!` + 跳过该项（其他项继续） |
| JSON 解析失败 | `log::error!` + 跳过 llm_providers |
| SQL 插入失败（约束冲突） | `log::warn!` + 继续（INSERT OR IGNORE 本就容忍） |

**关键不变量**：seed 失败**永远不阻塞 schema 升级**——schema 升级必须成功，否则用户 DB 锁死。

### 4.6 Prompts Seed 的更新冲突

`prompts` 表用户可能在 settings 编辑过。策略：

- `INSERT OR IGNORE`：id 已存在则跳过，**用户编辑不被覆盖**（正确）
- **新增「复原默认」按钮**：用户主动想复原时，点击 → `restore_prompt_from_seed(prompt_id)` 命令读 seeds/ 文件覆盖 textarea → 用户保存
- 用户没主动点复原，作者更新了 seed 文件，老用户收不到（这是正确的，尊重用户修改）

### 4.7 llm_providers.json Schema

```json
[
  {
    "config_key": "deepseek",
    "config_value": {
      "base_url": "https://api.deepseek.com/",
      "models": ["deepseek-chat", "deepseek-reasoner", "deepseek-v4", "deepseek-v4-flash"]
    },
    "description": "DeepSeek API",
    "category": "llm_provider"
  },
  // ... 其他 6 个
]
```

`config_value` 在 SQL 插入时 `serde_json::to_string` 序列化为 TEXT 存到 app_config.config_value 列。

---

## 5. 数据流与端到端流程

### 5.1 用户视角完整流程

```
1. 用户在 Finder 选中文件 / 文件夹
2. 按全局热键（默认 Cmd+Shift+Space）→ action bar 浮窗弹出
3. 浮窗顶部显示「N 个文件选中」badge
4. 浮窗菜单显示「Agent → 制作 PPT」（accepts=file 过滤后唯一可见的菜单组）
5. 用户点「制作 PPT」→ 因 prompt 含 {{task}}，自动启动录音
6. 用户口述：「做个瑞士风的，给老板看的」→ ASR 转写
7. 录音停止 → ASR 文本注入 {{task}}
8. octopus 在 Terminal.app 新窗口拉起：
     pi @<file1> @<file2> ... '<完整 prompt>'
9. Pi 启动，读 prompt → 根据口述选 skill → 装或调用 → 生成 PPT
10. Pi 在 Terminal 末尾打印产物绝对路径
11. 用户 Cmd+点击路径打开 PPT
```

### 5.2 octopus 端零改动验证（每步已实现）

| 步骤 | 已有实现位置 |
|---|---|
| Finder 选中捕获 | `finder_selection.rs`（AppleScript） |
| Files context 组装 | `action_bar_commands.rs::ActionBarContext::for_files` |
| accepts 过滤 | 前端 ActionBar 浮窗 `accepts === 'file' \|\| 'any'` |
| 含 `{{task}}` 触发音录 | `trigger_agent_voice`（v27 迁移） |
| ASR 文本注入 prompt | `render_agent_prompt`（action_bar_commands.rs:1169） |
| 命令模板渲染 | `agent_adapter::render_command`（行 81） |
| Terminal.app 拉起 | `terminal_launcher::TerminalAppLauncher` |

### 5.3 PPT 菜单项 DB 记录

「Agent」主菜单和「制作 PPT」子菜单**都用 title 去重、不固定 id**（对齐「问豆包」seed 模式，避免未来 schema id 冲突）。现有固定 id 用到 1-10（AI=1, 翻译=2, 搜索=3, 网页=4, 润色=5, 摘要=6, 解释=7, Google=8, 百度=9, Bing=10），新 seed 不进固定 id 段。

```sql
-- 主菜单：Agent（顶级菜单，title 去重，sort_order=5 排在「网页」之后）
INSERT INTO action_bar_items (parent_id, title, icon, action_type, action_data, sort_order, is_system, accepts)
SELECT NULL, 'Agent', 'bot', 'submenu', '', 5, 1, 'file'
WHERE NOT EXISTS (SELECT 1 FROM action_bar_items WHERE title='Agent' AND parent_id IS NULL);

-- 子菜单：制作 PPT
-- action_data 由 Rust 端从 make-ppt.prompt.md 读出后通过参数化 SQL 填入
-- parent_id 子查询定位 Agent 主菜单（不复用固定 id）
INSERT INTO action_bar_items (parent_id, title, icon, action_type, action_data, agent, accepts, sort_order, is_system)
SELECT
  (SELECT id FROM action_bar_items WHERE title='Agent' AND parent_id IS NULL),
  '制作 PPT',
  'presentation',
  'agent',
  :prompt_from_file,  -- Rust 端从 make-ppt.prompt.md 读出（rusqlite params! 绑定）
  'pi',
  'file',
  0,
  1
WHERE NOT EXISTS (
  SELECT 1 FROM action_bar_items
  WHERE title='制作 PPT'
    AND parent_id = (SELECT id FROM action_bar_items WHERE title='Agent' AND parent_id IS NULL)
);
```

> 实际 Rust 实现（`load_agent_make_ppt_seed`）：先把 prompt 文件内容读出，用 `conn.execute(sql, params![prompt_content])` 绑定参数（不走字符串拼接，避免 prompt 含单引号致 SQL 注入）。
>
> 子菜单 icon `presentation` 是 lucide-react 图标名（前端图标组件按名查表）。

### 5.4 Prompts 复原按钮流程

```
用户点「复原默认」
  ↓
前端 invoke("restore_prompt_from_seed", { promptId: 1 })
  ↓
后端调 infra::db::seed_prompt_path("default-polish")
  → 读 crates/infra/seeds/prompts/default-polish.md
  → 返回内容字符串
  ↓
前端把返回内容覆盖到 textarea（不自动保存，用户继续编辑或手动保存）
```

---

## 6. 错误处理矩阵

| 场景 | 用户视角 | octopus 端处理 |
|---|---|---|
| Pi/Claude 都未装 | 点「制作 PPT」→ toast「Pi 未安装（未在 PATH 找到 `pi`）」 | 已有逻辑（`agent_adapter::is_available`） |
| 选中的文件夹为空 | Terminal 跑起来但 agent 报告"无文件可读" | prompt 已指示 agent 检测 + 报告（octopus 不预检） |
| prompt 文件加载失败 | DB 中 PPT 子菜单 `action_data` 为空字符串 | log error，子菜单依然创建（用户可在 settings 手动编辑） |
| seeds 目录整体缺失 | 全新库无 prompts/llm_providers/PPT 菜单 | log::error 但不阻塞 schema 升级；用户仍能用其他功能 |
| agent 装了但没装 PPT skill | Terminal 中 agent 输出："需要安装 guizang-ppt-skill，运行 npx skills add ..." 或自己 fallback 写 HTML | prompt 已明确指示降级策略（§ 3.4） |
| 用户口述空音频 | "识别结果为空，无法重试" toast | 已有逻辑（trigger_agent_voice → ASR 空 → task failed） |
| Terminal.app 启动失败 | toast「终端启动失败：{error}」 | 已有逻辑（terminal_launcher::spawn Err 臂） |
| agent 干完活但没打印路径 | 用户在 Terminal 翻历史找路径 | octopus 不负责；prompt 已强制要求（§ 3.6） |
| agent 中途崩溃 | Terminal 看到 stack trace，无产物路径 | 用户在 Terminal 自己判断失败 |

### 关键不变量

1. seed 加载永不阻塞 schema 升级
2. DB 中永远有 PPT 菜单项记录（即使 prompt 内容空）
3. octopus 永不直接读用户选中的文件内容（agent 自己读，octopus 只传路径）
4. prompt 文件永不泄露到 octopus 仓库外（在 `crates/infra/seeds/` 跟代码版本控制）

---

## 7. 测试

### 7.1 单元测试矩阵

下表对照实施后实际落地的测试名。设计阶段的 12 项矩阵在实施时按 TDD 细化拆分（一项设计测试可能对应多个实际测试），表末附最终通过统计。

| # | 设计意图 | 实际测试名 | 位置 | 状态 |
|---|---|---|---|---|
| 1 | v38→v39 升级 | `migration_v38_to_v39_loads_external_seeds_and_preserves_user_edits` | `db.rs` | ✅ |
| 2 | 全新库 → v39 | `init_schema_fresh_db_builds_v39` | `db.rs` | ✅ |
| 3 | 已 v39 → no-op | `init_schema_already_v39_is_noop` | `db.rs` | ✅ |
| 4 | seed 缺失不阻塞 | `load_external_seeds_never_propagates_errors` + `load_prompt_seeds_missing_file_returns_err` | `seeds.rs` | ✅ |
| 5 | 幂等 | `load_prompt_seeds_is_idempotent_via_insert_or_ignore` + `load_llm_providers_seed_skips_existing_keys` + `load_agent_action_seeds_is_idempotent` | `seeds.rs` | ✅ |
| 6 | seed 路径函数 | `seed_prompt_path_returns_some_for_known_name` + `seed_prompt_path_returns_none_for_unknown_name` | `seeds.rs` | ✅ |
| 7 | PPT prompt 占位符 | `make_ppt_prompt_contains_required_placeholders` | `seeds.rs` | ✅ |
| 8 | 前端 accepts 过滤 | （未单测，project convention——前端逻辑靠 E2E） | — | ⏭ |
| 9 | File + agent + {{task}} → voice | `decide_files_action_agent_with_task_triggers_voice` | `action_hotkey.rs` | ✅ |
| 10 | File + agent 无 {{task}} → direct | `decide_files_action_agent_without_task_executes_directly` + `decide_files_action_script_type_executes_directly` + `decide_files_action_url_type_executes_directly` | `action_hotkey.rs` | ✅ |
| 11 | hide_action_bar=false 路径 | （纯函数 `decide_files_action` 覆盖；Tauri/Coordinator 耦合部分不单测，project convention） | — | ⏭ |
| 12 | system prompt 可编辑 | `update_prompt_at_allows_system_prompt` | `db.rs` | ✅ |

**实际通过统计**：`cargo test -p octopus-infra` 143 PASS / 0 FAIL；`cargo test -p octopus-desktop --lib` 375 PASS / 0 FAIL；前端 vitest 304 PASS / 0 FAIL。

**测试隔离**（参考 `db.rs:33` 既有约定）：测试时 seeds 文件路径必须能用 `CARGO_MANIFEST_DIR` 找到，避免 `cargo test` 时找不到文件。`seeds.rs` 的 `load_tests` mod 用 `SEEDS_FILE_MUTEX` 序列化 seed 文件读写测试，防并行 runner 竞态。

### 7.2 手工 E2E 验收清单

```bash
# 1. 干净环境验证
rm ~/.octopus/octopus.db
cargo run --release -p octopus-desktop --features embedded
# 验证：DB 重建 + Agent 主菜单存在 + PPT 子菜单存在 + prompts/llm_providers 已 seed

# 2. 老 DB 升级验证（v38 → v39）
# 用现有 ~/.octopus/octopus.db
cargo run --release -p octopus-desktop --features embedded
# 验证：升级日志显示 v39、原有数据未丢、新增 Agent 菜单

# 3. 端到端 PPT 流程
# Finder 选中一个含 3 个 markdown 的文件夹
# 全局热键 → Agent → 制作 PPT
# 口述「做个瑞士风」→ 录音停 → Pi 在 Terminal 启动
# 验证：Pi 收到的 prompt 包含口述内容 + 文件路径
# 验证：Pi 完成后打印绝对路径

# 4. prompt 复原按钮
# 设置 → 提示配方 → 编辑默认润色 → 改几个字 → 保存
# 点「复原默认」→ 验证 textarea 恢复到 seed 文件内容

# 5. seed 文件缺失降级
mv crates/infra/seeds /tmp/seeds-backup
rm ~/.octopus/octopus.db && cargo run ...
# 验证：schema 升级成功（不阻塞）、log 有 error、Agent 菜单 action_data 为空
mv /tmp/seeds-backup crates/infra/seeds
```

---

## 8. 文档交付

| 文件 | 内容 |
|---|---|
| `docs/features/make-ppt.md`（新建） | 用户向：(1) 怎么用（Finder 选中 → 热键 → Agent → 制作 PPT → 口述 → 等 agent 完成）(2) 推荐哪些 PPT skill、怎么装（4 个 skill 的安装命令 + 何时选哪个）(3) 产物在哪里找（Terminal 末尾的 ✅ 路径）(4) 改 prompt 怎么改（设置 → 命令面板 → Agent → 制作 PPT → 编辑） |
| `docs/architecture.md`（修改） | 在「AI 命令面板」章节追加：① 外置 seed 机制（`crates/infra/seeds/` 目录 + init_schema 加载策略 + 失败降级）② Agent 主菜单（`accepts=file`，承载 agent 类型子菜单）③ PPT 桥接（不改 agent 适配器层，纯 prompt 模板 + skill 候选清单）④ prompts 复原按钮（settings panel）⑤ init_schema 简化（v39） |

---

## 9. 实施顺序（写 plan 时参考）

1. **infra 外置 seed 加载机制**——基础设施先行，可独立测试
2. **简化 init_schema**——删历史迁移分支，加 v39 分支
3. **db.sql 清理**——删 3 类内联 seed + 加 Agent 主菜单 schema
4. **seeds/ 目录与文件**——prompts/llm_providers/PPT prompt 全写好
5. **prompts 复原命令 + 前端按钮**——独立小特性
6. **docs/features/make-ppt.md**——文档先行，验证 prompt 时能参照
7. **architecture.md 同步**

---

## 11. Quick Execute 扩展（agent × Files × 语音）

### 11.1 背景

现有 `quick_execute`（`action_hotkey.rs:88`）只支持 `Selection::Text`，遇 File/Folder 直接 `return`（注释明确说"菜单项热键语义是对文本执行动作"）。但 agent 类型菜单项（accepts=file）需要：**Finder 选中文件 → 全局热键 → 直接口述需求 → agent 执行**——跳过 ActionBar 浮窗。

### 11.2 改动概览

只改 `action_hotkey.rs::quick_execute`，无新增命令、无前端改动。

```
旧链路：
  热键 → detect → 仅 Text 通过 → execute_action_bar_inner（直接执行，agent 类型走 Terminal 拉起，无 task）

新链路（File/Folder 分支）：
  热键 → detect
    ├─ Text  →（保持现状）execute_action_bar_inner
    ├─ File/Folder → 写 PENDING_CONTEXT (kind=Files)
    │                → 查 item: 若 action_type=agent 且 prompt 含 {{task}}
    │                  → 调 trigger_agent_voice（复用现有命令，触发音录）
    │                → 否则（非 agent 或无 {{task}}）
    │                  → execute_action_bar_inner（直接执行）
    └─ None → 静默失败（保持现状）
```

### 11.3 详细分支逻辑

```rust
fn quick_execute(item_id: i64, app: &AppHandle) {
    let saved_baseline = save_change_count_baseline();
    let selection = detect_selection(app);
    restore_change_count_baseline(saved_baseline);

    match selection {
        Selection::Text { text, .. } => {
            // 现有逻辑保持不变（gather_context → set_pending_context → execute）
            // ...
        }
        Selection::File { files, .. } | Selection::Folder { folders: files, .. } => {
            // ── 写 PENDING_CONTEXT (kind=Files) ──
            // trigger_agent_voice 内部从 PENDING_CONTEXT 读 files，所以必须先写。
            let ctx = ActionBarContext::for_files(files.clone());
            set_pending_context(ctx);

            // ── 查 item 决定路径 ──
            let item = match octopus_infra::db::load_action_bar_item(item_id) {
                Ok(Some(it)) => it,
                _ => { log::warn!("..."); return; }
            };

            if item.action_type == "agent" && item.action_data.contains("{{task}}") {
                // 含 {{task}} → 走 trigger_agent_voice 路径（触发音录）
                // 复用现有 Tauri 命令逻辑——但 trigger_agent_voice 是 #[tauri::command]，
                // 不能直接调；提取核心逻辑为 pub fn 或直接重新实现（几行代码）
                let coordinator = app.state::<crate::coordinator::Coordinator>();
                trigger_agent_voice_core(item_id, app, coordinator.inner());
            } else {
                // 非 agent 或无 {{task}} → 直接执行（url/script/copy_path/agent-无task）
                // 复用 Text 分支的执行逻辑
                // 注意：agent 无 {{task}} 时 prompt 仍可能含 {{files}}，需渲染
                execute_action_bar_inner_via_runtime(item_id, "".into(), app);
            }
        }
        Selection::None => {
            log::info!("[action-hotkey] 无选中，跳过 item_id={}", item_id);
            return;
        }
    }
}
```

### 11.4 trigger_agent_voice_core 提取

现有 `trigger_agent_voice` Tauri 命令（`action_bar_commands.rs:1796`）做的事：

1. 查 item
2. 从 PENDING_CONTEXT 读 files
3. derive_cwd + 组 context JSON
4. 创建 agent_task（DB）
5. hide_action_bar_window + finalize_action_bar
6. `coordinator.start_agent_recording(task_id)`

`quick_execute` 路径下不需要第 5 步（ActionBar 没显示）；其他步骤都需要。**提取为内部纯函数**：

```rust
// crates/desktop/src/action_bar_commands.rs

/// trigger_agent_voice 的核心逻辑——Tauri 命令和 quick_execute 共用。
/// `hide_action_bar: bool` 控制是否走 hide 浮窗（quick_execute 路径 ActionBar 没显示，传 false）。
pub fn trigger_agent_voice_core(
    item: &ActionBarItem,
    app: &AppHandle,
    coordinator: &crate::coordinator::Coordinator,
    hide_action_bar: bool,
) -> Result<(), String> {
    let files: Vec<String> = PENDING_CONTEXT.lock().unwrap()
        .as_ref().map(|c| c.files.clone()).unwrap_or_default();
    let cwd = derive_cwd(&files);
    let context = serde_json::json!({
        "kind": "files",
        "files": files,
        "cwd": cwd,
        "prompt_template": item.action_data,
    }).to_string();
    let task_id = uuid::Uuid::new_v4().to_string();
    octopus_infra::db::insert_agent_task(&task_id, &item.agent, &context)
        .map_err(|e| e.to_string())?;
    if hide_action_bar {
        hide_action_bar_window(app);
        finalize_action_bar(app);
    }
    coordinator.start_agent_recording(task_id);
    Ok(())
}

#[tauri::command]
pub async fn trigger_agent_voice(
    item_id: i64,
    app: AppHandle,
    coordinator: tauri::State<'_, crate::coordinator::Coordinator>,
) -> Result<(), String> {
    let item = octopus_infra::db::load_action_bar_item(item_id)
        .map_err(|e| e.to_string())?
        .ok_or("菜单项不存在")?;
    trigger_agent_voice_core(&item, &app, coordinator.inner(), true)
}
```

### 11.5 seed 不设默认 global_shortcut

按用户决策（spec § 2.5 / 选项 2），seed 创建 PPT 菜单项时 `global_shortcut=''`（默认值）。用户需要「快捷键直述」体验时，在设置里手动给 PPT 菜单项配 global_shortcut。

理由：
- 全局热键资源紧张，避免占用常见组合（Cmd+Shift+P 是 VS Code 命令面板等）
- 不与用户其他软件冲突
- 用户主动配置 = 用户理解了这是 Quick Execute 路径

文档（`docs/features/make-ppt.md`）需要明确告诉用户："如果想要『快捷键直接口述』体验，去设置 → 命令面板 → Agent → 制作 PPT → 全局快捷键 配一个组合键"。

### 11.6 关键不变量

1. `trigger_agent_voice_core` 是纯逻辑函数，无 Tauri State 依赖（参数传入 coordinator）
2. ActionBar 浮窗路径（`hide_action_bar=true`）与 quick_execute 路径（`hide_action_bar=false`）走同一份核心逻辑
3. quick_execute 的 File/Folder 分支不会污染 Text 分支（match 穷尽）
4. agent 类型但无 `{{task}}` → 直接执行（prompt 用 {{files}} 渲染后丢给 agent）

### 11.7 测试

新增单测：

| # | 测试 | 验证什么 |
|---|---|---|
| T-quick-1 | `quick_execute_file_selection_with_agent_task_triggers_voice` | mock detect 返回 File → 检查 coordinator.start_agent_recording 被调 |
| T-quick-2 | `quick_execute_file_selection_with_agent_no_task_executes_directly` | mock detect 返回 File + item 无 {{task}} → 走 execute_action_bar_inner |
| T-quick-3 | `trigger_agent_voice_core_with_hide_false_skips_hide` | hide_action_bar=false 时 hide_action_bar_window 不被调 |

---

## 13. 实施期修订（v40 / v41 补丁）

实施后用户实测发现两个 bug，对应 schema 升级 v39→v40→v41：

### 13.1 v40：`need_voice` 字段取代 `{{task}}` 字符串扫描（commit `9c5c67ae`）

**Bug**：原方案让前端 / `quick_execute` / `trigger_agent_voice` 都用 `action_data.includes("{{task}}")` 判定是否触发语音。脆弱——用户实测发现早期 Task 1 残留的 `action_data=''` PPT 菜单永远进不了语音路径。

**修订**：新增 `action_bar_items.need_voice INTEGER NOT NULL DEFAULT 0` 列（DB v40）。
- `ActionBarItem.need_voice: bool` 字段贯穿全链路
- 前端 `index.tsx`、`action_hotkey::decide_files_action`、`trigger_agent_voice` Tauri 命令**全部**从扫描字符串改为读字段
- 设置面板 ActionBarPanel 新增「语音输入」Toggle（仅 agent 类型显示）
- DB CRUD（`insert_action_bar_item` / `update_action_bar_item`）签名加 `need_voice: bool` 参数
- `extensions.rs` 调用方传 `need_voice=false`（script 类型不需要）
- seed 加载时强制 `need_voice=1`（PPT 菜单语义）

### 13.2 v40：`render_command` 检测目录（commit `9c5c67ae`）

**Bug**：用户实测 Pi 报 `EISDIR: illegal operation on a directory, read`——Pi 的 `@<path>` 语法只接受文件，传目录会崩。

**修订**：`agent_adapter::render_command` 渲染 `{files_at}` 时检测每个路径：
- `std::path::Path::new(f).is_dir()` = true → 不加 `@` 前缀（传裸路径）
- 否则 → 加 `@` 前缀（正常 Pi 文件引用）
- prompt 文本（`make-ppt.prompt.md`）补充提示：「不要用 `@<dir>`，先 `ls` 展开为文件列表」
- `{files}` 占位符不受影响（本来就只传路径列表，文件/目录都安全）

### 13.3 v41：PPT 子菜单去重 + seed 再自愈（commit `1e688edc`）

**Bug**：用户 DB 残留 2 条「制作 PPT」子菜单——早期 `INSERT OR IGNORE`（表无 UNIQUE 约束）曾留下多条。

**修订**：
- `load_agent_action_seeds` 增加 dedup 步骤：`DELETE WHERE id NOT IN (SELECT MIN(id) ...)` 保留最早一条
- seed 加载 5 步流程：插 Agent 主菜单 → 查 Agent id → **去重 PPT 子菜单** → 插 PPT 子菜单（WHERE NOT EXISTS）→ UPDATE 自愈（空 action_data 补 prompt + need_voice 强制 1）
- v40→v41 迁移：再跑一次 `load_external_seeds`，让已 v40 的用户 DB 也吃到去重修复

---

## 12. 未来扩展（YAGNI）

- ❌ **octopus 内置 PPT 引擎**：偏离桥接器定位
- ❌ **目录递归**：让 agent 自己 walk 即可
- ❌ **Skill 安装检测**：让 agent 自己探 PATH
- ❌ **产物路径回收 / macOS 通知**：Terminal 打印路径已够用；未来若加，让 prompt 指示 agent 自己 osascript 发系统通知（零代码改动）
- ❌ **多 PPT 风格分支菜单**：一个 prompt + agent 决策已够；用户需要可自建多个菜单项
- ❌ **约束 agent 输出路径**：prompt 建议已够
- 💡 **后续可加 Agent 主菜单其他子菜单项**：「整理这个文件夹」「总结文档」「翻译全部」——同样的 agent 桥接机制，纯新增 prompt 文件

---

## 14. v43 修订：两阶段大纲工作流（PPT 大纲 + PPT 制作）

### 14.1 设计动机

用户实测反馈：直接通过「PPT 制作」一步生成 PPT，质量不稳定——即使 pi 使用了 guizang-ppt-skill，也只能说比无 skill 好一点点。根本原因：

- guizang SKILL.md 内部虽然有「大纲协助（叙事弧）」章节，但**大纲是隐式状态**——agent 内部脑补完就直接进 Step 2 拷模板、Step 3 填内容
- **用户拿不到中间产物**，无法增删页、改要点、补关键数据、调层级——没有 human review 卡点，质量完全靠大模型一次性理解

### 14.2 两阶段拆分

把原「制作 PPT」拆成**两个对偶菜单**：

```
[选中文件/目录] → PPT 大纲 → outline.md（agent 停手）
                              ↓ 用户用编辑器增删页 / 改要点
                              ↓ 要继续调整？直接在 Terminal 里给 agent 打字
[选中 .md]      → PPT 制作 → 最终 PPT（跳过 guizang Step 1，按大纲渲染）
```

| 菜单 | sort_order | icon | 行为 |
|---|---|---|---|
| **PPT 大纲**（v43 新增） | -1 | `file-text` | 读文件 → 生成 Markdown 中间产物 → **停手** |
| **PPT 制作**（v43 改名，原「制作 PPT」） | 0 | `presentation` | 输入 .md 跳过 Step 1；输入源文件正常走 Step 1 |

### 14.3 Markdown 大纲 Schema（极简版）

```markdown
---
title: <主题>
audience: <受众，如「产品经理」「技术评审」>
duration_min: 30        # 驱动页数预算：15min≈10页 / 30min≈20页 / 45min≈25-30页
style: A                # A=电子杂志风 / B=瑞士国际主义风
---

## P01 · Hook · <页标题>
- 一句话要点
- 一句话要点

## P02 · Context · <页标题>
- ...

（按叙事弧 Hook → Context → Core → Shift → Takeaway 组织）
```

**设计取舍**：经过与完整版 schema 对比，用户选择极简版——4 个 front matter 字段 + 每页 H2 + 自然语言 bullet。`layout` / `theme` / `image_slot` 等版式决策仍交给 guizang 自己脑补，避免用户编辑时学一堆字段。

### 14.4 细化机制：交给 Terminal，不在 octopus 侧做

**关键发现**：Pi / Claude 的现有模板（`pi {files_at} {prompt}` / `claude --add-dir {cwd} {prompt}`）都没带 `-p`/`--print`，所以 agent 跑完初始 prompt 后**停在交互式 TUI 等用户继续打字**。session 按 cwd 隐式持久化：
- Pi: `~/.pi/agent/sessions/<encoded-cwd>/*.jsonl`
- Claude: `~/.claude/projects/<encoded-cwd>/*.jsonl`
- 两边都支持 `--continue` / `--resume <id>` / `--session-id <id>`

**v43 的细化责任归属**：
- v1 生成后，agent 在 Terminal 里交互式 TUI **本就停着**
- 用户要细化：直接在 Terminal 打字「在 v1 基础上加入 XX」→ agent 改 v1 → 满意后切走
- octopus **不管第二轮**——无需记 session_id、无需双模式 prompt、无需版本号
- 关掉 Terminal 后又想改：用户自己 `cd <cwd> && pi --continue`（Pi/Claude 的原生能力）

**砍掉的过度设计**（plan v2 曾考虑）：
- ❌ 版本号 `-v1` / `-v2` / `-vN+1`
- ❌ 双模式 prompt（初次生成 / 细化迭代）
- ❌ "保留前一版"逻辑
- ❌ octopus 侧的 session_id 跟踪
- ❌ `{session_id}` / `{continue_flag}` 占位符

### 14.5 v43 改名迁移

`制作 PPT` → `PPT 制作`（与新增的 `PPT 大纲` 形成对偶）。**row id 保持不变**——不破坏用户在「设置」里绑定的全局快捷键。

实现：`load_agent_action_seeds` 步骤 3 加一句 `UPDATE ... SET title='PPT 制作' WHERE title='制作 PPT'`。

### 14.6 seeds.rs 重构

v43 抽出 `upsert_agent_submenu(conn, parent_id, title, icon, prompt_content, sort_order)` 共用函数，复用给 PPT 大纲 / PPT 制作两个子菜单的「去重 + INSERT + 自愈」3 步流程。原 v40 的硬编码 INSERT + UPDATE 拆解后更清晰。

### 14.7 make-ppt.prompt.md 的 .md 输入识别

在「文件读取约束」之前新增章节「**特殊输入：Markdown 大纲**」：

- 检测 `{{files}}` 是否含 `.md` 文件
- 是 → 跳过 guizang Step 1 的 7 个澄清问题，按每页 H2 直接渲染
- 否 → 正常走 Step 1（从需求开始问）

明确约束：「**不要二次总结、不要改变页数和顺序**——用户的编辑是故意的」。

### 14.8 影响面（5 个文件）

| 文件 | 改动 |
|---|---|
| `crates/infra/seeds/agent_actions/ppt-outline.prompt.md` | **新增**：大纲生成 prompt |
| `crates/infra/seeds/agent_actions/make-ppt.prompt.md` | 加「特殊输入：Markdown 大纲」章节 |
| `crates/infra/src/seeds.rs` | 改名迁移 + upsert_agent_submenu 抽取 + 新增 PPT 大纲注入 |
| `crates/infra/src/db.rs` | **schema v42→v43 bump**（触发 seed 重跑）+ 测试断言更新 + v42→v43 升级测试 |
| `docs/` | spec §14 / plan v43 / architecture |

**不动**：`render_agent_prompt` / `coordinator.rs` / `action_bar_commands.rs` / `agent_adapter.rs` / 前端代码 / db.sql（表结构不变）

### 14.9 Schema v42 → v43（纯触发 seed 重跑）

**问题**：v43 没有表结构变更，只改了 seed（改名 + 新增）。但 `init_schema` 的 `if v >= 42 { return Ok(()) }` 早返会让已 v42 的老 DB 永远不重新加载 seed——用户实测发现菜单不出现。

**修复**：bump `user_version` 42 → 43，纯粹为了触发 `load_external_seeds(conn)` 重跑：
- 已 v42 的用户重启 octopus 后自动升级到 v43
- seed 重跑完成「制作 PPT」→「PPT 制作」改名 + 新增「PPT 大纲」子菜单
- row id 不变（保快捷键）

`init_schema` 改动：
```rust
if v >= 43 {  // 原 v42
    return Ok(());
}
// ...
conn.execute("PRAGMA user_version = 43", [])?;  // 原 42（v17+ 分支）
// ...
conn.execute("PRAGMA user_version = 43", [])?;  // 原 42（v<17 全新库分支）
```

测试覆盖：`migration_v42_to_v43_renames_and_adds_ppt_outline` —— 模拟 v42 老库（手工倒回「制作 PPT」标题 + 删 PPT 大纲），跑 init_schema，验证 bump + 改名 + 新增 + row id 不变 4 个不变量。
