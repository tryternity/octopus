# 从文件制作 PPT

> 通过 Actionbar 召唤外部 Agent（Pi / Claude Code）阅读文件并生成 PPT。

适用场景：你已经在 Finder 整理好需求文档 / 设计稿 / 周报素材，想口述一句风格要求就让 Agent 帮你产出一份演示文稿。

## 准备

### 1. 安装 Agent

至少装一个支持的 CLI agent：

| Agent | 安装 | 适配 |
|---|---|---|
| **Pi**（默认） | `npm install -g --ignore-scripts @earendil-works/pi-coding-agent` | `pi @file1 @file2 'prompt'` |
| Claude Code | 见 [claude.com/claude-code](https://claude.com/claude-code) | `claude --add-dir <cwd> 'prompt'` |

打开 **设置 → 智能体管理 → 刷新检测**，确认 Pi 已被识别（绿色 ✅）。如果没识别到，检查 `pi` 是否在登录 shell 的 `PATH` 里（`which pi` 验证）。

### 2. （可选）安装 PPT skill

octopus 内置的 prompt 会向 Agent 推荐 4 个 PPT skill，按你的偏好装一个或多个。**不装也行**——Agent 会用 HTML 手写一份基础 PPT（16:9 单文件）。

| skill | 适合 | 安装 |
|---|---|---|
| `guizang-ppt-skill`（默认推荐） | 瑞士风版式锁定、汇报场景，质量下限高 | `npx skills add https://github.com/op7418/guizang-ppt-skill --skill guizang-ppt-skill` |
| `lewislulu/html-ppt-skill` | 多主题可选（约 36 套：彩色 / 霓虹 / 科技 / dark） | `npx skills add https://github.com/lewislulu/html-ppt-skill` |
| `ppt-master`（python） | 需要可编辑的 `.pptx`，方便发给同事再改 | `git clone https://github.com/hugohe3/ppt-master.git && cd ppt-master && pip install -r requirements.txt` |
| `OfficeCLI` | 高保真 + 自愈（render→look→fix），重视觉一致性 | `curl -fsSL https://raw.githubusercontent.com/iOfficeAI/OfficeCLI/main/install.sh \| bash` |

skill 的选择规则在 prompt 里已写明，用户口述里出现「可编辑 / pptx / 改字」会优先走 `ppt-master` 或 `OfficeCLI`；出现具体风格词（瑞士风 / 暗色 / 霓虹 / 科技感 / 学术）会匹配对应 HTML PPT skill；不说偏好默认 `guizang-ppt-skill`。

## 使用

### 方式 A：通过 Actionbar 浮窗（默认）

1. 在 Finder 选中**文件**或**文件夹**（可多选）
2. 按全局热键（默认 `⌘⇧␣`）→ 浮窗弹出
3. 选 **Agent → 制作 PPT**
4. **自动开始录音** → 口述你的需求，例如：
   - 「做个瑞士风的，给老板看的」
   - 「可编辑的 .pptx」
   - 「暗色科技风，多图」
   - 「不要了，直接 HTML 简版」
5. 停止说话后自动结束录音 → Agent（Pi）在 Terminal.app 新窗口启动
6. 等 Agent 完成，**末尾会打印绝对路径**：

   ```
   ✅ ============================================
   ✅ PPT 已生成：/Users/xxx/.../2026-07-19-季度汇报.html
   ✅ ============================================
   ```

7. 在 Finder 按 `⌘⇧G` 粘贴路径定位，或在 Terminal 里 `⌘+点击` 路径直接打开。

### 方式 B：通过全局快捷键直接口述（需配置）

如果你希望跳过 Actionbar 浮窗、按一下快捷键就开始录音（Quick Execute 路径）：

1. **设置 → 命令面板** → 找到「制作 PPT」项
2. 在「全局快捷键」填一个组合（例如 `⌘⌥P`）
3. 保存
4. Finder 选中文件 → 按 `⌘⌥P` → **直接开始录音**（不弹浮窗）→ 录完 Agent 启动

> 若快捷键不生效，可能被系统或其他 app 占用——换一个组合重试。
>
> 这条路径会跳过浮窗但保留语音录音（因为 prompt 含 `{{voice}}` 占位符，需要口述需求）。如果你想完全不要语音，编辑 action 把 `{{voice}}` 占位符从 prompt 里删掉——但通常你都需要口述风格要求。

## 产物在哪里？

- **优先位置**：第一个选中文件的父目录（即 Agent 启动时的工作目录）
- **文件名**：`YYYY-MM-DD-<主题>.<扩展名>`
- **路径**：Agent 完成后会在 Terminal 末尾明确打印绝对路径——找不到就翻 Terminal 历史

如果你只选了文件夹没选文件，Agent 会用文件夹路径作为工作目录，产物会落在该文件夹里。

## 修改 prompt

**设置 → 命令面板** → 找到「制作 PPT」项 → 编辑 `action_data`（即 prompt 模板）。

修改方向举例：

- 加公司 logo / 字体 / 品牌色要求
- 改默认推荐 skill（替换 skill 表格）
- 调产物命名规则（例如固定放 `~/Documents/Slides/`）
- 加「不要包含哪些内容」的负面清单

改完保存即生效，下次口述就会用新 prompt。已编辑过的 prompt **不会被升级覆盖**——octopus 用 `INSERT OR IGNORE` 写入 seed，只影响首次安装。

## 改用其他 Agent

**设置 → 命令面板** → 找到「制作 PPT」项 → 把 `agent` 字段从 `pi` 改成 `claude`（需先装 Claude Code 并在「智能体管理」刷新检测到）。

也可以在 **设置 → 智能体管理** 自定义 adapter（自定义命令模板），然后在菜单项里选你的 adapter。

## 故障排查

| 现象 | 原因 | 解决 |
|---|---|---|
| 点「制作 PPT」报「Pi 未安装」 | PATH 找不到 `pi` | 装 Pi，或改 `agent=claude` |
| Agent 启动但报告「无文件可读」 | 选中的是空文件夹 | 选有实际文件的目录 |
| Agent 报告「需要装 X skill」 | 没装任何 PPT skill | 按提示装一个，或让 Agent fallback HTML 手写 |
| 录音结束 Agent 没启动 | ASR 文本为空 | 重试，或检查麦克风权限（系统设置 → 隐私 → 麦克风） |
| Terminal 没看到产物路径 | Agent 中途崩溃 | 翻 Terminal 历史看错误；通常 prompt 给的文件格式不被 skill 支持 |
| 全局快捷键方式 B 没反应 | 快捷键被占用或未保存 | 换组合；确认设置页保存成功 |
| 找不到「制作 PPT」菜单项 | DB 损坏或 seed 没写入 | 在「命令面板」手动新建一个 `action_type=agent`、`agent=pi`、`accepts=file` 的菜单项 |

## 内置 prompt

完整的内置 prompt 见仓库 [`crates/infra/seeds/agent_actions/make-ppt.prompt.md`](../../crates/infra/seeds/agent_actions/make-ppt.prompt.md)。

你可以直接编辑这个 seed 文件让默认 prompt 升级——影响的是新装用户；已装用户改各自的 `action_data` 即可（seed 写入用 `INSERT OR IGNORE`，不会覆盖已存在的行）。

prompt 里写死了 5 条不可省略的约束：
- 仅允许使用列出的 4 个 skill（不联网搜其他）
- 文件夹递归读取，跳过 `.git` / `node_modules` / 二进制
- 含敏感信息（API key / 密码）不写进 PPT，末尾告知跳过哪些
- 完成时必须打印**绝对路径**（不得相对）
- 中途失败必须明确说「未生成产物」
