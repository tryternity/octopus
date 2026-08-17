# toggle_favorite 事务化 + vault import id 校验 设计

> 2026-08-14 · P3 防御性加固

## P3-1: toggle_favorite 非事务

`toggle_favorite`（clipboard/favorite.rs）的 2-3 个 DB 操作各自走独立 `with_db`（独立连接 autocommit），中间失败留不一致（favorite 表已改但 history.is_favorite 未改）。

**修复**：
- infra: 4 个 `_at` 函数（`insert_favorite_at`/`soft_delete_favorite_at`/`load_favorite_at`/`restore_favorite_at`）从 `pub(crate)` 提 `pub`；新增 `set_clipboard_is_favorite_at`
- clipboard: `toggle_favorite` 改用 `with_db(|conn| { ... unchecked_transaction ... })` 单连接事务

## P3-2: vault import id 校验对称

`import_all_from_files`（vault/sync/store.rs）从 JSON 读 cipher/folder id 不校验。path 构造入口已有 `validate_uuid`，但 JSON 内容的 id 可能与文件名不同。

**修复**：解析后加 `validate_uuid(&file.id)` 检查，非法 id skip + warn（同 corrupt file 容错模式）。
