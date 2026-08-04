# AGENTS.md — octopus 代码库指南

## 项目概述

octopus 是一个基于 ONNX Runtime 的语音识别（ASR）工具集，使用 Rust 编写。提供 CLI、HTTP Server、Tauri 桌面应用三种使用方式，并集成了 LLM 文本润色能力。

**架构、功能、技术细节以 [`docs/`](docs/) 下文档为唯一真相源**——本文件只描述项目结构和开发流程，避免重复维护导致文档滞后。

## 关键命令

### 构建

⚠️ **Cargo profile 三层结构**（详见 `docs/architecture.md`）：
- `--release`：默认 release，**不带 LTO/strip**（链接快，开发期迭代用）
- `--profile optimize`：在 release 上叠 LTO/strip/codegen-units=1，**生产构建必用**
- `--profile profiling`：带 DWARF 符号，samply/xctrace 性能分析用

```bash
# 构建全部（含 library）—— 生产构建用 optimize
cargo build --profile optimize

# 仅构建 server + cli（最常用）—— 开发期可省 LTO 用 release
cargo build --release -p octopus-server -p octopus-cli

# 仅构建 library
cargo build --release -p octopus-asr-local

# 构建桌面应用（embedded 模式，默认）—— 生产构建
# ⚠️ 必须 --features custom-protocol，否则 tauri 走 devUrl=http://localhost:1420
# （cfg(dev) = !has_feature("custom-protocol")，跟 release/debug profile 无关）
cargo run --profile optimize -p octopus-desktop --features embedded,custom-protocol

# 构建桌面应用（含云端 ASR：阿里云/字节跳动/腾讯）
cargo run --profile optimize -p octopus-desktop --features embedded,cloud,custom-protocol

# 构建桌面应用（WebSocket 远程模式）
cargo run --profile optimize -p octopus-desktop --features remote-ws,custom-protocol

# 构建桌面应用（gRPC 远程模式）
cargo run --profile optimize -p octopus-desktop --features remote-grpc,custom-protocol
```

### 打包（macOS DMG）

```bash
# 生产级 DMG（--profile optimize，LTO+strip，体积小，链接慢）
./scripts/build-macos-dmg.sh

# 调试打包流程（纯 release，无 LTO，链接快）
./scripts/build-macos-dmg.sh --no-lto

# 构建完 open .app 冒烟测试
./scripts/build-macos-dmg.sh --open
```

产物：`target/<profile>/bundle/dmg/octopus_<version>_<arch>.dmg`（未签名，自用/内测）+ `target/<profile>/bundle/macos/octopus.app`。

feature 组合固定 `embedded,cloud,vault,custom-protocol`（`custom-protocol` 生产 build 必须启用）。详见 [`docs/architecture.md` §打包/分发](docs/architecture.md) + [plan](docs/superpowers/plans/archived/2026-07-23-macos-dmg-packaging.md)。

### 开发运行

```bash
# CLI 查看模型配置
cargo run -p octopus-cli -- config

# CLI 麦克风实时识别
cargo run -p octopus-cli -- e2e --model sensevoice

# CLI 文件识别
cargo run -p octopus-cli -- transcribe <wav_path> --model whisper --language zh

# CLI 流式测试
cargo run -p octopus-cli -- stream-test test.wav --model zipformer-ctc

# Server 启动（默认 3000 端口）
cargo run -p octopus-server

# 桌面应用（推荐用脚本，会清 WebView 缓存）
./run-octopus.sh

# LLM 润色测试
cargo run --release --package octopus-llm --example test_polish
```

### 测试

```bash
# 运行全部测试（内联 unit tests）
cargo test

# 单个 crate 测试
cargo test -p octopus-asr-local
cargo test -p octopus-infra
cargo test -p octopus-desktop
```

注意：没有独立的 `tests/` 目录，所有测试都是 `#[cfg(test)] mod tests {}` 内联在源文件中。

#### 测试分层（三层）

测试按耗时 + 依赖分为三层，用 `#[ignore]` 标记区分：

| 层 | 命令 | 耗时 | 跑什么 | 何时跑 |
|---|---|---|---|---|
| **核心** | `cargo test` | ~15s | 纯逻辑测试（含 vault 弱 KDF 测试） | **每次合并必跑** |
| **慢** | `cargo test -- --ignored` | ~2min | + 参数边界 / API key / 文件系统 / 模型缓存 | 重构 / 功能增减后全面回归 |
| **real-model** | `cargo test -p octopus-asr-local -- --ignored` | ~5min | + 真实 ASR 模型推理（需 HuggingFace 缓存） | 手动验证模型识别质量 |

**标记约定**：
- `#[ignore = "real-model: ..."]` — 需真实模型文件/DB 引擎，默认跳过
- `#[ignore = "slow: ..."]` 或裸 `#[ignore]` — 需特殊环境（API key / 音频文件 / 网络），默认跳过
- 无标记 — 核心测试，`cargo test` 默认跑

**vault 弱 KDF**：vault 测试在 `#[cfg(test)]` 下用 `Argon2Params::test_params()`（memory_kib=8, iterations=1）替代生产参数（64MB, 3 iterations），argon2 派生从 ~0.4s/次 → <1ms/次。生产代码（`setup_vault` 非 test 分支）仍用 `Default`。

**asr-local real_model 测试**：9 个需真实模型的测试统一 `#[ignore = "real-model"]`，`--ignored` 跑时保留 skip-by-return 作为二次防护（无模型优雅跳过，不 panic）。

## Cargo Workspace 结构

```
crates/
├── infra/        # octopus-infra — 基础设施层，无项目内依赖（DB / config / consts / paths / cpu）
├── onnx-infra/   # onnx-infra — ONNX Runtime 封装（ort session 管理 / 维度推断）
├── sync/         # octopus-sync — 通用 git 同步基础设施（git wrapper / outline / error / privacy / store 工具 / hotword 模块）
├── scheduler/    # octopus-scheduler — 通用后台调度器（定时任务 + CPU 空闲检测，依赖 infra）
├── asr/          # octopus-asr-local — 核心推理库（Whisper/SenseVoice/Paraformer/Zipformer + Silero VAD）
├── asr-cloud/    # octopus-asr-cloud — 云端 ASR 协议层（Aliyun/ByteDance/Tencent/Baidu WS 流式 + 批引擎）
├── clipboard/    # octopus-clipboard — 剪贴板历史管理（监听 / 存储 / FTS5 / image_data / 软删回收站）
├── ocr/          # octopus-ocr — OCR 图片识别（统一接口，分发到 paddle-ocr）
├── paddle-ocr/   # octopus-paddle-ocr — PaddleOCR ONNX 推理（PP-OCRv6 检测+识别）
├── capx/         # octopus-capx — 屏幕截图（xcap 封装 / 滚动截图 / 区域选区）
├── translation/  # octopus-translation — 翻译引擎（本地 OPUS-MT + 云端 API）
├── search/       # octopus-search — 独立搜索引擎（应用索引 / 快捷指令 / 历史搜索）
├── llm/          # octopus-llm — LLM 润色客户端
├── vault/        # octopus-vault — 密码保险库纯逻辑库（加密 / Auto-Type / TOTP / vault git 同步）
├── download/     # octopus-download — 通用下载器（分块并发 + 断点续传 + 校验 + 镜像）
├── cli/          # octopus-cli — 命令行工具
├── server/       # octopus-server — HTTP/WebSocket 服务
├── desktop/      # octopus-desktop — Tauri 2 桌面应用（聚合所有上层 crate）
└── dlp/          # octopus-dlp — 视频音频下载转码 sidecar（yt-dlp 封装）
```

### 依赖关系

```
infra ← (所有 crate)  — infra 是唯一无项目内依赖的 crate
onnx-infra ← (asr)  — ASR 引擎依赖 ONNX Runtime 封装
sync ← (vault, desktop)  — vault 复用 sync 通用代码；desktop 热词命令算 md5
scheduler ← (desktop)  — desktop setup 创建 Scheduler 实例 + 注册定时任务
asr ← (cli, server, desktop via "embedded" feature)
asr-cloud ← (desktop via "cloud" feature)
clipboard ← (desktop)
ocr ← (desktop) → paddle-ocr  — ocr 分发到 paddle-ocr 实现
capx ← (desktop)
translation ← (desktop)
search ← (desktop)
llm ← (asr via dev-dep, desktop)
vault ← (desktop via "vault" feature)
download ← (desktop)
desktop → feature-gated: embedded (=asr) | remote-ws | remote-grpc | cloud (云端 ASR WS 流式：Aliyun/ByteDance/Tencent/Baidu)
desktop → feature-gated: vault (=vault + keychain + TOTP)
```

**infra 是唯一无项目内依赖的 crate**，任何跨 crate 共享的内容应放在 infra。

## 技能（Skills）路径

自定义 skill 实际存放在 `~/.agents/skills/`（项目内置的 `<available_skills>` 列表中的 `~/.claude/skills/` 路径是旧的，部分 skill 如 `z-sync-superpowers`、`z-mermaid`、`z-module` 等只在 `~/.agents/skills/` 下）。

**找不到 skill 时，先到 `~/.agents/skills/` 下查找**，而非默认搜索或放弃。

常用路径：
- `~/.agents/skills/z_sync_superpowers/SKILL.md` — 代码变更后同步 superpowers specs/plans 文档
- `~/.agents/skills/z_mermaid/SKILL.md` — Mermaid 图表
- `~/.agents/skills/z_module/SKILL.md` — 模块化
- `~/.agents/skills/superpowers/` — superpowers 技能集（brainstorming、writing-plans 等）

## 开发流程（文档驱动）

**一切以文档为基础继续开发，保持文档与代码同步。** 架构、功能、技术细节以 [`docs/`](docs/) 下文档为唯一真相源——遇到「代码怎么实现」「架构怎么组织」「流程怎么走」的问题，**先查文档，不猜代码**；文档没覆盖或描述过时，先修文档再改代码。

### 大需求（新功能 / 架构调整 / 接口变化）

必须完整经过 superpowers 工作流，**不得跳步**：

1. **brainstorming** — 用 `superpowers:brainstorming` skill 充分探讨需求、用户意图、设计取舍，确认方向后再动手
2. **写 spec** — 在 `docs/superpowers/specs/YYYY-MM-DD-<feature>-design.md` 记录设计（功能、架构、接口、不变量、降级路径）
3. **写执行计划** — 在 `docs/superpowers/plans/YYYY-MM-DD-<feature>.md` 分解任务（每任务含文件、变更点、验证命令）
4. **实现代码** — 按计划逐任务实现，每任务后跑验证命令
5. **review plan（强制）** — 实现完成后必须回看 plan，把实际偏差、新增决策、删除/合并的子任务回写到 plan。**plan 是「实施记录」而非「一次性待办」**，最终必须反映实际实现

### 小需求（bug fix / 参数调整 / 文案修改）

实现前 **review 相关 spec 和 plan**：
- 找到对应 spec / plan，检查设计描述是否仍然成立
- 及时修改文档中过时的地方（参数变了、阈值变了、流程变了）
- 没有对应 spec 的小改动，至少更新 `docs/architecture.md` 相关章节

### 文档与测试准则（强制）

**文档先行**：稍微大点的功能特性（新功能 / 行为变更 / 新组件），动手写代码前必须先补充或新增 spec（`docs/superpowers/specs/`），可选 plan（`docs/superpowers/plans/`）。纯 bug fix 可省略 spec，但仍需更新 `docs/architecture.md` 相关章节。

**TDD 优先**：尽量测试先行（test-driven development）。无法先写测试的场景（如 UI 交互、AX 集成），事后补录测试也可以——重点是防止回归问题。纯 bug fix 至少补一个覆盖该 bug 的回归测试。

**判断标准**：
| 改动类型 | spec/plan | 测试 |
|---|---|---|
| 新功能 / 行为变更 / 新组件 | ✅ 必须先写 | ✅ TDD 或事后补 |
| bug fix | ❌ 可省（更新 architecture.md） | ✅ 至少回归测试 |
| 参数调整 / 文案修改 | ❌ 可省 | 视情况 |

### 前端页面设计（强制）

涉及任何页面、组件、弹窗、浮层等前端 UI 的修改或新建，**必须使用 `frontend-design` skill 进行设计**：

- 动手写代码前，先 `view` 该 skill 的 SKILL.md，按其指导原则（色彩、字体、布局、签名元素）做设计规划
- 不是简单套用 shadcn/ui 默认样式，而是做出有意图、有辨识度的设计选择
- 纯功能性改动（如改个文案、修个逻辑 bug）无需触发此流程，但涉及视觉表现的改动必须遵守

### 序列化 casing 规范（强制）

**Tauri 边界**（命令返回值 + 事件 payload + 前端 TS interface）统一 **camelCase**。**协议层 / 外部格式** 保持各自 casing 不动。

| 层级 | casing | 规则 | 例子 |
|---|---|---|---|
| **命令返回值 DTO** | camelCase | Rust 字段 snake_case + `#[serde(rename_all = "camelCase")]`；前端 interface camelCase | `newId` / `filePath` / `isAvailable` |
| **事件 payload（Tauri emit）** | enum 外层 kebab-case tag + 变体字段 camelCase | `#[serde(tag = "event", rename_all = "kebab-case")]` 外层 + `#[serde(rename_all = "camelCase")]` 每个变体 | `{"event":"merge-done","id":1,"newId":2}` |
| **前端 TS interface** | camelCase | 与后端 DTO 一致，零转换 | `interface MergeResult { newId: number }` |
| **❌ 协议层（不动）** | snake_case + kebab tag | Swift helper 子进程协议，硬编码字段名 | `HelperEvent` / `RecordingRequest`（`crates/record/src/protocol.rs`） |
| **❌ 外部格式对齐（不动）** | 各异 | 对齐外部 JSON 格式 | `ThemeColors`（Tailwind kebab）/ Bitwarden importer（camelCase）/ paddle-ocr config（snake_case） |
| **✅ DB 直映射（返回前端的）** | camelCase | 2026-07-28 全工程 casing 统一完成。返回前端的 DB struct 加 `rename_all = "camelCase"`（`TranscriptionRecord` / `ClipboardItem` / `MetaInfo` / `RecordingMeta` / `ActionBarItem` 等）；纯内部 DB 映射不返回前端的（`ModelRow` / `LocalAsrModelRow` 用 rusqlite row.get 不经 serde）不需加 | — |
| **✅ vault sync 持久化** | camelCase | 2026-07-28 改为 camelCase（按全新库策略，老 sync 数据需清掉重建）。`MetaFile` / `CipherFile` / `CipherEncStrings` / `CipherPlaintextMeta` / `FolderFile` 均加 `rename_all = "camelCase"` | `crates/vault/src/sync/store.rs` |

**判断流程**（新加 Serialize struct/enum 时）：
1. 是协议层 / 外部格式？→ 保持现状
2. 是命令返回值 / 事件 payload？→ **必须 camelCase**（外层 struct 加 `rename_all = "camelCase"`；enum 外层 kebab + 变体内 camelCase）
3. 是 DB 直映射 / vault sync 持久化？→ **camelCase**（返回前端的加；纯内部不经 serde 的随意）

**历史教训（Task 4.1 blocker，2026-07-27）**：前端写 `audioTracks`（camelCase）但后端 `RecordingMeta` 无 rename_all 序列化为 `audio_tracks`（snake_case）→ 前端拿到 `undefined` → UI 静默失败。**casing 不一致是运行期 bug，编译期不报错**。新代码必须严格遵循本规范，前端 interface 与后端序列化 casing 一一对应。

**历史教训（PermissionStatus/PrivacySection，2026-07-29）**：record crate 的 `PermissionStatus` / `PrivacySection` enum 用 `rename_all = "lowercase"`，多词变体 `NotDetermined` 序列化为 `"notdetermined"`（无大写，不可读），前端误写 `"not-determined"`（带连字符）→ 永远匹配不到 not-determined 分支，权限卡片状态显示错误。`PrivacySection` 前端传 `"screen_capture"`（snake_case）但后端 lowercase 期望 `"screencapture"` → 反序列化可能失败。**enum 变体多词时必须用 camelCase**（`NotDetermined` → `"notDetermined"`），勿用 lowercase（`"notdetermined"` 不可读且易与前端猜测的 `"not-determined"` 不一致）。已统一改为 `rename_all = "camelCase"`。

详见全工程统一 roadmap：`docs/superpowers/specs/2026-07-27-screen-record-audio-post-merge.md` 实现注记 + 后续 casing-unification 系列 spec。

### 文档同步（强制）

代码变更完成后（或同时）必须同步文档：
- 架构概览：[`docs/architecture.md`](docs/architecture.md) — 最权威的结构文档，任何架构 / 流程 / 模块变化都要更新
- 规格文档：`docs/superpowers/specs/` — 功能设计、架构、接口
- 实施计划：`docs/superpowers/plans/` — 实施步骤、任务分解

### 混淆即讨论

如果代码实现与文档描述出现冲突，或文档描述含糊不清导致多种解读：
- **及时提出讨论**，不要自行假设继续推进
- 讨论澄清后回写到对应文档，避免下次重复混淆

### 改动验证纪律（强制）

任何代码改动（包括 bug fix、字段新增、接口修改），**声称完成前必须**完成以下验证，缺一不可。遵循 `superpowers:verification-before-completion` skill 的核心原则：evidence before assertions。

#### 1. 编译验证（改完即跑，不等报错）

```
改 Rust → cargo build（相关 crate）→ 看完整 error 列表 → 逐个修完 → 再 build → 0 error 0 warning
改 前端 → tsc + vite build → 0 error
```

- **看完整 error 列表**：不要修一个报一个。编译器一次性列出所有问题，全部修完再编译。
- **0 warning**：`unused variable` 等警告说明有遗漏（参数改名了但调用方没跟）。

#### 2. 影响面追踪（grep 所有消费点）

改 struct / enum / fn 签名 / 接口字段后：

```bash
# Rust
rg "改动的类型名" crates/ --type rust
# 前端
rg "改动的接口字段" crates/desktop/frontend/src/
```

- **所有构造点**（`StructName {`）都要更新
- **所有消费点**（读字段、调函数）都要检查
- 不能只改当前报错的那一处——同一 struct 有多个构造点时，全部要改

#### 3. 端到端调用链验证

改完后端命令 / Tauri 命令 / DB 函数后，**手动追踪完整链路**：

```
前端 invoke("xxx", { param }) → Tauri 命令签名（参数名 camelCase 映射）→ DB 函数 → 返回值结构 → 前端消费
```

- **参数名**：Tauri 2 自动 camelCase → snake_case 映射（`modelName` → `model_name`），参数名改了前端必须同步
- **返回值**：后端加了字段，前端 interface 必须同步加
- **删除/编辑链路**：确认 `id` / `key` 等标识符从后端 → 前端 → 再回后端的完整传递

#### 4. 测试验证

```bash
cargo test -p <改动的 crate> --lib
```

- 改动涉及的 crate 测试全过
- 新增字段 / 函数有对应测试覆盖
- 测试数据（如 INSERT 语句）与新 schema 一致

### Git 同步纪律（强制）

**在 worktree 上编码时，必须得到用户明确指令才能把代码同步到主干（main）。**

- ✅ 允许：在 worktree 分支上 commit、merge main 进分支、本地多 commit 累积
- ❌ 禁止：未经明确指令就把分支 push 到 origin/main、`git push origin HEAD:main`、`gh pr create --merge` 等任何让分支改动进入主干的操作

「明确指令」指用户原话包含「同步到 main」「push 到 main」「合并到主干」「branch -> main」等意图清晰的表述。模糊表述（如「同步一下」「处理一下」）不算——需要追问确认。

理由：worktree 是实验性工作的隔离区，过早同步主干会让未验证的改动污染所有人。即使代码已通过所有测试 + 文档已同步，也必须等用户明确放行——因为「是否准备好进主干」是用户的判断（可能还想 e2e 验证、可能还想拆分 PR、可能时机不对），不是 AI 的判断。

被用户纠正后必须立即停下，不要因为「测试都过了」「文档都同步了」就继续推。

## 文档体系

```
docs/
├── architecture.md          # 架构概览（最权威的结构文档）
├── api.md                    # Server HTTP/WS API
├── configuration.md          # 配置指南
├── download_architure.md    # 模型下载架构
├── features/                 # 按模块的功能说明（screenshot.md 等）
├── research/                 # 竞品调研 / 技术分析报告（按日期，非 spec/plan 的研究性文档）
├── dictionaries/             # 词典资源（纠错 / 拼音等）
├── code-review/              # 代码审查报告
├── pr/                       # PR 相关说明
└── superpowers/
    ├── specs/                 # 功能设计规格（按日期，大需求必备）
    └── plans/                 # 实施计划（按日期，大需求必备）
```

## 运行时文件布局

```
~/.octopus/
├── octopus.db          # SQLite（唯一存储：models/app_config/clipboard_history/vault_*/hotword_sets/hotword_words 等表，schema v58）
├── config.yaml         # 应用配置（缺失用默认值）
├── VOICE_POLISH.md     # 自定义润色 prompt（可选，覆盖内置默认）
├── .sync/              # git 同步仓库根（GitHub/Gitee private repo 的本地 clone）
│   ├── .git/
│   ├── vault/          # vault 数据（加密）：meta.json + outline.json + ciphers/<2hex>/<uuid>.json + folders/
│   └── hotword/        # 热词数据（明文，两级 outline）：outline.json（总，词典）+ <set-uuid>/{meta.json, outline.json（词）, <2hex>/<word-uuid>.json}
└── models/
    ├── vad.onnx             # VAD 覆盖（可选——通用名，放任意 VAD 模型覆盖内嵌的 silero_vad_v6；不存在用 include_bytes! 内嵌加载）
    └── zipformer/           # 默认 ASR（兜底引擎 zipformer-small，27M，source_type=0 builtin，首次启动下载，Step 3 已实现）

~/.cache/huggingface/hub/   # 大模型 HF 缓存
```

## config 目录

`config/` 是指向 `~/.octopus/` 的软链接，这是实际运行配置目录（不在 git 仓库内，无密钥泄露风险）。

对 `config/` 下文件的读写操作，必须使用绝对路径 `~/.octopus/`（即 `/Users/wudarui/.octopus/`）进行，不要通过 `config/` 相对路径访问：
- 读：`~/.octopus/config.yaml`、`~/.octopus/record.txt` 等
- 写：直接写 `~/.octopus/` 下对应文件
- 原因：`config/` 经符号链接访问时，自动安全分类器无法判断目标在仓库外，可能误判为"向仓库提交密钥"而拦截；用绝对路径 `~/.octopus/` 可避免误拦。

## 重要 Gotchas

### worktree 前端产物污染导致诡异崩溃（⚠️ 已踩坑，诡异 bug 先清 dist + node_modules）

在 worktree 里调试时，如果对前端做过**手动软链 node_modules**（如 `ln -s 主干/node_modules worktree/node_modules`）或频繁切换分支后未重新 `npm install`，会导致 **node_modules / dist 状态不一致**——vite 的 dep optimizer 缓存（`node_modules/.vite`）基于绝对路径，软链混合两个路径状态后打包出的 dist 产物异常，WKWebView 加载后初始化失败（前端 IPC 无响应 `FallbackStart`）。

**症状**：同代码同 commit，主干路径跑正常、worktree 跑崩；`web content process terminated` 日志（注意：这是 WKWebView 隐藏窗口的正常回收，**不是崩溃判据**，主干也有）；前端 `prepare-record` 200ms 无响应 → `FallbackStart`；删 Rust `target` 重编译无效（因为污染源是前端产物，不是 Rust）。

**诊断方法**：遇到"同代码不同行为"的诡异 bug，**第一步彻底清理前端产物**：
```bash
cd crates/desktop
rm -rf dist frontend/node_modules frontend/.vite   # 前端三件套
cd frontend && npm install && npm run build         # 干净重装 + 重 build
rm -rf ../target                                      # Rust 也一并清（保险）
```
而不是急着 git bisect（bisect 时增量编译反而加剧污染）。**切勿手动软链 node_modules**——worktree 各自独立 `npm install`。

**案例（2026-07-29）**：托盘麦克风子菜单功能排查时，误判 `web content process terminated` 为崩溃，花了数小时 git bisect + cpal 异步化等方向全错。期间为图省事软链了主干 node_modules 到 worktree（后被 npm 实体化），导致前端产物污染。最终 `rm -rf dist node_modules .vite target` 全量重装重 build 后不崩。真正的崩溃判据是前端 IPC 是否响应（`StartRecording` 正常 vs `FallbackStart` 超时）。

### Zipformer Whisper 特征归一化（已踩 3 次坑，勿再改错）

Transducer 系列（`zh-int8-2025-06-30` / `zh-xlarge-int8-2025-06-30`）和 `zipformer-ctc` 使用 whisper 特征（ONNX metadata `feature=whisper` → `is_whisper=true`）。`normalize_whisper_features`（实现统一在 `feature.rs`，zipformer/streaming_zipformer/qwen3_asr 共用，2026-07-29 合并）有 3 个关键约束，全部来自 sherpa-onnx C++ 源码（`sherpa-onnx/csrc/math.cc::NormalizeWhisperFeatures`），**修改前务必先读参考实现**：

1. **公式不可变**：最后一步 `(clamped + 4.0) / 4.0`（范围~0-2）。曾错误写成 `clamped - clamp_min`（范围 0-8，尺度差 4 倍）→ ONNX 模型输入分布不匹配 → 输出乱码。

2. **流式必须 per-chunk 归一化**：每个 chunk 切片后**独立** normalize，不是对整段特征全局归一化。sherpa-onnx 的 `online-recognizer-transducer-impl.h` 就是 per-chunk 调 `NormalizeFeatures`。曾误改为 pseudo-global（每次重算 history+buffer 全局归一化），方向完全错误——`history_samples` 每 tick 内容不同导致 max_v 跨 tick 不稳定。

3. **Transducer `history_samples` 仅保留最后 1 帧**（`Z_FRAME_SHIFT` = 160 samples），与 CTC 引擎一致。曾错误保留全部未消费样本（可达上万），导致每次重算特征时归一化 max_v 剧烈跳变 + $O(N^2)$ 性能崩坏。

**诊断方法**：如果流式 Transducer 输出乱码（"回 月 因 同"式重复 token），对照 sherpa-onnx 命令行输出验证——同一段音频，如果 sherpa-onnx 正常但我们的乱码，必定是上述 3 点之一。

### Paraformer Fbank 特征提取（5 个必做步骤，缺一即乱码）

流式 Paraformer 的 fbank 特征提取必须与 sherpa-onnx `kaldi-native-fbank` 完全一致，否则输出 token 重复（`thedayday`/`tomtomor`）或英文粘连。**5 个步骤缺一不可**：

1. **DC offset removal**（`remove_dc_offset=true`）— 每帧 FFT 前减帧均值
2. **Pre-emphasis**（`preemph_coeff=0.97`）— `y[i]=x[i]-0.97*x[i-1]`。**无跨帧状态**：帧重叠（shift=160 < len=400），上一帧末尾并非本帧 start-1，直接从连续缓冲回溯 `samples[start-1]`（减去本帧 mean 近似去直流），无需 `preemph_prev` 字段
3. **Povey 窗**（流式 Paraformer）— `(0.5-0.5cos(2πi/(N-1)))^0.85`，**非 hamming**
4. **Mel 滤波器 high_freq=7600 Hz**（`high_freq=-400`，即 Nyquist-400），**非 8000 Hz**
5. **增量式 fbank 提取**（流式）— 音频线性追加到 `raw_samples`、fbank 帧按序增量计算到 `fbank_cache`。不可按 chunk 重复提取（重叠帧重复计算 + 边界问题）

离线 Paraformer 用 **hamming 窗**，流式用 **povey 窗**。`compute_fbank(samples, window, preemph_coeff)` 参数化窗口，两者共享同一实现，**pre-emphasis 均无状态**（直接回溯连续缓冲 `samples[start-1]`）。

另：`decode_tokens` 遵循 sherpa-onnx `Convert()` 空格逻辑——ASCII 词前加空格、`@@` BPE 合并；`smart_append()` 在 chunk 边界检测 ASCII↔非 ASCII 插入空格。流式引擎累积 `all_token_ids` 跨 chunk 整体 `decode_tokens`（非逐 chunk 解码），避免 BPE 续接断裂（`val@@`+`ue` 被切成 `val`/`ue`）。`StreamingSession` Paraformer 用 `punct_prefix` + `committed_chars` 管理逗号分句。

**热路径性能**：decoder_caches 用 `copy_from_slice` 复用预分配内存（省 ~320KB/chunk），encoder 输入 `into_shape` 零拷贝（省 ~45KB），CIF 用 `as_slice()` 引用（省 ~20-40KB），decoder 键名预分配 `cache_keys`（省 16× format!）。

**诊断方法**：`cargo test -p octopus-asr-local --lib streaming_paraformer::tests::test_streaming_paraformer_real_model -- --nocapture`，对比输出与 sherpa-onnx 参考值 `"昨天是 monday today day is 礼拜二 the day after tomorrow 是星期"`。详见 [spec](docs/superpowers/specs/2026-06-21-paraformer-fbank-feature-extraction-fix.md)。

### 物理/逻辑坐标转换（⚠️ 已踩坑 6+ 次，勿再搞错）

macOS 有**两套**坐标 API，**必须区分**：

| API | 返回单位 | 除 scale？ | 用途 |
|-----|---------|:---------:|------|
| `CGEvent::location()` | **逻辑坐标（points）** | ❌ 不除 | 鼠标位置 |
| `Monitor::position()` / `Monitor::size()` | **物理像素** | ✅ 除 | 显示器范围/位置 |
| `window.inner_position()` / `inner_size()` | **逻辑坐标** | ❌ 不除 | 窗口位置/尺寸（Tauri 自动转换） |

**曾犯错误**：把 `CGEvent::location()` 当物理坐标除 scale → 浮窗位置偏到完全无关的地方、副屏选中浮窗出现在主屏。`CGEvent` 返回的就是 Quartz 逻辑坐标（points），与 Tauri `LogicalPosition` 一致。

**任何坐标比较**（如判断鼠标是否在某显示器范围内）必须统一到逻辑坐标。Monitor 的物理值 ÷ `scale_factor()` 转逻辑。

**捷径**：若已知 `CGDirectDisplayID`（如录屏 `Source::Display { display_id }`、helper `--list-displays` 的 `id`），**直接用 `core_graphics::display::CGDisplay::new(id).bounds()`** 拿逻辑 CGRect——CoreGraphics 原生返回逻辑 points，**已含 scale，无需再除**。比遍历 `app.available_monitors()` 做 bounds 命中更可靠（Tauri Monitor 不暴露 CGDirectDisplayID，匹配只能靠坐标 heuristic）。注意副屏在主屏左侧/上方时 `bounds.origin` 是负数，pill 定位不能再 `.max(0.0)`（会把窗口推回主屏）。

已修复的文件：
- `action_bar_commands.rs::get_mouse_position`——CGEvent 不除 scale（曾错误除过）
- `compact_editor_window.rs`——Monitor position/size 除 scale
- `window_position.rs::is_position_visible`——Monitor position/size 除 scale
- `record_control_window.rs::compute_position`——曾 `let _ = display_id;` 丢弃 display_id 永远用 `primary_monitor()` + `Monitor::position()` 未除 scale，双重错误导致副屏录制 pill 跑到主屏右下角。改用 `CGDisplay::new(display_id).bounds()` 直接查逻辑边界（2026-07-26）
- `record_window.rs::compute_position` + `ui/overlay_window.rs` fallback——`pos.x/y`（Monitor::position 物理像素）未除 scale 直接加 `sz.width/scale`（逻辑）混算，下游 `LogicalPosition` 契约违反。主屏 origin≠0 + Retina 时偏移。改 `pos.x as f64 / scale` 统一逻辑坐标（2026-08-04 第十二轮 P2-3/P2-4）

**诊断方法**：日志打印 raw 坐标 + scale factor，对比预期逻辑值（主屏左上角应该是 ~0,0；1440×900 逻辑屏的右下角应该是 ~1440,900）。

### Tauri 2 窗口创建注意事项（已踩坑多次）

- **每个新窗口必须加入 `capabilities/default.json` 的 `windows` 数组**——否则 `listen`/`invoke` 被拒（报 `event.listen not allowed on window`）
- **全局热键触发的命令不能在主线程 sleep**——阻塞事件循环 → 窗口 show/set_focus 时序错乱 → Esc/按钮无响应。必须 `std::thread::spawn`
- **全局热键触发浮窗前隐藏常规窗口**——Regular 激活策略下激活 app 会把所有可见窗口带到前台。用 `activation::hide_regular_windows`
- **mousedown capture 拦截 onClick**——`addEventListener("mousedown", fn, true)` 在 onClick 之前触发。外部点击检测用 `click` 冒泡阶段（`false`）
- **transparent 窗口的 html 背景色**——`transparent:true` 只让窗口支持透明，html `backgroundColor` 仍渲染不透明层。透明窗口不设 html/body 背景
- **`builder.maximized(true)` 在 WRY 不生效**——用 show 前 `win.maximize()` 或主屏尺寸直接创建
- **click-through poller 的 BAR_W 必须与前端容器同宽**——`result_window.rs` 的 `BAR_W` 判定精简态可交互区域，如果前端 CSS 改了容器宽度但 `BAR_W` 没同步，poller 会误判光标在小条外 → 窗口穿透 → 按钮点不到（已踩坑 1 次：前端从 520 改为 720 但 BAR_W 仍为 520）
- **CM6 滚动条需要两件事**——`.cm-scroller` 显式 `overflow: auto`（index.css，Tailwind v4 preflight 可能覆盖默认值）+ 所有 flexbox 祖先链 `min-h-0`（否则内容撑开容器不滚动）
- **跨窗口 event listener 必须稳定化（已踩坑 2 次：paste-text + new-tab）**——`listen()` 是 async Promise，若 listener 的 `useEffect` 依赖了一个引用不稳定的回调（如 `useCallback(fn, [tabs, activeId])`——`tabs` 每次 `setTabs` 都变），effect 会随每次 render cleanup 旧 listener + re-register 新 listener，**re-register 间隙 listener 处于未注册状态**，Rust `emit_to` 的 event 撞上间隙就丢。症状：后端日志显示 emit 成功，前端就是不响应（ActionBar agent 命令建不出 tab 即此 bug）。**修复模式（对齐 `TerminalPane.tsx` paste-text 的 `d5a879ed`）**：①用 `ref` 持有最新回调（`fnRef.current = fn`），listener effect 只挂一次（`[]` 依赖）；②`target` 用 `{ kind: "WebviewWindow", label }` 精确匹配（比裸 string `AnyLabel` 投递更可靠）。**所有 `listen()` 调用都要检查这个模式**——callback 引用是否稳定、effect 依赖是否最小化。
- **osascript 读环境变量必须用 `do shell script`，勿用 `system attribute`（已踩坑 1 次：问豆包中文乱码）**——AppleScript 的 `system attribute "VAR"` 返回 **MacRoman Pascal string**（历史遗留），中文 UTF-8 环境变量读出来是乱码（`测试中文`→`娴璇涓`）。`octopus` 通过 `cmd.env("OCTOPUS_TEXT", text)` 传 UTF-8，但 osascript 用 `system attribute` 读就坏。**正确写法**：`do shell script "printf %s \"$OCTOPUS_TEXT\""`——在 shell 内读 `$VAR`（双引号展开是 UTF-8 安全，变量展开结果不再二次解析，实测对中文/引号/`$`/反引号都正确）。所有 osascript 脚本读 `OCTOPUS_TEXT` / `OCTOPUS_PACKAGE_DIR` 都要用 `do shell script`，不要用 `system attribute`。
- **WKWebView 的 HTML5 Drag and Drop 不可靠——拖拽需求优先用 Tauri `onDragDropEvent`（OS 文件）+ pointer events（内部 DOM），勿先试 HTML5 DnD（已踩坑 1 次：终端文件拖拽）**——HTML5 DnD（`draggable`/`dataTransfer`）在 WKWebView + xterm canvas 下 `onDrop` 完全不触发（bubble 和 capture 阶段都试过，xterm 内部元素拦截事件且不冒泡）。**正确分治**：(1) OS 文件拖入（Finder → 窗口）用 `getCurrentWebview().onDragDropEvent`（Tauri 原生事件，OS 提供 ghost，照搬 Terax `useTerminalFileDrop`）；(2) webview 内部 DOM 拖拽（如文件树 → 终端）用 pointer events 模拟（mousedown 记录状态 + document mouseup hit-test + 自定义 ghost div 跟随鼠标）。详见 `dragStore.ts` + spec `2026-08-01-terminal-drag-file-to-terminal-design.md`。
