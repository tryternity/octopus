# 屏幕录制（Screen Record）

macOS 端基于 ScreenCaptureKit 的屏幕录制（display / window / area 三种源）+ 实时标注 + 录后音轨合并 + GIF 导出 + 自动字幕。仅 macOS（record crate 暂只 mac provider）。

## 启动与源选择

- **快捷键** `Cmd+Shift+R`（toggle，全局常驻）+ `Esc`（停止，**按需注册**——录制开始 register、停止/kill unregister，避免吞掉其他窗口的 DOM 级 Esc）
- **托盘菜单** toggle 单项（idle「开始录屏 ⌘⇧R」→ recording「停止录屏 ⎋」）
- **配置浮窗** `record_window.rs` + `RecordConfig.tsx`（`record-config.html`）：选 display / window / area + 音频开关（系统音频 + 麦克风）

## 三种录制源

| 源 | 控制浮窗 | 标注 |
|---|---|---|
| display / window | `record_control_window.rs` pill（桌面右下角，红点+时长+暂停+停止） | 无 |
| area | `record_annotation_window.rs` overlay（always_on_top 全屏透明，SCK 录到标注进视频） | 实时标注（矩形/箭头/画笔/文字/马赛克等，复用 `components/Annotation/`） |

- **区域选区** `record_area_picker.rs` + `AreaPicker.tsx`：复用 screenshot 多屏全屏窗口 + 坐标换算
- **pill 副屏定位** 用 `CGDisplay::bounds()` 查逻辑边界（修副屏 pill 跑主屏 bug）

## 录屏 helper（Swift sidecar）

vendor openscreen 项目的 ScreenCaptureKit helper（`crates/record/native/macos/`，MIT）。主进程 spawn 子进程，JSON-over-stdio 通信（argv[1]=RecordingRequest，stdout=HelperEvent 流，stdin=命令）。帧数据不经 IPC——SCStream → AVAssetWriter 在 helper 内闭环写文件。

详见 architecture.md「屏幕录制」段 + archived/screen-record-design spec。

## 音轨

- 录制双轨输出（system + mic 各一轨）
- **录后按需合并**：停止后 `ffprobe` 探测实际音轨 → 写 DB `audio_tracks` JSON + mp4 udta metadata；前端 hover 显示音轨标签，双轨时显示「合并音轨」按钮 → ffmpeg `amix`（非 `amerge`，声道不同）→ 另存 `xxx_merged.mp4`

## 录制历史

设置页 RecordingPanel：列表视图（缩略图 + 时长 + 音轨标签 + 收藏 + 删除）。命令 `list/get/thumbnail/rename/favorite/delete/open/reveal`。

- **删除**（2026-07-29）：`permanent=true` 物理删 DB 行 + 磁盘 mp4 + 关联 `.N.srt`；`permanent=false` 仅删 DB 行。前端弹框问是否删磁盘文件
- **GIF 导出**（F20）：`export_gif` spawn ffmpeg 裸调 + `check_ffmpeg`（灰禁 + 安装引导，含 evermeet.cx curl 方式）
- **自动字幕**（2026-07-28）：`generate_subtitle` 抽 PCM → ASR 转写 → 生成 SRT（`<recording>.<track>.srt`）+ 可选 LLM 润色

## 异步任务事件

录屏异步任务（GIF / 音轨合并 / 字幕）统一 emit `record://task` 事件，payload 为 `RecordTaskEvent` enum（内部 tagged，变体 kebab-case + 字段 camelCase），替代原独立事件名。

## 存储与配置

- **路径** `~/download/octopus/recordings/`（可配，DB `record_output_dir`）
- **DB** `recordings` 表（schema v54）：id / file_path（绝对路径）/ source / source_type / duration_ms / audio_tracks（JSON）/ has_audio / created_at / is_favorite
- **配置项**（DB app_config 表）：`record_microphone_device` / `record_output_dir` / `record_reveal_after_stop` / `subtitle_llm_polish_default` 等
