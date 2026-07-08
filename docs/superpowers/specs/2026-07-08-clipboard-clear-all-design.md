# 剪贴板浮窗「一键清理」设计规格

**日期**：2026-07-08
**范围**：`crates/desktop/frontend/src/pages/Clipboard/index.tsx`（浮窗底栏按钮 + 两步确认）+ `crates/clipboard/src/store.rs`（按 filter 批量删除）+ `crates/desktop/src/clipboard_commands.rs`（新命令）+ `crates/desktop/src/main.rs`（注册）。
**目标**：在剪贴板浮窗底部 BAR 增「清理」按钮，一键删除**当前 tab 类别下所有非收藏条目**，删除前两步确认防误触。

---

## 1. 背景与动机

剪贴板历史累积快，浮窗是日常使用入口，缺一个「快速清掉非收藏」的按钮（全量清理当前只在设置页，按 id 删）。

需求要点（用户确认）：
- **范围 = 当前 tab 类别**（非全局）：在「全部」删所有非收藏；在「文本」只删非收藏文本；在「图片」只删非收藏图片……
- **二次确认**：防误触。形式选「两步按钮 inline」——点 1 次变红「再点确认」，3s 内再点才执行。
- **收藏豁免**：永远只删非收藏（`is_favorite = 0`）。
- **与搜索框正交**：清理按 tab 类别删非收藏，**不受搜索框内容影响**（搜索是临时筛选、清理是清空这一类；`build_where` 不接 `search`，故搜索词不进 DELETE WHERE）。用户若在「文本」tab 搜了「abc」再点清理，删的是**所有非收藏文本**而非仅匹配「abc」的。

---

## 2. 后端设计

### 2.1 `store::clear_history_by_filter`（新增，`store.rs`，紧邻 `clear_history` L271）

复用现有 `build_where`（`store.rs:190`）把 filter 转 SQL where 子句，与现有 `clear_history`（`store.rs:271`）对称：

```rust
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
    if rows > 0 { cleanup_unreferenced_images(conn)?; }
    Ok(rows)
}
```

`build_where` 是同模块 private `fn`，`clear_history_by_filter` 同在 `store.rs` 可直接调；`QueryFilter` 已在 store.rs 作用域内（`query_history` 已用）。`build_where` 只读 `qf.filter` 字段，故 `search/page/size` 填占位即可。

**filter 语义对照**（前端 `TABS_VALUES` ↔ `build_where` ↔ `keep_favorite=true` 结果）：

| 前端 tab | filter 值 | build_where | + AND is_favorite=0 | 删除效果 |
|----------|-----------|-------------|---------------------|----------|
| 全部 | `all` | （空） | `is_favorite = 0` | 删所有非收藏 |
| 文本 | `text` | `item_type = 'text'` | `item_type = 'text' AND is_favorite = 0` | 删非收藏文本 |
| 图片 | `image` | `item_type = 'image'` | 同上模式 | 删非收藏图片 |
| 语音 | `asr` | `item_type = 'voice'` | 同上模式 | 删非收藏语音 |
| OCR | `ocr` | `item_type = 'ocr'` | 同上模式 | 删非收藏 OCR |
| 文件 | `file` | `item_type = 'file'` | 同上模式 | 删非收藏文件 |
| 收藏 | `favorite` | `is_favorite = 1` | `is_favorite = 1 AND is_favorite = 0` | **删 0 条**（恒假） |

**收藏 tab 删 0 条**是 build_where + keep_favorite 的自然结果（恒假 WHERE）。前端在该 tab 下**禁用清理按钮**（§3.3），避免用户点了没反应的困惑；后端无需特判，恒假 DELETE 无副作用。

### 2.2 新命令 `clear_clipboard_history_by_filter`（`clipboard_commands.rs`，紧邻 L76）

镜像现有 `clear_clipboard_history`（`clipboard_commands.rs:65`），多一个 `filter` 参数：

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

- **保留旧 `clear_clipboard_history`** 不动：零成本，避免破坏潜在调用方 / 未来全局清理需求。
- emit `clipboard://changed` → `useClipboardHistory` 自动 refresh，前端无需手动刷新。
- Tauri 自动把命令参数 `keep_favorite` ↔ 前端 `keepFavorite` 做 snake/camel 转换。

### 2.3 注册（`main.rs:231` `clear_clipboard_history,` 后一行）

`clipboard_commands::clear_clipboard_history_by_filter,`

---

## 3. 前端设计（`Clipboard/index.tsx`）

### 3.1 位置（Footer L234-245）

右侧与「管理」成组——左 `{total} 条`，右 `[🗑 清理] [⚙管理]`：

```tsx
<div className="flex items-center justify-between px-3 py-1 border-t border-border text-[10px] text-muted-foreground/80">
  <span>{total} 条</span>
  <div className="flex items-center gap-2">
    {/* 清理：两步确认 */}
    <button ...>...</button>
    {/* 管理：现有，不动 */}
    <button onClick={() => invoke("open_settings", { initialPage: "clipboard" })} ...>
      <Settings2 className="w-2.5 h-2.5" /> 管理
    </button>
  </div>
</div>
```

`Trash2` 图标从已 import 的 `lucide-react` 补（现有 import 行 L10）。

### 3.2 两步状态机（用户选的 inline 两步）

新增 state：
```tsx
const [confirming, setConfirming] = useState(false);
const confirmTimer = useRef<number | null>(null);
```

状态转换：
1. **idle**（confirming=false）：文案「清理」，muted 色。
2. 点 1 次 → **confirming=true**：变红（`text-red-500`）+ 文案「再点确认」+ 启动 **3s** `window.setTimeout`（到期 → setConfirming(false) + 清 timer）。
3. 计时内再点 → `invoke("clear_clipboard_history_by_filter", { filter, keepFavorite: true })`（catch console.error）+ 立即 setConfirming(false) + 清 timer。
4. 3s 到期未点 → 自动回 idle。

点击 handler 逻辑：
```tsx
onClick={() => {
  if (!confirming) {
    setConfirming(true);
    confirmTimer.current = window.setTimeout(() => {
      setConfirming(false);
      confirmTimer.current = null;
    }, 3000);
  } else {
    if (confirmTimer.current) { clearTimeout(confirmTimer.current); confirmTimer.current = null; }
    setConfirming(false);
    invoke("clear_clipboard_history_by_filter", { filter, keepFavorite: true }).catch(console.error);
  }
}}
```

**timer 清理**（防泄漏 / 防跨 tab 误触发）：
- `filter` 变化 → 清 timer + 回 idle（避免在 A tab 点了第一步、切到 B tab 后计时还在跑，第二次点击误清 B）：
  ```tsx
  useEffect(() => {
    setConfirming(false);
    if (confirmTimer.current) { clearTimeout(confirmTimer.current); confirmTimer.current = null; }
  }, [filter]);
  ```
- 组件卸载 → 清 timer（useEffect cleanup）。
- 浮窗 hide 不单独处理（hide ≠ 卸载，state 保留；filter 切换 + 卸载已覆盖主场景，hide 期间 timer 到期回 idle 无副作用）。

### 3.3 收藏 tab 禁用

`filter === "favorite"` 时按钮 `disabled`（`opacity-50 cursor-not-allowed`）+ `title="收藏标签下无可清理项"`，不进入两步流程。理由见 §2.1 表（删 0 条）。

---

## 4. 数据流

```
[点 清理] → [点 再点确认]
  → invoke("clear_clipboard_history_by_filter", { filter, keepFavorite: true })
  → store::clear_history_by_filter (with_db)
  → DELETE FROM clipboard_history WHERE {build_where(filter)} AND is_favorite = 0
  → cleanup_unreferenced_images（清孤立 image_data blob）
  → emit "clipboard://changed"
  → useClipboardHistory 监听 → refresh → 列表更新（非收藏条目消失，total 下降）
```

`useClipboardHistory` 已监听 `clipboard://changed`（现有 toggle/delete 路径同机制），零新增事件订阅。

---

## 5. 改动文件清单

| 文件 | 改动 |
|------|------|
| `crates/clipboard/src/store.rs` | + `clear_history_by_filter`（L281 后，`clear_history` 下方） |
| `crates/desktop/src/clipboard_commands.rs` | + `clear_clipboard_history_by_filter` 命令（L76 后） |
| `crates/desktop/src/main.rs` | + 注册（L231 后一行） |
| `crates/desktop/frontend/src/pages/Clipboard/index.tsx` | Footer 清理按钮 + 两步状态机 + 收藏 tab 禁用 + Trash2 import |

---

## 6. 测试

### 6.1 store 单测（`store.rs` tests 模块，复用现有 in-memory DB helper）
- `clear_history_by_filter_all_keep_favorite`：插 3 条（2 非收藏 + 1 收藏），filter="all" keep=true → 删 2，剩 1 收藏。
- `clear_history_by_filter_text_only`：插 text + image 各 1 非收藏，filter="text" keep=true → 删 1 text，image 留。
- `clear_history_by_filter_image_cleans_blob`：插 image 条目 + 对应 image_data blob，filter="image" → 删条目 + `cleanup_unreferenced_images` 清孤立 blob（断言 image_data 行数减少）。
- `clear_history_by_filter_favorite_deletes_zero`：插 2 收藏，filter="favorite" keep=true → 删 0，2 条均留。
- `clear_history_by_filter_keep_false_all`：filter="all" keep=false → 全删（含收藏）。

### 6.2 前端（两步状态机行为）
- 点 1 次 → confirming=true + 文案变「再点确认」+ 红色。
- 计时内再点 → invoke 被调，参数 `{ filter, keepFavorite: true }`。
- 3s 到期未点 → 自动回 idle（文案回「清理」）。
- filter 切换 → 清 timer + 回 idle（已起确认态被重置）。
- favorite tab → 按钮 disabled，点击无反应。

### 6.3 回归
- 现有 `clear_history` 单测不受影响（未改该函数）。
- `useClipboardHistory` 在 `clipboard://changed` 后 refresh 行为不变。

---

## 7. 设计决策与遗留

- **复用 build_where 而非特判 filter**：filter→SQL 映射已有单一权威 `build_where`，清理路径复用它，保证「查询看到的 = 清理删除的」语义一致（不会出现查询能看到的条目清不掉、或清掉了查询看不到的条目）。
- **两步 inline 而非弹窗**：浮窗是轻量快速入口，弹窗打断流；两步按钮就地确认更轻。3s 超时避免确认态卡住。
- **收藏 tab 禁用而非删 0**：删 0 条技术无害，但用户点了「清理」却什么都没发生是困惑；禁用 + tooltip 更明确。
- **保留旧 clear_clipboard_history**：零成本，避免破坏潜在调用方。
- **YAGNI**：不做撤销、不做「将清理 N 条」预览计数（invoke 后 refresh 即显示结果）、不做按搜索词清理（范围 = tab 类别，已确认）。
