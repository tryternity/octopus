# Extension Package 格式——实施计划

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 引入 Extension Package（`.octopusext` 文件夹），支持 config.yaml 声明元数据 + 执行体 + skill 预留，ZIP 导入到 `~/.octopus/extensions/` 并创建 DB 菜单项。

**Architecture:** Package 文件夹含 `config.yaml`（YAML 元数据）+ 脚本文件 + 资源。导入时校验 config → 解压到 extensions → 创建 `action_bar_items` DB 记录（`action_data` 存脚本绝对路径）。运行时 script 分支通过 `action_data` 前缀区分内联 vs 文件路径，Package 脚本额外设 `OCTOPUS_PACKAGE_DIR` 环境变量。设置页新增扩展子页（拖拽导入 + 卡片列表 + 删除）。

**Tech Stack:** Rust（serde_yaml + std::fs + zip 解压）、TypeScript / React（ActionBarPanel 扩展子页 + drop zone）

**Spec:** [`docs/superpowers/specs/2026-07-10-extension-package-design.md`](../specs/2026-07-10-extension-package-design.md)

## Global Constraints

- `~/.octopus/extensions/` 下每个含 `config.yaml` 的子文件夹 = 一个 Package
- config.yaml 格式：YAML（`serde_yaml` 已在 infra + desktop Cargo.toml 中）
- `action.script` 必须是相对路径（指向 Package 内文件）
- DB `action_data` 存脚本**绝对路径**（以 `/` 开头区分内联脚本）
- Package 脚本执行时设 `OCTOPUS_PACKAGE_DIR` 环境变量（Package 文件夹绝对路径）
- 同名文件夹覆盖（升级不创建新 DB 记录）
- 扩展子页元信息（version/description/skill）从 config.yaml 实时读取，不存 DB
- skill 块纯声明性——一期仅前端展示 + config 预留，不做 agent 调度
- 浮窗不区分 Package vs 内联——统一走 `list_action_bar_items`

---

## File Structure

| 文件 | 职责 |
|------|------|
| `crates/desktop/src/extensions.rs` | **新建**——Package 加载/校验/导入/删除逻辑 |
| `crates/desktop/src/action_bar_commands.rs` | spawn_script 适配（文件路径 + OCTOPUS_PACKAGE_DIR） |
| `crates/desktop/src/main.rs` | invoke_handler 注册新 command |
| `crates/desktop/frontend/src/pages/Settings/ActionBarPanel.tsx` | 扩展子页 UI |

---

## Task 1: Package 加载与校验——`extensions.rs`

**Files:**
- Create: `crates/desktop/src/extensions.rs`

**Interfaces:**
- Produces: `PackageConfig` struct（反序列化 config.yaml）、`ExtensionInfo` struct、`scan_extensions()` / `load_package_config()` / `validate_package()` 函数

- [x] **Step 1: 创建 extensions.rs + PackageConfig struct**

```rust
// crates/desktop/src/extensions.rs
//! Extension Package 加载、校验、导入。

use serde::Deserialize;
use std::path::{Path, PathBuf};

/// config.yaml 反序列化结构
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PackageConfig {
    pub name: String,
    pub description: String,
    pub version: String,
    pub author: String,
    pub action: PackageAction,
    #[serde(default)]
    pub rules: Option<serde_yaml::Mapping>,
    #[serde(default)]
    pub skill: Option<PackageSkill>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PackageAction {
    #[serde(default = "default_script_type")]
    pub r#type: String,          // "script"（一期仅此值）
    pub script: String,           // 相对路径
    #[serde(default = "default_true")]
    pub is_async: bool,
    #[serde(default)]
    pub write_output_to_clipboard: bool,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PackageSkill {
    pub r#ref: Option<String>,       // 关联已有 skill 目录
    pub file: Option<String>,        // Package 内 SKILL.md
    pub description: Option<String>, // agent 可读描述
}

fn default_script_type() -> String { "script".into() }
fn default_true() -> bool { true }

/// 扩展子页展示信息
#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ExtensionInfo {
    pub dir_name: String,
    pub name: String,
    pub description: String,
    pub version: String,
    pub author: String,
    pub has_skill: bool,
    pub skill_ref: Option<String>,
    pub script_file: String,
    pub script_type_label: Option<String>,  // magic comment（读脚本第一行）
    pub is_async: bool,
    pub db_item_id: Option<i64>,
    pub parent_id: Option<i64>,
}
```

- [x] **Step 2: extensions 目录路径 + scan_extensions()**

```rust
/// ~/.octopus/extensions/
pub fn extensions_dir() -> PathBuf {
    crate::octopus_config_home().join("extensions")
}

/// 扫描 extensions 目录，返回所有合法 Package 的 (dir_name, PackageConfig)
pub fn scan_extensions() -> Vec<(String, PackageConfig)> {
    let dir = extensions_dir();
    if !dir.exists() {
        return Vec::new();
    }
    let mut packages = Vec::new();
    if let Ok(entries) = std::fs::read_dir(&dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                let config_path = path.join("config.yaml");
                if config_path.exists() {
                    if let Ok(config_str) = std::fs::read_to_string(&config_path) {
                        if let Ok(config) = serde_yaml::from_str::<PackageConfig>(&config_str) {
                            packages.push((entry.file_name().to_string_lossy().to_string(), config));
                        }
                    }
                }
            }
        }
    }
    packages
}
```

- [x] **Step 3: validate_package()——校验解压后的临时目录**

```rust
/// 校验 Package 目录结构 + config.yaml 必填字段 + 脚本文件存在
pub fn validate_package(pkg_dir: &Path) -> Result<PackageConfig, String> {
    let config_path = pkg_dir.join("config.yaml");
    if !config_path.exists() {
        return Err("缺少 config.yaml".into());
    }
    let config_str = std::fs::read_to_string(&config_path)
        .map_err(|e| format!("读取 config.yaml 失败: {}", e))?;
    let config: PackageConfig = serde_yaml::from_str(&config_str)
        .map_err(|e| format!("config.yaml 格式错误: {}", e))?;

    if config.name.trim().is_empty() { return Err("name 不能为空".into()); }
    if config.description.trim().is_empty() { return Err("description 不能为空".into()); }
    if config.version.trim().is_empty() { return Err("version 不能为空".into()); }
    if config.author.trim().is_empty() { return Err("author 不能为空".into()); }

    if config.action.r#type != "script" {
        return Err(format!("action.type 仅支持 script（当前: {}）", config.action.r#type));
    }
    let script_path = pkg_dir.join(&config.action.script);
    if !script_path.exists() {
        return Err(format!("脚本文件不存在: {}", config.action.script));
    }
    Ok(config)
}
```

- [x] **Step 4: read_script_magic_comment()——读脚本第一行用于展示**

```rust
/// 读脚本文件第一行 magic comment（如 #python / #shell）
pub fn read_script_magic_comment(pkg_dir: &Path, script_rel: &str) -> Option<String> {
    let script_path = pkg_dir.join(script_rel);
    let content = std::fs::read_to_string(&script_path).ok()?;
    content.lines().next().map(|l| l.trim().to_string())
}
```

- [x] **Step 5: 编译**

Run: `cargo build -p octopus-desktop --features embedded`
Expected: 编译通过（extensions.rs 被引入需在 main.rs 或 lib.rs 加 `mod extensions;`）

- [x] **Step 6: 在 main.rs 注册模块**

在 `crates/desktop/src/main.rs` 的 `mod` 声明区追加：
```rust
mod extensions;
```

- [x] **Step 7: 编译 + Commit**

Run: `cargo build -p octopus-desktop --features embedded`
Expected: 编译通过

```bash
git add crates/desktop/src/extensions.rs crates/desktop/src/main.rs
git commit -m "feat: extensions.rs——Package 加载/校验（config.yaml 反序列化 + scan/validate）"
```

---

## Task 2: ZIP 导入 + DB 记录创建——Tauri command

**Files:**
- Modify: `crates/desktop/src/extensions.rs`（import_extension + delete_extension）
- Modify: `crates/desktop/src/main.rs`（invoke_handler 注册）

**Interfaces:**
- Consumes: Task 1 的 `validate_package` / `extensions_dir` / `scan_extensions`
- Produces: `import_extension()` / `list_extensions()` / `delete_extension()` / `refresh_extensions()` Tauri command

- [x] **Step 1: Cargo.toml 加 zip crate**

在 `crates/desktop/Cargo.toml` `[dependencies]` 中追加：
```toml
zip = "2"
```

Run: `cargo build -p octopus-desktop` 确认 zip crate 编译。

- [x] **Step 2: import_extension()——解压 + 校验 + 安装 + DB 记录**

```rust
use std::fs;
use std::io::Read;

/// 解压 zip 到临时目录 → 校验 → 移动到 extensions → 创建 DB 记录
#[tauri::command]
pub fn import_extension(zip_path: String, parent_id: Option<i64>) -> Result<String, String> {
    // 1. 解压到临时目录
    let tmp_dir = std::env::temp_dir().join(format!("octopus-ext-{}", std::process::id()));
    let _ = fs::remove_dir_all(&tmp_dir);
    fs::create_dir_all(&tmp_dir).map_err(|e| format!("创建临时目录失败: {}", e))?;

    let zip_file = fs::File::open(&zip_path).map_err(|e| format!("打开 zip 失败: {}", e))?;
    let mut archive = zip::ZipArchive::new(zip_file).map_err(|e| format!("读取 zip 失败: {}", e))?;

    for i in 0..archive.len() {
        let mut file = archive.by_index(i).map_err(|e| format!("读取 zip 条目失败: {}", e))?;
        let outpath = match file.enclosed_name() {
            Some(path) => tmp_dir.join(path),
            None => continue,
        };
        if file.is_dir() {
            fs::create_dir_all(&outpath).map_err(|e| format!("创建目录失败: {}", e))?;
        } else {
            if let Some(parent) = outpath.parent() {
                fs::create_dir_all(parent).map_err(|e| format!("创建目录失败: {}", e))?;
            }
            let mut outfile = fs::File::create(&outpath).map_err(|e| format!("创建文件失败: {}", e))?;
            std::io::copy(&mut file, &mut outfile).map_err(|e| format!("写入文件失败: {}", e))?;
        }
    }

    // 2. 找到顶层文件夹（含 config.yaml）
    let pkg_dir = find_package_root(&tmp_dir)
        .ok_or_else(|| "zip 内未找到含 config.yaml 的顶层文件夹".to_string())?;

    // 3. 校验
    let config = validate_package(&pkg_dir)?;

    // 4. 移动到 extensions（覆盖同名）
    let dir_name = pkg_dir.file_name().map(|n| n.to_string_lossy().to_string())
        .ok_or("无法获取文件夹名")?;
    let dest = extensions_dir().join(&dir_name);
    let _ = fs::remove_dir_all(&dest);
    fs::create_dir_all(&extensions_dir()).map_err(|e| format!("创建 extensions 目录失败: {}", e))?;
    copy_dir_recursive(&pkg_dir, &dest).map_err(|e| format!("复制文件失败: {}", e))?;
    let _ = fs::remove_dir_all(&tmp_dir);

    // 5. 创建 DB 记录
    let script_abs = dest.join(&config.action.script);
    let item_id = octopus_infra::db::insert_action_bar_item(
        parent_id,
        &config.name,
        "",
        "script",
        &script_abs.to_string_lossy(),
        config.action.is_async,
        config.action.write_output_to_clipboard,
    ).map_err(|e| e.to_string())?;

    Ok(config.name)
}

/// 在解压目录中找到含 config.yaml 的顶层文件夹
fn find_package_root(base: &Path) -> Option<PathBuf> {
    // 直接是 base/config.yaml
    if base.join("config.yaml").exists() { return Some(base.to_path_buf()); }
    // 或 base/<subdir>/config.yaml
    if let Ok(entries) = fs::read_dir(base) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() && path.join("config.yaml").exists() {
                return Some(path);
            }
        }
    }
    None
}

/// 递归复制目录
fn copy_dir_recursive(src: &Path, dst: &Path) -> std::io::Result<()> {
    if !dst.exists() { fs::create_dir_all(dst)?; }
    for entry in fs::read_dir(src)? {
        let entry = entry?;
        let src_path = entry.path();
        let dst_path = dst.join(entry.file_name());
        if src_path.is_dir() {
            copy_dir_recursive(&src_path, &dst_path)?;
        } else {
            fs::copy(&src_path, &dst_path)?;
        }
    }
    Ok(())
}
```

- [x] **Step 3: list_extensions()——返回扩展列表 + DB 关联**

```rust
#[tauri::command]
pub fn list_extensions() -> Result<Vec<ExtensionInfo>, String> {
    let packages = scan_extensions();
    let dir = extensions_dir();

    // 从 DB 查所有 action_data 以 extensions 路径开头的记录
    let db_items = octopus_infra::db::list_all_action_bar_items().map_err(|e| e.to_string())?;
    let db_map: std::collections::HashMap<String, (i64, Option<i64>)> = db_items.iter()
        .filter(|i| i.action_data.starts_with(dir.to_string_lossy().as_ref()))
        .map(|i| (i.action_data.clone(), (i.id, i.parent_id)))
        .collect();

    let mut list = Vec::new();
    for (dir_name, config) in packages {
        let pkg_path = dir.join(&dir_name);
        let script_abs = pkg_path.join(&config.action.script);
        let script_key = script_abs.to_string_lossy().to_string();
        let (db_item_id, parent_id) = db_map.get(&script_key)
            .map(|(id, pid)| (Some(*id), *pid))
            .unwrap_or((None, None));

        list.push(ExtensionInfo {
            dir_name,
            name: config.name,
            description: config.description,
            version: config.version,
            author: config.author,
            has_skill: config.skill.is_some(),
            skill_ref: config.skill.as_ref()
                .and_then(|s| s.r#ref.clone().or_else(|| s.file.clone())),
            script_file: config.action.script,
            script_type_label: read_script_magic_comment(&pkg_path, &config.action.script),
            is_async: config.action.is_async,
            db_item_id,
            parent_id,
        });
    }
    Ok(list)
}
```

- [x] **Step 4: delete_extension() + refresh_extensions()**

```rust
#[tauri::command]
pub fn delete_extension(dir_name: String) -> Result<(), String> {
    let dir = extensions_dir().join(&dir_name);
    if !dir.exists() {
        return Err("扩展包不存在".into());
    }
    // 删 DB 记录（action_data 匹配 extensions 路径）
    let db_items = octopus_infra::db::list_all_action_bar_items().map_err(|e| e.to_string())?;
    for item in db_items {
        if item.action_data.contains(&dir_name) && item.action_data.starts_with(extensions_dir().to_string_lossy().as_ref()) {
            // 用底层 delete（绕过 is_system 检查，扩展导入时 is_system=0）
            let _ = octopus_infra::db::delete_action_bar_item(item.id);
        }
    }
    // 删文件夹
    fs::remove_dir_all(&dir).map_err(|e| format!("删除文件夹失败: {}", e))?;
    Ok(())
}

#[tauri::command]
pub fn refresh_extensions() -> Result<(), String> {
    // scan_extensions 已实时扫描，此处仅确保目录存在
    fs::create_dir_all(extensions_dir()).map_err(|e| format!("创建目录失败: {}", e))?;
    Ok(())
}
```

- [x] **Step 5: main.rs 注册 4 个 command**

在 `crates/desktop/src/main.rs` invoke_handler 追加：
```rust
            extensions::import_extension,
            extensions::list_extensions,
            extensions::delete_extension,
            extensions::refresh_extensions,
```

- [x] **Step 6: 编译 + Commit**

Run: `cargo build -p octopus-desktop --features embedded`
Expected: 编译通过

```bash
git add crates/desktop/Cargo.toml crates/desktop/src/extensions.rs crates/desktop/src/main.rs
git commit -m "feat: ZIP 导入 + list/delete/refresh extension command"
```

---

## Task 3: spawn_script 适配——文件路径 + OCTOPUS_PACKAGE_DIR

**Files:**
- Modify: `crates/desktop/src/action_bar_commands.rs`

**Interfaces:**
- Consumes: 无新接口
- Produces: `load_script_source()` 辅助函数 + script 分支适配

- [x] **Step 1: load_script_source()——区分内联 vs 文件路径**

在 `action_bar_commands.rs` 的 `spawn_script` 之前加：

```rust
/// 加载脚本源码：绝对路径 → 读文件；否则 → 内联（action_data 本身）
fn load_script_source(action_data: &str) -> String {
    if action_data.starts_with('/') {
        std::fs::read_to_string(action_data).unwrap_or_default()
    } else {
        action_data.to_string()
    }
}
```

- [x] **Step 2: script 分支适配**

将 `execute_action_bar_inner` 的 `"script"` 分支中的 `source` 变量改为 `load_script_source`：

```rust
        "script" => {
            let is_async = item.is_async;
            let write_output = item.write_output_to_clipboard;
            let item_title = item.title.clone();
            let item_id = item.id;

            // Package 脚本（action_data 是文件路径）vs 内联脚本
            let source = load_script_source(&item.action_data);
            let pkg_dir = if item.action_data.starts_with('/') {
                std::path::Path::new(&item.action_data).parent()
                    .map(|p| p.to_string_lossy().to_string())
            } else { None };

            if is_async {
                run_script_async(&source, &text, item_id, pkg_dir)?;
                Ok(false)
            } else {
                let text_clone = text.clone();
                let result = tokio::task::spawn_blocking(move || {
                    run_script_sync_blocking(&source, &text_clone, item_id, pkg_dir)
                }).await.map_err(|e| format!("脚本执行线程异常: {}", e))??;
                // ... 后续 exit_code/stdout 处理不变 ...
            }
        }
```

- [x] **Step 3: run_script_async / run_script_sync_blocking 加 pkg_dir 参数**

两个函数签名加 `pkg_dir: Option<String>` 参数，传给 `spawn_script`：

```rust
fn run_script_async(source: &str, text: &str, item_id: i64, pkg_dir: Option<String>) -> Result<(), String> {
    let (child, script_type) = spawn_script(source, text, false, &pkg_dir)?;
    // ... 不变 ...
}

fn run_script_sync_blocking(source: &str, text: &str, item_id: i64, pkg_dir: Option<String>) -> Result<ScriptResult, String> {
    let (child, script_type) = spawn_script(source, text, true, &pkg_dir)?;
    // ... 不变 ...
}
```

- [x] **Step 4: spawn_script 加 pkg_dir 参数 + OCTOPUS_PACKAGE_DIR**

签名改为：
```rust
fn spawn_script(source: &str, text: &str, capture_output: bool, pkg_dir: &Option<String>) -> Result<(std::process::Child, String), String> {
```

在 `cmd.env("OCTOPUS_TEXT", text);` 之后追加：
```rust
    if let Some(dir) = pkg_dir {
        cmd.env("OCTOPUS_PACKAGE_DIR", dir);
    }
```

- [x] **Step 5: 编译 + 测试 + Commit**

Run: `cargo build -p octopus-desktop --features embedded && cargo test -p octopus-desktop`
Expected: 编译通过，104 测试全过

```bash
git add crates/desktop/src/action_bar_commands.rs
git commit -m "feat: spawn_script 适配 Package 脚本（文件路径 + OCTOPUS_PACKAGE_DIR）"
```

---

## Task 4: 前端——扩展子页 UI

**Files:**
- Modify: `crates/desktop/frontend/src/pages/Settings/ActionBarPanel.tsx`

**Interfaces:**
- Consumes: Task 2 的 `list_extensions` / `import_extension` / `delete_extension` / `refresh_extensions`

> **强制**：涉及前端 UI 修改，动手前先 `view` frontend-design skill SKILL.md 做设计规划。

- [x] **Step 1: 加载 frontend-design skill**

View: `/Users/wudarui/.claude/skills/frontend-design/SKILL.md`

- [x] **Step 2: ExtensionInfo 前端接口 + 扩展子页组件**

在 ActionBarPanel.tsx 中新增：

```typescript
interface ExtensionInfo {
  dirName: string;
  name: string;
  description: string;
  version: string;
  author: string;
  hasSkill: boolean;
  skillRef: string | null;
  scriptFile: string;
  scriptTypeLabel: string | null;
  isAsync: boolean;
  dbItemId: number | null;
  parentId: number | null;
}

const ExtensionsPanel = ({ showToast }: { showToast: (msg: string) => void }) => {
  const [extensions, setExtensions] = useState<ExtensionInfo[]>([]);
  const [loaded, setLoaded] = useState(false);
  const [dragging, setDragging] = useState(false);

  const refresh = useCallback(async () => {
    try {
      const list = await invoke<ExtensionInfo[]>("list_extensions");
      setExtensions(list);
    } catch (e) { showToast("加载失败：" + e); }
    setLoaded(true);
  }, [showToast]);

  useEffect(() => { refresh(); }, [refresh]);

  // 拖拽导入
  const handleDrop = useCallback(async (e: React.DragEvent) => {
    e.preventDefault();
    setDragging(false);
    const files = Array.from(e.dataTransfer.files);
    const zipFile = files.find(f => f.name.endsWith(".octopusext.zip"));
    if (!zipFile) { showToast("请拖入 .octopusext.zip 文件"); return; }
    try {
      // Tauri drag-drop 需走前端 FS API 获取路径
      // 实际实现中用 @tauri-apps/api/webview 的 onDragDropEvent 或 Tauri file-drop 事件
      const name = await invoke<string>("import_extension", { zipPath: zipFile.path, parentId: null });
      showToast(`已导入：${name}`);
      refresh();
    } catch (e) { showToast("导入失败：" + e); }
  }, [showToast, refresh]);

  const handleDelete = useCallback(async (dirName: string) => {
    try {
      await invoke("delete_extension", { dirName });
      showToast("已删除");
      refresh();
    } catch (e) { showToast("删除失败：" + e); }
  }, [showToast, refresh]);

  if (!loaded) return <p className="py-12 text-center text-sm text-muted-foreground">加载中…</p>;

  return (
    <div
      onDragOver={(e) => { e.preventDefault(); setDragging(true); }}
      onDragLeave={() => setDragging(false)}
      onDrop={handleDrop}
      className={cn("rounded-lg border border-dashed transition-colors",
        dragging ? "border-voice bg-voice/5" : "border-border")}
    >
      {/* ... 卡片列表 / 空状态 / 删除 ... */}
    </div>
  );
};
```

- [x] **Step 3: header 切换 + 子页路由**

在 header 按钮区加「扩展」按钮（与「执行记录」同模式），view state 加 `"extensions"` 值：

```typescript
const [view, setView] = useState<"menu" | "runs" | "extensions">("menu");
```

Body 区域：
```tsx
{view === "extensions" ? <ExtensionsPanel showToast={showToast} /> : view === "runs" ? <ScriptRunsList ... /> : !loaded ? ... : ...}
```

- [x] **Step 4: 扩展卡片 UI**

每个卡片展示：名称 + 版本 + SKILL 徽章 + 描述 + 脚本信息 + 挂载位置 + 删除按钮。遵循 frontend-design skill 指导。

- [x] **Step 5: tsc + 构建 + Commit**

Run: `cd crates/desktop/frontend && npx tsc --noEmit && npm run build`
Expected: 无错误

```bash
git add crates/desktop/frontend/src/pages/Settings/ActionBarPanel.tsx
git commit -m "feat(ui): 扩展子页——拖拽导入 + 卡片列表 + 删除"
```

---

## Task 5: 启动扫描 + 文档同步

**Files:**
- Modify: `crates/desktop/src/main.rs`（setup 中扫描 extensions 目录）
- Modify: `docs/architecture.md`
- Modify: `docs/superpowers/specs/2026-07-09-action-bar-menu-db-design.md`

- [x] **Step 1: 启动时确保 extensions 目录存在**

在 `main.rs` 的 `setup` 闭包中追加：
```rust
    // 确保 extensions 目录存在
    let ext_dir = crate::extensions::extensions_dir();
    if !ext_dir.exists() {
        let _ = std::fs::create_dir_all(&ext_dir);
    }
```

- [x] **Step 2: architecture.md 更新**

action bar 第 9 点中追加 Extension Package 描述：config.yaml 声明元数据 + 执行体 + skill 预留；ZIP 导入到 ~/.octopus/extensions/；导入 = 创建 DB action_bar_items 记录；spawn_script 通过 action_data 前缀区分内联 vs 文件路径；Package 脚本额外设 OCTOPUS_PACKAGE_DIR。

- [x] **Step 3: spec 交叉引用更新**

`2026-07-09-action-bar-menu-db-design.md` 中 script 相关段落补 Extension Package 交叉引用。

- [x] **Step 4: 编译 + 测试 + Commit**

Run: `cargo build -p octopus-desktop --features embedded && cargo test -p octopus-desktop`
Expected: 全部通过

```bash
git add crates/desktop/src/main.rs docs/
git commit -m "feat: 启动扫描 extensions + 文档同步"
```

---

## Self-Review

**1. Spec coverage:**
- §2 目录结构 → Task 1 Step 2 (scan_extensions) ✅
- §3 config.yaml 字段 → Task 1 Step 1 (PackageConfig) ✅
- §4 DB 集成 → Task 2 Step 2 (import_extension INSERT) ✅
- §5 加载时机 → Task 5 Step 1 (启动扫描) ✅
- §6 ZIP 导入 → Task 2 Step 2 (import_extension) ✅
- §7 设置页 UI → Task 4 ✅
- §8 spawn_script 适配 → Task 3 ✅
- §9 不变量 → Global Constraints 逐条覆盖 ✅

**2. Placeholder scan:** 无 TBD/TODO ✅

**3. Type consistency:**
- `PackageConfig` / `PackageAction` / `PackageSkill` — Task 1 定义，Task 2 消费 ✅
- `ExtensionInfo` — Task 1 定义，Task 2 list_extensions 产出，Task 4 前端消费 ✅
- `spawn_script(source, text, capture_output, pkg_dir)` — Task 3 定义新签名 ✅
- `run_script_async` / `run_script_sync_blocking` 加 `pkg_dir: Option<String>` — Task 3 Step 3 ✅
