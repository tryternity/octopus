# 2026-07-26 录屏 GIF 导出（F20，P3 独立项）

## 背景

录屏 spec §2.20 规划的 F20：把已录制的 MP4 转成 GIF。spec 标 P3（高级，中长期），但 F20 是 P3 里**最独立**的一项（不依赖光标轨迹 helper、时间线编辑器、AVFoundation webcam），可以单独推进。

## 范围

- 录屏历史列表每行加「导出 GIF」按钮（Clapperboard 图标）
- 点击 → 后端 spawn ffmpeg 转 GIF → toast 反馈
- GIF 自动保存到源 MP4 同目录、同名 `.gif`（`-y` 覆盖）

## 设计决策

| 决策 | 选择 | 理由 |
|---|---|---|
| ffmpeg 路径解析 | 复制 dlp 的 `get_binary_path` 到 record_commands.rs 作私有 `find_ffmpeg()` | 不动 dlp（GPLv3 隔离）/ infra（职责扩张）；desktop 已有 4 处 which 内联副本 |
| 保存位置 | 自动保存到 `{源MP4}.gif` | MVP 最简；录屏文件已在 `~/.octopus/recordings/`，用户可控；对话框打断轻量感 |
| 进度反馈 | toast + `record://gif-*` 事件 | ffmpeg 单遍转码无流式进度，spinner + toast 起止点足够 |
| 图标 | `Clapperboard` | `Film` 已被占用（录屏本体）；Clapperboard=场记板，"后期产物"语义 |
| DB 字段 | 不加 gif_path | GIF 是衍生文件非元数据；同名规则可推断；避免 schema 升级 |
| 依赖 | 不加 ffmpeg_sidecar | 裸调 `tokio::process::Command`；ffmpeg 缺失则报错引导 `brew install ffmpeg` |

## ffmpeg 参数（spec §2.20）

```bash
ffmpeg -y -i input.mp4 -vf "fps=15,scale=800:-1:flags=lanczos" -loop 0 output.gif
```

- `fps=15` 15 帧/秒（GIF 体积权衡）
- `scale=800:-1` 宽度上限 800px，`-1` 保持宽高比
- `flags=lanczos` 高质量缩放
- `-loop 0` 无限循环
- 不用两遍调色板优化（palettegen/paletteuse）—— MVP 简化，质量够用，优化版耗时翻倍

## 实施（2 改动 + i18n）

### 改动
| 文件 | 改动 |
|---|---|
| `crates/desktop/src/record_commands.rs` | + `find_ffmpeg()` 私有函数；+ `export_gif` 命令（~70 行，含 emit 事件） |
| `crates/desktop/src/main.rs` | invoke_handler 加 `record_commands::export_gif` |
| `crates/desktop/frontend/src/pages/Settings/RecordingPanel.tsx` | import Clapperboard/Loader2；父加 gifExportingId state；RecordingRowProps 加 gifExportingId/onExportGif；handleExportGif + 按钮 JSX（Captions 后、Trash2 前） |
| `crates/desktop/frontend/src/locales/zh-CN.yaml` + `en.yaml` | settings.recordings 下加 exportGif/exportGifStarted/exportGifDone/exportGifFailed |

### 不改动
- ❌ db.sql（不加 gif_path）
- ❌ paths.rs（不加 ffmpeg 工具）
- ❌ dlp crate（GPLv3 隔离）
- ❌ Cargo.toml（不加 ffmpeg_sidecar）

## 事件流

```
前端按钮 → invoke("export_gif", {id})
       ↓ emit record://gif-started {id}
后端 spawn ffmpeg
       ↓ 成功：emit record://gif-done {id, path} + Ok(path)
       ↓ 失败：emit record://gif-failed {id, error} + Err
前端 await invoke → toast（成功/失败）+ 清 loading
```

事件作为多窗口同步备用（Settings 窗口 invoke 等待时，其他窗口若监听也能更新）；MVP 前端靠 invoke 返回值处理，不强制监听事件。

## 验证

### 编译
- `cargo build --release -p octopus-desktop` → 0 error 0 warning
- `npm run build` → 0 error

### 端到端（用户实测）
1. 录一段 10 秒视频
2. 历史列表点 GIF 按钮（Clapperboard）→ spinner 转 + toast「正在导出」
3. 几秒后 toast「已导出到 xxx.gif」
4. Finder 显示能看到 .gif 文件
5. 未装 ffmpeg 时 → toast 报错引导 `brew install ffmpeg`

## 风险

1. **ffmpeg 未安装**：报错引导 brew install。MVP 不做启动时探测 + 灰按钮（体验优化留后续）
2. **大文件耗时**：toast + spinner 兜底；超长视频 GIF 无意义，MVP 不警告
3. **同名覆盖**：`-y` 覆盖（重导出即覆盖是合理默认）

## 不在范围

- 启动时 ffmpeg 探测 + 按钮灰禁（体验优化）
- 调色板两遍优化（质量优化，耗时翻倍）
- 另存为对话框（导出位置选择）
- F15 ASR 字幕 / F17 全文搜索（P2，依赖 F15）
- F18 可编辑光标 / F19 录后编辑器 / F21 摄像头（P3 大需求）
