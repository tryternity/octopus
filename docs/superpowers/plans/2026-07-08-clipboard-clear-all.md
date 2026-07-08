# 剪贴板浮窗「一键清理」实施计划

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 在剪贴板浮窗底部 BAR 增「清理」按钮，一键删除当前 tab 类别下所有非收藏条目，两步 inline 确认防误触。

**Architecture:** 后端复用现有 `build_where`（filter→SQL 单一权威）新增 `store::clear_history_by_filter`，保证「查询看到的 = 清理删除的」语义一致；新 Tauri 命令 `clear_clipboard_history_by_filter` emit 已有 `clipboard://changed`，前端 `useClipboardHistory` 自动 refresh；前端两步状态机（点 1 次变红 + 3s 超时，再点执行），filter 切换/卸载清 timer，收藏 tab 因 `is_favorite=1 AND is_favorite=0` 恒假删 0 条故禁用按钮。

**Tech Stack:** Rust（octopus-clipboard / octopus-desktop，Tauri 2，rusqlite）+ React 19 + TypeScript + lucide-react。测试：Rust `cargo test`（in-memory SQLite）、前端 vitest（仅纯逻辑，无组件测试基建）。

**Spec:** `docs/superpowers/specs/2026-07-08-clipboard-clear-all-design.md`

---

## ⚠️ worktree 执行注意

本仓库工作在 worktree `/Users/wudarui/workspace/agent/octopus/.claude/worktrees/scroll-stitch-debug`。Bash 的 cwd 实测是**主仓库**（非 worktree），故所有 cargo / npm / grep / git 命令必须显式指 worktree：

- Rust：`cargo test --manifest-path /Users/wudarui/workspace/agent/octopus/.claude/worktrees/scroll-stitch-debug/Cargo.toml -p <pkg> ...`
- 前端：`npm --prefix /Users/wudarui/workspace/agent/octopus/.claude/worktrees/scroll-stitch-debug/crates/desktop/frontend ...`
- git：`git -C /Users/wudarui/workspace/agent/octopus/.claude/worktrees/scroll-stitch-debug ...`
- Edit 工具用绝对路径（不受 cwd 影响）

下文为简洁，记 `WT=/Users/wudarui/workspace/agent/octopus/.claude/worktrees/scroll-stitch-debug`。

---

## 文件结构

| 文件 | 责任 | 改动 |
|------|------|------|
| `crates/clipboard/src/store.rs` | 剪贴板历史 CRUD（DB 层） | + `clear_history_by_filter` 函数（紧邻 `clear_history` L271）+ 5 个单测 |
| `crates/desktop/src/clipboard_commands.rs` | Tauri 命令层（DB 调用 + emit） | + `clear_clipboard_history_by_filter` 命令（紧邻 L76） |
| `crates/desktop/src/main.rs` | 命令注册（`generate_handler!`） | + 1 行注册（L231 后） |
| `crates/desktop/frontend/src/pages/Clipboard/index.tsx` | 剪贴板浮窗 UI | Footer 清理按钮 + 两步状态机 + 收藏禁用 + Trash2 import |

DB 层（store）可单测 → 命令层靠 cargo check + 前端 e2e → 前端靠 tsc + 手动验证（项目无 React 组件测试基建，不强造）。

---

## Task 1: `store::clear_history_by_filter`（TDD）

**Files:**
- Modify: `crates/clipboard/src/store.rs` — 新增 `clear_history_by_filter`（L281 `clear_history` 闭包后、L283 `// ── image_data CRUD ──` 前）+ tests 模块新增 5 测试（L403 `mod tests` 内）

- [ ] **Step 1: 写第一个失败测试**

在 `mod tests`（L403 起）末尾追加。复用现有 `open_test_db()`（L406）、`insert_clipboard_item` + `NewClipboardItem`、`toggle_favorite`、`query_history`：

```rust
#[test]
fn clear_history_by_filter_all_keep_favorite() {
    let conn = open_test_db();
    // 插 3 条文本（NewClipboardItem 无 is_favorite 字段，默认非收藏）
    for id in [1i64, 2, 3] {
        insert_clipboard_item(&conn, &NewClipboardItem {
            id, item_type: ItemType::Text, content: format!("c{}", id),
            ref_data: None, meta_info: None, created_at: iso_now(),
            has_thumbnail: None, is_rich: false,
        }).unwrap();
    }
    toggle_favorite(&conn, 3).unwrap(); // id=3 设为收藏
    let deleted = clear_history_by_filter(&conn, "all", true).unwrap();
    assert_eq!(deleted, 2);
    let remaining = query_history(&conn, &QueryFilter {
        filter: "all".into(), search: None, page: 1, size: 10,
    }).unwrap();
    assert_eq!(remaining.len(), 1);
    assert_eq!(remaining[0].id, 3);
    assert!(remaining[0].is_favorite);
}
```

- [ ] **Step 2: 跑测试确认失败（函数未定义）**

```bash
cargo test --manifest-path $WT/Cargo.toml -p octopus-clipboard --lib store::tests::clear_history_by_filter_all_keep_favorite
```
Expected: 编译失败 `cannot find function clear_history_by_filter`。

- [ ] **Step 3: 实现 `clear_history_by_filter`**

在 `clear_history`（L271-281）闭块后、`// ── image_data CRUD ──`（L283）前插入：

```rust
/// 按 filter（类型筛选）批量删除。复用 build_where 把 filter 转 SQL where，
/// keep_favorite=true 追加 AND is_favorite = 0（空 where 时 where 即 is_favorite = 0）。
/// 与 clear_history 对称：删后 cleanup_unreferenced_images 清孤立 image_data blob。
/// filter="favorite" + keep_favorite=true → "is_favorite = 1 AND is_favorite = 0" 恒假，删 0 条
/// （收藏 tab 自然结果，前端禁用按钮，后端无需特判）。
pub fn clear_history_by_filter(conn: &Connection, filter: &str, keep_favorite: bool) -> Result<usize> {
    let qf = QueryFilter { filter: filter.to_string(), search: None, page: 1, size: 1 };
    let mut where_clause = build_where(&qf);
    if keep_favorite {
        if where_clause.is_empty() {
            where_clause = "is_favorite = 0".to_string();
        } else {
            where_clause.push_str(" AND is_favorite = 0");
        }
    }
    let sql = if where_clause.is_empty() {
        "DELETE FROM clipboard_history".to_string()
    } else {
        format!("DELETE FROM clipboard_history WHERE {}", where_clause)
    };
    let rows = conn.execute(&sql, [])?;
    if rows > 0 {
        cleanup_unreferenced_images(conn)?;
    }
    Ok(rows)
}
```

- [ ] **Step 4: 跑测试确认通过**

```bash
cargo test --manifest-path $WT/Cargo.toml -p octopus-clipboard --lib store::tests::clear_history_by_filter_all_keep_favorite
```
Expected: PASS。

- [ ] **Step 5: 补 4 个场景测试**

在 `mod tests` 继续追加：

```rust
#[test]
fn clear_history_by_filter_text_only() {
    let conn = open_test_db();
    insert_clipboard_item(&conn, &NewClipboardItem {
        id: 1, item_type: ItemType::Text, content: "text".into(),
        ref_data: None, meta_info: None, created_at: iso_now(),
        has_thumbnail: None, is_rich: false,
    }).unwrap();
    insert_clipboard_item(&conn, &NewClipboardItem {
        id: 2, item_type: ItemType::Image, content: String::new(),
        ref_data: Some("hash2".into()), meta_info: None, created_at: iso_now(),
        has_thumbnail: Some(true), is_rich: false,
    }).unwrap();
    insert_image_data(&conn, "hash2", &[1, 2, 3], &[4, 5, 6], 10, 10).unwrap();
    let deleted = clear_history_by_filter(&conn, "text", true).unwrap();
    assert_eq!(deleted, 1);
    let remaining = query_history(&conn, &QueryFilter {
        filter: "all".into(), search: None, page: 1, size: 10,
    }).unwrap();
    assert_eq!(remaining.len(), 1);
    assert_eq!(remaining[0].item_type, ItemType::Image);
}

#[test]
fn clear_history_by_filter_favorite_deletes_zero() {
    let conn = open_test_db();
    for id in [1i64, 2] {
        insert_clipboard_item(&conn, &NewClipboardItem {
            id, item_type: ItemType::Text, content: format!("c{}", id),
            ref_data: None, meta_info: None, created_at: iso_now(),
            has_thumbnail: None, is_rich: false,
        }).unwrap();
        toggle_favorite(&conn, id).unwrap(); // 两条都设收藏
    }
    let deleted = clear_history_by_filter(&conn, "favorite", true).unwrap();
    assert_eq!(deleted, 0);
    let remaining = query_history(&conn, &QueryFilter {
        filter: "all".into(), search: None, page: 1, size: 10,
    }).unwrap();
    assert_eq!(remaining.len(), 2);
}

#[test]
fn clear_history_by_filter_keep_false_all() {
    let conn = open_test_db();
    insert_clipboard_item(&conn, &NewClipboardItem {
        id: 1, item_type: ItemType::Text, content: "c1".into(),
        ref_data: None, meta_info: None, created_at: iso_now(),
        has_thumbnail: None, is_rich: false,
    }).unwrap();
    toggle_favorite(&conn, 1).unwrap(); // 收藏条目 keep=false 时也应删
    let deleted = clear_history_by_filter(&conn, "all", false).unwrap();
    assert_eq!(deleted, 1);
    let remaining = query_history(&conn, &QueryFilter {
        filter: "all".into(), search: None, page: 1, size: 10,
    }).unwrap();
    assert!(remaining.is_empty());
}

#[test]
fn clear_history_by_filter_image_cleans_blob() {
    let conn = open_test_db();
    insert_clipboard_item(&conn, &NewClipboardItem {
        id: 1, item_type: ItemType::Image, content: String::new(),
        ref_data: Some("hash1".into()), meta_info: None, created_at: iso_now(),
        has_thumbnail: Some(true), is_rich: false,
    }).unwrap();
    insert_image_data(&conn, "hash1", &[1, 2, 3], &[4, 5, 6], 10, 10).unwrap();
    let before: i64 = conn.query_row("SELECT COUNT(*) FROM image_data", [], |r| r.get(0)).unwrap();
    assert_eq!(before, 1);
    let deleted = clear_history_by_filter(&conn, "image", true).unwrap();
    assert_eq!(deleted, 1);
    // cleanup_unreferenced_images 应清掉孤立 blob
    let after: i64 = conn.query_row("SELECT COUNT(*) FROM image_data", [], |r| r.get(0)).unwrap();
    assert_eq!(after, 0);
}
```

- [ ] **Step 6: 跑全量 store 测试确认无回归**

```bash
cargo test --manifest-path $WT/Cargo.toml -p octopus-clipboard --lib store::
```
Expected: 现有全部测试 + 新增 5 个全 PASS。

- [ ] **Step 7: Commit**

```bash
git -C $WT add crates/clipboard/src/store.rs
git -C $WT commit -m "feat(clipboard): store clear_history_by_filter 按 filter 批量删非收藏

复用 build_where 拼 WHERE + AND is_favorite = 0，与 clear_history
对称（含 cleanup_unreferenced_images）。5 单测覆盖 all/text/favorite(删0)/
keep=false/image blob 级联。"
```

---

## Task 2: Tauri 命令 + 注册

**Files:**
- Modify: `crates/desktop/src/clipboard_commands.rs` — 新增命令（L76 `clear_clipboard_history` 闭块后）
- Modify: `crates/desktop/src/main.rs` — 注册（L231 `clear_clipboard_history,` 后一行）

- [ ] **Step 1: 新增命令**

在 `clear_clipboard_history`（clipboard_commands.rs L65-76）闭块后插入。镜像它，多一个 `filter` 参数：

```rust
#[tauri::command]
pub async fn clear_clipboard_history_by_filter(
    filter: String,
    keep_favorite: bool,
    app_handle: tauri::AppHandle,
) -> Result<usize, String> {
    let n = octopus_infra::db::with_db(|conn| {
        octopus_clipboard::store::clear_history_by_filter(conn, &filter, keep_favorite)
    })
    .map_err(|e| e.to_string())?;
    let _ = app_handle.emit("clipboard://changed", ());
    Ok(n)
}
```

- [ ] **Step 2: 注册命令**

`crates/desktop/src/main.rs` L231 `clipboard_commands::clear_clipboard_history,` 后加一行（保持与上下文缩进一致，14 空格）：

```rust
            clipboard_commands::clear_clipboard_history_by_filter,
```

- [ ] **Step 3: cargo check 验证编译**

```bash
cargo check --manifest-path $WT/Cargo.toml -p octopus-desktop
```
Expected: 编译通过，无错误（warning 可接受）。

- [ ] **Step 4: Commit**

```bash
git -C $WT add crates/desktop/src/clipboard_commands.rs crates/desktop/src/main.rs
git -C $WT commit -m "feat(clipboard): clear_clipboard_history_by_filter 命令 + 注册

镜像 clear_clipboard_history 多 filter 参数，emit clipboard://changed
触前端自动 refresh。旧命令保留不动。"
```

---

## Task 3: 前端 Footer 清理按钮 + 两步状态机

**Files:**
- Modify: `crates/desktop/frontend/src/pages/Clipboard/index.tsx` — Trash2 import、confirming state + confirmTimer ref、filter 切换/卸载清 timer、Footer 按钮改造

> 项目前端测试基建只有纯逻辑模块测试（`*.test.ts`，无 `@testing-library/react`）。两步状态机是组件交互 + setTimeout 副作用，强造组件单测需引入 testing-library（超范围，YAGNI）。本 Task 靠 tsc + 手动验证；spec §6.2 行为项作为手动检查清单（见 Step 7）。

- [ ] **Step 1: 补 Trash2 import**

L10 现有：
```tsx
import { Pin, X, Settings2, CircleCheck, CircleX } from "lucide-react";
```
改为：
```tsx
import { Pin, X, Settings2, CircleCheck, CircleX, Trash2 } from "lucide-react";
```

- [ ] **Step 2: 加 confirming state + confirmTimer ref**

在 `const [recording, setRecording] = useState(true);`（L26）后加：
```tsx
// 一键清理两步确认：点 1 次 → confirming=true（变红 + 3s 超时），再点才执行。
const [confirming, setConfirming] = useState(false);
const confirmTimer = useRef<number | null>(null);
```

- [ ] **Step 3: filter 切换 + 卸载时清 timer**

在 items 夹紧的 `useEffect`（L33-40）后追加两个 effect：
```tsx
// filter 切换 → 清确认态 + 清 timer（避免在 A tab 点了第一步、切到 B tab 后第二次点击误清 B）
useEffect(() => {
  setConfirming(false);
  if (confirmTimer.current) {
    clearTimeout(confirmTimer.current);
    confirmTimer.current = null;
  }
}, [filter]);

// 卸载清 timer，防泄漏
useEffect(() => {
  return () => {
    if (confirmTimer.current) clearTimeout(confirmTimer.current);
  };
}, []);
```

- [ ] **Step 4: 改造 Footer（L234-245 整块替换）**

现有 Footer：
```tsx
{/* Footer */}
<div className="flex items-center justify-between px-3 py-1 border-t border-border text-[10px] text-muted-foreground/80">
  <span>{total} 条</span>
  <button
    className="flex items-center gap-0.5 hover:text-foreground transition-colors"
    onClick={() => invoke("open_settings", { initialPage: "clipboard" })}
    title="管理剪贴板"
  >
    <Settings2 className="w-2.5 h-2.5" />
    管理
  </button>
</div>
```
替换为（右侧两按钮成组，清理在前、管理在后）：
```tsx
{/* Footer */}
<div className="flex items-center justify-between px-3 py-1 border-t border-border text-[10px] text-muted-foreground/80">
  <span>{total} 条</span>
  <div className="flex items-center gap-2">
    {/* 一键清理：删当前 tab 类别下所有非收藏条目（与搜索框正交）。
        两步确认：点 1 次 → 变红「再点确认」+ 3s 超时，再点才执行。
        收藏 tab 因 is_favorite=1 AND is_favorite=0 恒假删 0 条，禁用按钮。 */}
    <button
      className={cn(
        "flex items-center gap-0.5 transition-colors",
        filter === "favorite"
          ? "opacity-50 cursor-not-allowed"
          : confirming
            ? "text-red-500"
            : "hover:text-foreground",
      )}
      disabled={filter === "favorite"}
      title={
        filter === "favorite"
          ? "收藏标签下无可清理项"
          : confirming
            ? "再点一次确认清理"
            : "清理非收藏"
      }
      onClick={() => {
        if (filter === "favorite") return;
        if (!confirming) {
          setConfirming(true);
          confirmTimer.current = window.setTimeout(() => {
            setConfirming(false);
            confirmTimer.current = null;
          }, 3000);
        } else {
          if (confirmTimer.current) {
            clearTimeout(confirmTimer.current);
            confirmTimer.current = null;
          }
          setConfirming(false);
          invoke("clear_clipboard_history_by_filter", { filter, keepFavorite: true }).catch(console.error);
        }
      }}
    >
      <Trash2 className="w-2.5 h-2.5" />
      {confirming ? "再点确认" : "清理"}
    </button>
    <button
      className="flex items-center gap-0.5 hover:text-foreground transition-colors"
      onClick={() => invoke("open_settings", { initialPage: "clipboard" })}
      title="管理剪贴板"
    >
      <Settings2 className="w-2.5 h-2.5" />
      管理
    </button>
  </div>
</div>
```

- [ ] **Step 5: tsc 类型检查**

```bash
npm --prefix $WT/crates/desktop/frontend run build
```
（vite build 含 tsc 类型检查）
Expected: 编译成功，无 TS 错误。

- [ ] **Step 6: Commit**

```bash
git -C $WT add crates/desktop/frontend/src/pages/Clipboard/index.tsx
git -C $WT commit -m "feat(clipboard): 浮窗底栏一键清理按钮 + 两步确认

右侧与「管理」成组，两步 inline（点1次变红+3s超时，再点执行），
filter 切换/卸载清 timer，收藏 tab 禁用。与搜索框正交。"
```

- [ ] **Step 7: 手动验证清单（spec §6.2，无组件单测故手动）**

构建桌面应用后实跑剪贴板浮窗，逐项确认：
- [ ] 「全部」tab：插几条非收藏 + 1 条收藏 → 点清理 → 变红「再点确认」→ 3s 内再点 → 非收藏消失、收藏留。
- [ ] 3s 超时：点 1 次后等 3s → 自动回「清理」灰色。
- [ ] filter 切换：点 1 次进确认态 → 切 tab → 按钮回「清理」（确认态被重置）。
- [ ] 「文本」tab：只删非收藏文本，图片/语音等其他类别不动。
- [ ] 「收藏」tab：按钮 disabled（opacity-50 + cursor-not-allowed），点击无反应，tooltip「收藏标签下无可清理项」。
- [ ] 搜索框有内容时点清理：仍删整个 tab 类别非收藏（与搜索词无关）。
- [ ] image 条目清理后无内存/DB 膨胀（cleanup_unreferenced_images 已在 Task 1 单测覆盖，手动仅看无异常）。

---

## Self-Review

**1. Spec 覆盖：**
- §2.1 `clear_history_by_filter` + filter 语义表 → Task 1（实现 + all/text/favorite/keep=false 4 测试覆盖表的全部分支语义）
- §2.1 image blob 级联 → Task 1 `clear_history_by_filter_image_cleans_blob`
- §2.2 命令 + emit → Task 2 Step 1
- §2.3 注册 → Task 2 Step 2
- §3.1 位置（与「管理」成组）→ Task 3 Step 4
- §3.2 两步状态机 + 3s + filter 切换/卸载清 timer → Task 3 Step 2/3/4
- §3.3 收藏 tab 禁用 → Task 3 Step 4（disabled + tooltip）
- §1 与搜索框正交 → Task 3 Step 7 手动清单 + spec 已述
- §6.1 store 单测 5 项 → Task 1 全覆盖
- §6.2 前端行为 → Task 3 Step 7 手动清单
- 无遗漏。

**2. Placeholder 扫描：** 全部 Step 含完整代码 / 确切命令 / 预期输出，无 TBD/TODO/"add error handling" 等。

**3. 类型一致性：**
- `clear_history_by_filter(conn: &Connection, filter: &str, keep_favorite: bool) -> Result<usize>` — Task 1 定义、Task 2 命令内调用 `clear_history_by_filter(conn, &filter, keep_favorite)` 一致。
- 命令名 `clear_clipboard_history_by_filter` — Task 2 定义、Task 3 前端 `invoke("clear_clipboard_history_by_filter", ...)` 一致。
- 前端参数 `{ filter, keepFavorite: true }` — Tauri 自动 snake/camel 转 `filter` + `keep_favorite`，与命令签名一致。
- `confirming` / `confirmTimer` 命名跨 Step 2/3/4 一致。
- 无类型/命名漂移。
