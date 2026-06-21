# 模型管理 GUI 接入 实施计划

> spec：`docs/superpowers/specs/2026-06-21-model-management-gui-design.md`。worktree `model-mgmt-ui`。

**Goal**：设置窗口「模型管理」页（页面3）从占位变成可浏览/下载 HF 模型、看实时进度。

**Architecture**：后端新模块 `model_commands.rs`（list + download，复用 download crate，mpsc→Tauri 事件）；前端独立 `models.js`（`index.html` 仅两处局部改动，隔离与 setting-ui2 的冲突）。

---

## Task 1：后端 model_commands.rs

**Files：** Create `crates/desktop/src/model_commands.rs`

- [ ] 写 `DownloadableModel` DTO + `is_hf_repo` 判定函数
- [ ] 写 `list_downloadable_models` 命令（list_engines + resolve 取 source + resolve_model_dir 判 downloaded）
- [ ] 写 `download_model` async 命令（HfRequest + resolve_tasks + mpsc 转发 emit download-progress/download-file）
- [ ] `#[cfg(test)]` 单测：is_hf_repo 各分支

## Task 2：后端接线

**Files：** `crates/desktop/Cargo.toml`、`crates/desktop/src/main.rs`

- [ ] Cargo.toml 加 `octopus-download = { path = "../download" }`
- [ ] main.rs 加 `mod model_commands;`
- [ ] invoke_handler 注册 `model_commands::list_downloadable_models` / `model_commands::download_model`
- [ ] `cargo check -p octopus-desktop` 通过

## Task 3：前端 models.js

**Files：** Create `crates/desktop/dist/settings/models.js`

- [ ] `renderModels()`：invoke list → 卡片列表（名称/category/description/downloaded 或下载按钮）
- [ ] 下载：invoke download_model + listen download-file/download-progress → 进度条
- [ ] 镜像输入框：get_config 回填 + set_config 写入
- [ ] `initModelsPage()` 挂 window

## Task 4：index.html 两处改动

**Files：** `crates/desktop/dist/settings/index.html`

- [ ] `#page-models` 占位 → `<div id="models-list">` 空容器
- [ ] 加 `<script src="models.js"></script>`
- [ ] switchPage('models') 调 initModelsPage()

## Task 5：验证 + 收尾

- [ ] `cargo check --workspace --all-targets` + `cargo clippy -p octopus-desktop`
- [ ] `cargo test -p octopus-desktop model_commands`
- [ ] 同步 architecture.md（模型管理页 / model_commands / models.js）
- [ ] 更新 memory parallel-workstreams
