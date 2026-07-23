# 任务

阅读以下文件并制作成 PPT（演示文稿）。文件清单：

{{files}}

> 若上方清单含目录路径（不是单个文件），请用 `ls <dir>` / `find <dir> -type f` 递归列出目录下所有支持的文本文件（md/txt/docx/pdf/code），跳过 `.git` / `node_modules` / 二进制文件。**不要尝试用 `@<dir>` 方式读取目录**——会 EISDIR 报错，必须先展开为文件列表再逐个读。

# 用户的额外指令

{{voice}}

# 推荐的 PPT Skill 清单（按需选一）

你被允许使用以下 4 个 PPT skill 之一。**不要联网搜索其他 skill**——只用本清单。

| 路线 | skill 名 | 安装命令 | 关键词 | 输出 |
|---|---|---|---|---|
| HTML PPT（瑞士风/版式锁定，质量下限高） | `guizang-ppt-skill` | `npx skills add https://github.com/op7418/guizang-ppt-skill --skill guizang-ppt-skill` | 默认 / "专业" "汇报" "正式" | 单文件 HTML |
| HTML PPT（多主题可选） | `lewislulu/html-ppt-skill` | `npx skills add https://github.com/lewislulu/html-ppt-skill` | "彩色" "霓虹" "科技" "dark" "主题" | 单文件 HTML |
| 原生可编辑 PPTX | `ppt-master`（python） | `git clone https://github.com/hugohe3/ppt-master.git && cd ppt-master && pip install -r requirements.txt` | "可编辑" "PowerPoint" "pptx" "改字" | .pptx |
| Office DOM（高保真 + 自愈） | `OfficeCLI` | `curl -fsSL https://raw.githubusercontent.com/iOfficeAI/OfficeCLI/main/install.sh | bash` | "office" "dom" "结构化" "高保真" | .pptx + 渲染 |

# Skill 选择规则

1. 用户提到「可编辑 / pptx / 改字 / PowerPoint / 给同事共享 .pptx」→ 优先 `ppt-master` 或 `OfficeCLI`
2. 用户提到具体风格（瑞士风/暗色/霓虹/科技感/学术）→ 选对应的 HTML PPT skill（关键词匹配主题）
3. 用户没说偏好 → 默认 `guizang-ppt-skill`（版式锁定，质量下限高）
4. 用户指定其他 skill（明确说出名字）→ 尊重用户选择，但你需提醒不在本清单内可能有未知风险

# 未装 Skill 的降级策略

1. **首选**：告诉用户需要装哪个 + 给出完整安装命令（上方表格里的）。用户装完后让他重新跑这个任务。
2. **fallback**：若用户希望立即产出，直接用 HTML 手写一份单文件 PPT：
   - 16:9 固定宽高比
   - 含封面 / 目录 / 章节 / 正文 / 结尾页
   - 内联 CSS，零依赖，浏览器打开即放映
   - 视觉简洁专业（白底深色字 / 一种强调色）

**不要尝试联网搜索其他 PPT skill——只用本 prompt 列出的 4 个。**

# 特殊输入：Markdown 大纲

若 {{files}} 含 `.md` 文件，**视为用户已经 review 过的 PPT 大纲**（来自「PPT 大纲」菜单的中间产物），按以下规则处理：

1. **跳过 guizang Step 1 的 7 个澄清问题**，直接进入 Step 2（拷模板）+ Step 3（按每页 H2 填充）
2. **front matter 的 `style` 字段决定用哪种风格**：
   - `style: A` → 电子杂志风（guizang-ppt-skill 默认 / lewislulu）
   - `style: B` → 瑞士国际主义风（guizang-ppt-skill 风格 B）
3. **每个 `## Pxx` 是一页**，下面的 `-` bullet 是该页要点
4. **不要二次总结、不要改变页数和顺序**——用户的编辑是故意的，照搬即可
5. 大纲 front matter 里若没有 `audience` / `duration_min` / `style`，按默认值（30min / style A）处理

> 如果 `.md` 文件看起来不像 PPT 大纲（无 front matter、无 `## Pxx` 结构），按普通文本文件处理，正常走 Step 1。

# 文件读取约束

- 若传入的是**文件夹**：递归列出文件（`ls -R` 或 walk），跳过 `.git` / `node_modules` / 二进制文件（图片/视频/可执行文件）
- 若传入的是**多个文件**：阅读每个文件后**统一规划 PPT 结构**，不要每文件一页
- 若只有音频/视频文件：先转写（可调用系统 ASR 或下载工具），再用文本生成 PPT
- 若文件含敏感信息（API key、密码），**不要写进 PPT**，并在最后告知用户跳过了哪些内容

# 完成后的强制披露（不可省略）

PPT 生成完成后，你必须在 Terminal 输出的最后一段明确告知用户：

```
✅ ============================================
✅ PPT 已生成：/Users/xxx/your-path/your-deck.html
✅ 打开方式：在 Finder 中按 Cmd+Shift+G 粘贴路径，或直接 Cmd+点击上方路径
✅ ============================================
```

要求：
- 路径必须是**绝对路径**（不要相对路径）
- 优先把产物放在用户当前工作目录下（即第一个选中文件的父目录）
- 文件名要有意义：`YYYY-MM-DD-<主题简述>.<扩展名>`
- 若有多份产物（HTML + PDF + PPTX），全部列出
- 若中途失败，必须明确说「未生成产物」，不要让用户误以为成功
