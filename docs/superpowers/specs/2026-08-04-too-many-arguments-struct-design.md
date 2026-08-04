# too_many_arguments struct 治理设计

- 日期：2026-08-04
- 分支：`refactor/too-many-arguments`
- Worktree：`.worktrees/refactor/too-many-arguments`
- 类型：重构（clippy 治理，签名重构，零行为变更）
- Baseline：`cargo clippy --workspace --all-targets | grep -c "too many arguments"` = **8 unique 函数**

---

## 1. 背景与动机

workspace 有 8 个函数触发 `clippy::too_many_arguments`（参数 ≥8），全是 DB CRUD 或 session 启动函数，参数平铺 8-14 个：

```rust
// 13 参数——位置传参，新增字段破坏顺序，调用点难以读懂
pub fn insert_action_bar_item(
    parent_id: Option<i64>, title: &str, icon: &str, action_type: &str,
    action_data: &str, is_async: bool, write_output_to_clipboard: bool,
    agent: &str, accepts: &str, trigger_keyword: &str, is_enabled: bool,
    need_voice: bool, app_bundle_ids: &str,
) -> Result<i64>
```

痛点：
- **位置传参易错**——13 个参数调用点要数位置对应
- **新增字段破坏调用点**——加一个字段所有调用点都要改
- **可读性差**——`fn(true, false, true, ...)` 看不懂

### 1.1 目标

引入 Input struct（参数包）替代平铺参数。**保持 infra 的"struct = 数据，fn = 操作"惯例**——struct 仅承载参数，不挂方法（非 Active Record）。

### 1.2 非目标

- ❌ 不改 desktop Tauri 命令（4 个参数多的命令，Tauri 命令惯例平铺；且 desktop 无 `#![warn(clippy::all)]`，本就不报警告）
- ❌ 不改其他 clippy warning（本次仅 too_many_arguments）
- ❌ 不动 DB schema / 业务逻辑 / 测试数据
- ❌ 不引入 Active Record 模式（保持 infra 的"自由函数 + DTO struct"风格）

---

## 2. 范围：8 个函数 / 4 组

| 组 | crate | 函数 | 当前参数数 | 设计 |
|---|---|---|---|---|
| **A** | infra | insert_action_bar_item | 13 | ActionBarItemInput |
| A | infra | insert_action_bar_item_at | 14（含 conn） | ActionBarItemInput |
| A | infra | update_action_bar_item | 12 | ActionBarItemUpdate |
| A | infra | update_action_bar_item_at | 13（含 conn） | ActionBarItemUpdate |
| **B** | infra | insert_cloud_model | 8 | CloudModelInput |
| B | infra | update_cloud_model | 8（含 id） | CloudModelUpdate |
| **E** | infra | insert_script_run | 7（边缘） | ScriptRunRecord |
| **C** | asr-cloud | run_baidu_session | 9（含 channels） | BaiduSessionConfig |

注：clippy 默认阈值 7 参数，E 组 7 参数也报（边缘）。

---

## 3. struct 设计（字段组合模式）

### A 组：ActionBar（crates/infra/src/db/action_bar.rs）

```rust
/// action_bar item insert/update 公共字段。
/// 不含 id（自增）/ parent_id（仅 insert）/ sort_order（DB 默认）/
/// is_system（代码设）——这些不该由调用方传。
#[derive(Debug, Clone)]
pub struct ActionBarItemFields<'a> {
    pub title: &'a str,
    pub icon: &'a str,
    pub action_type: &'a str,
    pub action_data: &'a str,
    pub is_async: bool,
    pub write_output_to_clipboard: bool,
    pub agent: &'a str,
    pub accepts: &'a str,
    pub trigger_keyword: &'a str,
    pub is_enabled: bool,
    pub need_voice: bool,
    pub app_bundle_ids: &'a str,
}

/// insert_action_bar_item 输入——公共字段 + parent_id。
pub struct ActionBarItemInput<'a> {
    pub parent_id: Option<i64>,
    pub fields: ActionBarItemFields<'a>,
}

/// update_action_bar_item 输入——公共字段 + id（无 parent_id）。
pub struct ActionBarItemUpdate<'a> {
    pub id: i64,
    pub fields: ActionBarItemFields<'a>,
}
```

### B 组：CloudModel（crates/infra/src/db/models.rs）

```rust
/// cloud model insert/update 公共字段。
#[derive(Debug, Clone)]
pub struct CloudModelFields<'a> {
    pub provider: &'a str,
    pub category: &'a str,
    pub model_name: &'a str,
    pub source: &'a str,
    pub secret_key: &'a str,
    pub is_streaming: bool,
    pub is_thinking: bool,
}

/// insert_cloud_model 输入——公共字段 + domain。
pub struct CloudModelInput<'a> {
    pub domain: &'a str,
    pub fields: CloudModelFields<'a>,
}

/// update_cloud_model 输入——公共字段 + id。
pub struct CloudModelUpdate<'a> {
    pub id: i64,
    pub fields: CloudModelFields<'a>,
}
```

### E 组：ScriptRun（crates/infra/src/db/action_bar.rs）

```rust
/// insert_script_run 输入——脚本运行记录。
pub struct ScriptRunRecord<'a> {
    pub item_id: i64,
    pub script_type: &'a str,
    pub exit_code: Option<i32>,
    pub stdout: &'a str,
    pub stderr: &'a str,
    pub error_msg: &'a str,
}
```

### C 组：BaiduSession（crates/asr-cloud/src/baidu_stream.rs）

```rust
/// run_baidu_session 配置——owned String（async fn 跨 await 持有，不能用 &'a）。
pub struct BaiduSessionConfig {
    pub endpoint: String,
    pub appid: String,
    pub appkey: String,
    pub dev_pid: String,
    pub language: String,
    pub pre_roll_samples: Vec<f32>,
}
```

注意：`pcm_rx` / `result_tx`（channel）不进 config——它们是运行时 I/O，不是配置。

---

## 4. 函数签名改造

| 函数 | 改前 | 改后 |
|---|---|---|
| insert_action_bar_item | 13 参数 | `(input: &ActionBarItemInput)` |
| insert_action_bar_item_at | `(conn, 13 参数)` | `(conn, input: &ActionBarItemInput)` |
| update_action_bar_item | 12 参数 | `(update: &ActionBarItemUpdate)` |
| update_action_bar_item_at | `(conn, 12 参数)` | `(conn, update: &ActionBarItemUpdate)` |
| insert_cloud_model | 8 参数 | `(input: &CloudModelInput)` |
| update_cloud_model | 8 参数 | `(update: &CloudModelUpdate)` |
| insert_script_run | 7 参数 | `(record: &ScriptRunRecord)` |
| run_baidu_session | `(pcm_rx, result_tx, 7 参数)` | `(pcm_rx, result_tx, config: BaiduSessionConfig)` |

---

## 5. 迁移步骤（4 步）

每步独立 commit + 该 crate `cargo test` 全过。

| 步骤 | 内容 | 风险 |
|---|---|---|
| **1. B 组：CloudModel** | 新 3 struct + 改 2 函数 + 改调用点 | 低 |
| **2. E 组：ScriptRun** | 新 1 struct + 改 1 函数 + 改调用点 | 低 |
| **3. C 组：BaiduSession** | 新 1 struct（owned）+ 改 1 函数 + 改调用点 | 低 |
| **4. A 组：ActionBar** | 新 3 struct + 改 4 函数 + 改 desktop 调用点（跨 crate） | 中 |

A 组最后做——字段最多、跨 crate、改动面最大。前 3 组建立模式。

---

## 6. 不变量（必须保持）

1. **零行为变更**——各 crate 测试数不减
2. **`cargo build --workspace`**：0 error 0 warning
3. **`cargo clippy --workspace --all-targets 2>&1 \| grep -c "too many arguments"`**：从 8 → 0
4. **DB 操作语义不变**——SQL 语句一字不改，仅参数传递形状变
5. **公开 API 兼容性**：8 个函数都是 infra/asr-cloud 的 pub/pub(crate) 函数，签名变更会影响 desktop——但 desktop 是唯一消费者，编译失败即发现

---

## 7. 风险与缓解

| 风险 | 概率 | 影响 | 缓解 |
|---|---|---|---|
| 生命周期编译失败 | 中 | 编译失败 | infra 全同步函数，`&'a str` 安全；C 组用 owned String |
| 调用点遗漏 | 中 | 编译失败（参数数不匹配） | 编译器即时报错；grep 二次防护 |
| 字段语义错位 | 低 | 行为变更 | insert/update 字段集分别核对 |
| desktop 跨 crate 调用断裂 | 中 | 编译失败 | desktop 调 infra pub fn，签名改后编译器立刻报 |
| C 组 owned String 性能退化 | 极低 | 微秒级 clone | 1 个调用点，clone 6 个 String 可忽略 |

---

## 8. 成功标准

1. `cargo build --workspace`：0 error 0 warning
2. `cargo test --workspace`：全过、各 crate 测试数不减
3. `cargo clippy --workspace --all-targets 2>&1 | grep -c "too many arguments"`：**0**
4. 8 个新 struct 定义（A3 + B3 + E1 + C1）
5. 8 个函数签名改为 struct 参数

---

## 9. 文档同步

无需改 architecture.md（struct 是实现细节）。本 spec 是唯一文档。

## 10. 关联

- clippy cleanup 分支 `cleanup/clippy-workspace`（已 merge main，含本次前置的 clippy 修复）
- AGENTS.md「改动验证纪律」要求：改 struct 后 grep 所有消费点 + 端到端调用链验证
