# 终端文件树侧栏（右侧，默认隐藏）

> 内嵌终端增强。右侧文件树侧栏，展示当前 cwd 的文件结构，默认隐藏可切换。

**日期**：2026-07-31
**蓝本**：Terax `modules/explorer/`（简化版——去掉 git 状态/拖放/CRUD）

## 目标

终端窗口右侧加一个文件树侧栏，展示当前活跃 tab 的 cwd 文件结构。默认隐藏，通过工具条切换展开/收缩。

## 范围

- ✅ 右侧侧栏，默认隐藏（收缩态小长条 `<<`）
- ✅ 展开态：上方工具条（`>>` 收起 + 隐藏文件切换）+ 下方文件树
- ✅ 文件树根目录跟随当前活跃 tab 的 trackedCwd（OSC 7）
- ✅ 懒加载：点击目录展开 → invoke 加载子项；目录优先 + 字母排序
- ✅ 默认隐藏 dot 文件 + gitignore 文件，工具条切换显示
- ✅ 文件点击仅高亮（Phase 1）
- ❌ git 状态着色（YAGNI）
- ❌ 拖放进终端 / 文件编辑 / 创建 / 删除
- ❌ 文件搜索

## 架构

### 布局

```
展开态：
┌──────────────────┬─────────────┐
│                  │ >>  👁       │ ← 工具条（上方，横向）
│  终端区           │─────────────│
│                  │ 📁 src/      │ ← 文件树（下方，滚动）
│                  │ 📄 README.md │
└──────────────────┴─────────────┘

收缩态：
┌────────────────────────────┬─┐
│  终端区                     │<<│ ← 小长条（点击展开）
└────────────────────────────┴─┘
```

### 数据流

```
当前 tab trackedCwd（OSC 7）
        ↓
FileTreePanel 根目录
        ↓ 点击目录展开
invoke terminal_list_dir(path, show_hidden)
        ↓ Rust：fs::read_dir + ignore crate（gitignore 过滤）
Vec<FileEntry { name, kind: "dir"|"file" }>
        ↓ 前端：目录优先 + 字母排序
渲染树节点
```

### Rust 命令 `terminal_list_dir`

```rust
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct FileEntry {
    name: String,
    kind: String,  // "dir" | "file"
}

#[tauri::command]
fn terminal_list_dir(path: String, show_hidden: bool) -> Result<Vec<FileEntry>, String>
```

- `fs::read_dir` 列直接子项
- `show_hidden=false`：过滤 dot 前缀文件
- gitignore 过滤：用 `ignore` crate（对齐 Terax `fs_read_dir`），需加依赖
- 目录优先 + case-insensitive 排序
- 错误（无权限/不存在）返回空数组（不阻断 UI）

### 前端组件

**`FileTreePanel.tsx`**（新组件）：
```typescript
type Props = {
  cwd: string | null;       // 当前 tab 的 trackedCwd
  expanded: boolean;        // 侧栏展开/收缩
  onToggle: () => void;     // 切换展开/收缩
};
```

内部状态：
- `showHidden: boolean`——工具条切换
- `tree: Record<string, FileEntry[] | "loading" | undefined>`——展开的目录路径 → 子项（懒加载）
- `expandedDirs: Set<string>`——当前展开的目录集合

交互：
- 收缩态：小长条 `<<`，点击 → `onToggle()` 展开
- 展开态：
  - 工具条：`>>`（收起）+ `👁`（切换隐藏文件）
  - 文件树：根目录（cwd）→ 展开的子目录递归渲染
  - 点击目录 → toggle 展开（未加载则 invoke）
  - 点击文件 → 高亮选中（Phase 1 无操作）

### CSS

- 展开态宽度 240px，收缩态宽度 24px（小长条）
- 工具条 32px 高，flex 横向
- 文件树 overflow-y auto，缩进按层级
- 过渡动画 width transition

### integration（index.tsx）

Terminal 组件加 `fileTreeOpen` state（默认 false）。布局条件渲染：
- `fileTreeOpen` → `<FileTreePanel cwd={activeTabCwd} expanded onToggle={...} />`
- `!fileTreeOpen` → 收缩态长条

切换入口：当前放在**左侧 sidebar header**（文件树图标按钮），或**终端区右上角**。但用户要求工具条在右侧侧栏本身——所以切换入口就是收缩态长条 / 展开态工具条的 `>>`/`<<`。

## 不变量

1. 侧栏默认隐藏（fileTreeOpen=false），用户主动展开
2. 文件树根目录始终是当前活跃 tab 的 trackedCwd（cd 后跟随）
3. dot/gitignore 文件默认隐藏，用户可切换
4. 懒加载——只加载展开的目录，不预加载整棵树

## 测试策略

- **Rust `terminal_list_dir`**：用 tempdir 造测试目录结构，验证目录优先排序 + dot 过滤 + gitignore 过滤——TDD
- **前端 FileTreePanel**：依赖真实 DOM + IPC，靠 e2e 冒烟（展开/收起/展开目录/切换隐藏）

## 依赖

| 新增 | 版本 | 用途 |
|---|---|---|
| `ignore` | 0.4 | gitignore 过滤（对齐 Terax fs/tree.rs） |

## 风险

1. **`ignore` crate 依赖**：Terax 已用，成熟可靠。Rust 端 read_dir + ignore walk 在大目录可能慢——懒加载只列直接子项，不递归。
2. **trackedCwd 为 null**（OSC 7 未收到）：文件树显示「等待目录...」或 home 兜底。
3. **权限**：macOS 某些目录（/System 等）无权限——返回空数组，不报错。
