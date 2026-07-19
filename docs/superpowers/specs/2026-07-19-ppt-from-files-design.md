# Finder 文件 → Actionbar → Agent → PPT 制作桥接设计

> **状态**：设计中（待实现）
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

octopus 是**桥接器**——零业务代码改动。本次的价值是：

1. 新增 1 个 PPT seed 菜单项（prompt 外置到 .md 文件）
2. **建立「seed 数据外部化」机制**——一举整合 PPT prompt + llm_provider + 润色 prompt 三类长文本 seed，从 db.sql 内联移到 `crates/infra/seeds/` 目录
3. **简化 init_schema**——删除 v17→v37 历史迁移分支（开发期唯一用户，DB 已 ≥v38，全是死代码）
4. 配套：prompts 复原按钮、用户文档、architecture.md 同步

### 2.2 改动清单

| # | 文件 | 性质 | 备注 |
|---|---|---|---|
| 1 | `crates/infra/seeds/prompts/default-polish.md` | 新建 | 默认润色 prompt（从 db.sql 抽出） |
| 2 | `crates/infra/seeds/prompts/advanced-polish.md` | 新建 | 进阶润色 prompt（从 db.sql 抽出） |
| 3 | `crates/infra/seeds/llm_providers.json` | 新建 | 7 个 LLM provider 配置（从 db.sql 抽出） |
| 4 | `crates/infra/seeds/agent_actions/make-ppt.prompt.md` | 新建 | PPT 制作 prompt 模板 |
| 5 | `crates/infra/src/db.rs` | 修改 | `init_schema` 简化（删历史迁移）+ `load_external_seeds` 加载函数 |
| 6 | `crates/infra/src/db.sql` | 修改 | 删除 3 类内联 seed + 保留 schema 和其他短 seed |
| 7 | `crates/infra/Cargo.toml` | 修改 | `package.include` 加 `seeds/` 目录（release 打包） |
| 8 | `crates/desktop/src/action_bar_commands.rs` | 修改 | 新增 `restore_prompt_from_seed(prompt_id)` Tauri 命令 |
| 9 | `crates/desktop/frontend/src/pages/Settings/PromptsPanel.tsx` | 修改 | 编辑表单加「复原默认」按钮（仅 system prompt） |
| 10 | i18n key（zh-CN.yaml / en.yaml） | 修改 | 「复原默认」中英 |
| 11 | `docs/features/make-ppt.md` | 新建 | 用户向文档 |
| 12 | `docs/architecture.md` | 修改 | 同步外置 seed + Agent 菜单 + PPT 桥接说明 |

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

| # | 测试 | 验证什么 |
|---|---|---|
| 1 | `migration_v38_to_v39_runs_external_seeds` | v38 库 → v39 升级时正确读 seeds/ 文件、插入 prompts/llm_providers/Agent 菜单 |
| 2 | `migration_v0_to_v39_fresh_db` | 全新库走 INIT_SQL + 外置 seed 一次到位 |
| 3 | `migration_v39_skipped_on_already_v39` | 已 v39 的库直接 return，不读 seed 文件 |
| 4 | `external_seed_missing_file_does_not_break_schema` | seeds/ 目录被删时 schema 仍升级成功（log error 跳过） |
| 5 | `seed_idempotent_on_repeated_init` | 连续调 init_schema 两次，菜单/prompts 不重复 |
| 6 | `restore_prompt_from_seed_returns_correct_content` | `seed_prompt_path("default-polish")` 返回文件内容 |
| 7 | `render_make_ppt_prompt_replaces_placeholders` | `{{task}}` `{{files}}` 正确替换 |
| 8 | `agent_menu_visible_only_in_file_context`（前端 vitest） | text 场景 Agent 主菜单不可见；file 场景可见 |

**测试隔离**（参考 `db.rs:33` 既有约定）：测试时 seeds 文件路径必须能用 `CARGO_MANIFEST_DIR` 找到，避免 `cargo test` 时找不到文件。

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

## 10. 未来扩展（YAGNI）

- ❌ **octopus 内置 PPT 引擎**：偏离桥接器定位
- ❌ **目录递归**：让 agent 自己 walk 即可
- ❌ **Skill 安装检测**：让 agent 自己探 PATH
- ❌ **产物路径回收 / macOS 通知**：Terminal 打印路径已够用；未来若加，让 prompt 指示 agent 自己 osascript 发系统通知（零代码改动）
- ❌ **多 PPT 风格分支菜单**：一个 prompt + agent 决策已够；用户需要可自建多个菜单项
- ❌ **约束 agent 输出路径**：prompt 建议已够
- 💡 **后续可加 Agent 主菜单其他子菜单项**：「整理这个文件夹」「总结文档」「翻译全部」——同样的 agent 桥接机制，纯新增 prompt 文件
