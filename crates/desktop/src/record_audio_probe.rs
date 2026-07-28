//! 录屏音频元数据探测——ffprobe 读 mp4 实际轨道（给 Task 2.2 写 metadata 用）。
//!
//! 与 `record_commands.rs` 同为 `#![cfg(target_os = "macos")]`：octopus-record 只在 macOS
//! 编译，本模块依赖 `octopus_record::RawAudioTrack`，故同样仅 macOS 编译。
//!
//! e2e 验证（真实 ffprobe + mp4）留到 Phase 5；本模块无纯单测——ffprobe 路径依赖环境。

#![cfg(target_os = "macos")]

use std::path::{Path, PathBuf};
use std::process::Stdio;

use octopus_record::{infer_audio_tracks, AudioTrack, RawAudioTrack};
use octopus_infra::octopus_config_home;
use serde::Deserialize;

/// 探测 ffprobe 路径。仿 `record_commands.rs::probe_ffmpeg`（同查找顺序）：
/// `~/.octopus/bin/ffprobe`（用户手动放/dlp 下载缓存）→ 系统 PATH（`which ffprobe`）。
///
/// 与 probe_ffmpeg 实现细节略有不同：probe_ffmpeg 第二步只判断 which 是否成功、
/// 成功后直接返回 `"ffmpeg"` 字面量（依赖 PATH 解析）；此处 ffprobe 返回完整路径，
/// 避免调用方在多 binary 目录场景下解析歧义。简化版只跑一次 which（一次 output 拿 stdout）。
pub fn probe_ffprobe() -> Option<PathBuf> {
    // 1. ~/.octopus/bin/ffprobe
    let home_bin = octopus_config_home().join("bin").join("ffprobe");
    if home_bin.exists() {
        return Some(home_bin);
    }
    // 2. 系统 PATH（which，一次 output 直接拿 stdout）
    // 不显式设 stdout——.output() 默认 piped 捕获；stderr 静默（which 找不到会写 stderr）。
    let out = std::process::Command::new("which")
        .arg("ffprobe")
        .stderr(Stdio::null())
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let p = String::from_utf8_lossy(&out.stdout).trim().to_string();
    if p.is_empty() {
        None
    } else {
        Some(PathBuf::from(p))
    }
}

/// ffprobe `-show_streams` 输出的 JSON 顶层结构。
#[derive(Debug, Deserialize)]
struct FfprobeOutput {
    streams: Vec<FfprobeStream>,
}

/// ffprobe 单条 stream。
///
/// `sample_rate` 在 ffprobe JSON 里是字符串（如 `"48000"`），需 `.parse()` 转 u32。
/// `codec_name` / `sample_rate` / `channels` 对视频流/异常情况可能缺失，全标 `Option`。
#[derive(Debug, Deserialize)]
struct FfprobeStream {
    #[serde(rename = "codec_type")]
    codec_type: String,
    #[serde(rename = "codec_name")]
    codec_name: Option<String>,
    #[serde(rename = "sample_rate")]
    sample_rate: Option<String>,
    channels: Option<u32>,
}

/// 跑 ffprobe 解析 mp4 实际音轨。
///
/// 参数：`-v quiet -print_format json -show_streams -select_streams a`——`-select_streams a`
/// 让 ffprobe 只输出音频流，避免过滤视频流的额外开销与潜在 codec_type 缺失。
///
/// 返回的 `Vec<RawAudioTrack>` index 按音频流在 ffprobe 输出中的顺序枚举（0-based），
/// 与 helper `addAudioInput` 顺序对齐（spike 验证 mic 先 add = track 0，system 后 = track 1）。
pub async fn probe_audio_tracks(ffprobe: &Path, mp4: &Path) -> Result<Vec<RawAudioTrack>, String> {
    let output = tokio::process::Command::new(ffprobe)
        .arg("-v").arg("quiet")
        .arg("-print_format").arg("json")
        .arg("-show_streams")
        .arg("-select_streams").arg("a")
        .arg(mp4)
        .output()
        .await
        .map_err(|e| format!("ffprobe spawn 失败: {e}"))?;

    if !output.status.success() {
        return Err(format!(
            "ffprobe 退出码非 0: {}",
            String::from_utf8_lossy(&output.stderr)
        ));
    }

    let parsed: FfprobeOutput = serde_json::from_slice(&output.stdout)
        .map_err(|e| format!("ffprobe JSON 解析失败: {e}"))?;

    // -select_streams a 已过滤，但双重保险：codec_type == "audio" 才收。
    // index 按枚举位置（i）算，不是 ffprobe 原始 stream index——后者在原 mp4 里可能
    // 包含视频流（如 v0/a1），与我们想要的「第 N 条音轨」语义不一致。
    let mut tracks = Vec::new();
    for (i, s) in parsed.streams.iter().enumerate() {
        if s.codec_type != "audio" {
            continue;
        }
        tracks.push(RawAudioTrack {
            index: i as u32,
            codec: s.codec_name.clone().unwrap_or_default(),
            sample_rate: s
                .sample_rate
                .as_ref()
                .and_then(|sr| sr.parse().ok())
                .unwrap_or(0),
            channels: s.channels.unwrap_or(0),
        });
    }
    Ok(tracks)
}

/// 用 ffmpeg `-c copy -metadata` 把 audio_tracks JSON 写进 mp4 udta atom。
///
/// 调用语义：失败不阻断主流程——调用方吞掉错误仅 log warn（DB 已有 audio_tracks 兜底）。
/// 流程：ffmpeg 写入 `*.meta.tmp` 临时文件 → 成功才 `rename` 覆盖原文件；失败删 tmp 避免半残。
///
/// `serde_json::to_string` 兜底 `"[]"`——`AudioTrack` 派生了 `Serialize`，理论上不会失败，
/// 保留兜底以防御未来字段变更（如含不可序列化类型）。
///
/// ffmpeg 参数：`-y -i <mp4> -c copy -metadata octopus_audio_tracks=<json> <tmp>`。
/// `-c copy` 不重编码（流拷贝），秒级完成，仅改写 udta box。
pub async fn write_audio_tracks_metadata(
    ffmpeg: &Path,
    mp4: &Path,
    tracks: &[AudioTrack],
) -> Result<(), String> {
    let json = serde_json::to_string(tracks).unwrap_or_else(|_| "[]".into());
    // 临时文件命名：`<name>.meta.tmp.mp4`。ffmpeg 按输出文件扩展名判断容器格式，
    // 真正的扩展名必须是 `.mp4`（最后一段 dot 之后）。曾用 `with_extension("mp4.meta.tmp")`
    // 产出 `.tmp` 结尾 → ffmpeg 报「Unable to choose an output format」静默失败（stderr
    // 被 null 吞，2026-07-28 e2e 发现）。现把 `.mp4` 放最后：先 with_extension("meta.tmp.mp4")
    // 把原 `.mp4` 替换成 `meta.tmp.mp4`，真扩展名仍是 mp4。
    let tmp = mp4.with_extension("meta.tmp.mp4");

    // stderr piped：失败时读 stderr 进 error message，便于诊断（不再 Stdio::null 吞掉）。
    let output = tokio::process::Command::new(ffmpeg)
        .arg("-y")
        .arg("-i").arg(mp4)
        .arg("-c").arg("copy")
        .arg("-metadata").arg(format!("octopus_audio_tracks={}", json))
        .arg(&tmp)
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .output()
        .await
        .map_err(|e| format!("ffmpeg spawn 失败: {e}"))?;

    if !output.status.success() {
        // 失败删 tmp，避免半残文件残留（_ 忽略「文件不存在」错误）。
        let _ = std::fs::remove_file(&tmp);
        let stderr = String::from_utf8_lossy(&output.stderr);
        let hint = stderr.lines().find(|l| l.contains("Error") || l.contains("Unable"))
            .unwrap_or("（无 Error 行）");
        return Err(format!("ffmpeg metadata 写入失败（退出码非 0）: {hint}"));
    }

    std::fs::rename(&tmp, mp4).map_err(|e| format!("覆盖原文件失败: {e}"))?;
    Ok(())
}

/// 完整流程：ffprobe 读 mp4 → 配置交叉推断 source。失败返回空 vec（不阻断）。
///
/// 给 `stop_and_store_inner` 用的组合函数：把「探测 ffprobe → 跑 mp4 → 推断 source」
/// 三步串起来，统一失败兜底（任一步失败返回空 vec，调用方不阻断录制入库）。
///
/// 参数：
/// - `mp4`：录屏 mp4 绝对路径
/// - `system_enabled` / `mic_enabled`：start 时用户配置（决定 source 推断方向）
/// - `mic_device_name`：start 时解析的麦克风设备名（None 时 mic 轨 device_name 字段为空）
pub async fn probe_recording_audio_tracks(
    mp4: &Path,
    system_enabled: bool,
    mic_enabled: bool,
    mic_device_name: Option<&str>,
) -> Vec<AudioTrack> {
    let ffprobe = match probe_ffprobe() {
        Some(p) => p,
        None => {
            log::debug!("[record] ffprobe 不可用，audio_tracks 兜底空");
            return vec![];
        }
    };
    match probe_audio_tracks(&ffprobe, mp4).await {
        Ok(raw) => infer_audio_tracks(raw, system_enabled, mic_enabled, mic_device_name),
        Err(e) => {
            log::warn!("[record] ffprobe 解析失败: {e}");
            vec![]
        }
    }
}

/// 合并产物路径：`xxx.mp4` → `xxx_merged.mp4`（同目录）。已含 `_merged` 不重复加。
///
/// 给 `merge_audio_tracks` 算 ffmpeg 输出路径用——与源同目录便于 reveal/file 管理，
/// 文件名加 `_merged` 后缀让用户在 Finder 里一眼区分。
///
/// 幂等性：源文件名已含 `_merged`（如重复合并）时原样返回，避免 `x_merged_merged.mp4`。
///
/// 边界处理：
/// - `file_name` 拿不到（如 `/` 根路径）→ 兜底 `"output.mp4"`（极端防御，正常路径不会触发）。
/// - 非 `.mp4` 扩展名（如 `.mkv`）→ 当前仍按 `.mp4` 后缀规则匹配（trim_end_matches(".mp4")
///   对 `.mkv` 无效，stem 即整个文件名 `xxx.mkv`，结果 `xxx.mkv_merged.mp4`）——
///   录屏当前只产 mp4，不做更复杂扩展名推断。
pub fn merged_output_path(input: &Path) -> PathBuf {
    let file_name = input
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("output.mp4");
    let stem = file_name.trim_end_matches(".mp4");
    let already_merged = stem.ends_with("_merged");
    let new_name = if already_merged {
        file_name.to_string()
    } else {
        format!("{stem}_merged.mp4")
    };
    input.with_file_name(new_name)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    /// 回归：`xxx.mp4` → `xxx_merged.mp4`（核心契约——给 `merge_audio_tracks` 算输出路径）。
    #[test]
    fn merged_output_path_appends_merged_suffix() {
        let p = PathBuf::from("/tmp/2026-07-27_10.30.00_123.mp4");
        let m = merged_output_path(&p);
        assert_eq!(
            m.file_name().unwrap().to_str().unwrap(),
            "2026-07-27_10.30.00_123_merged.mp4"
        );
    }

    /// 回归：已含 `_merged` 不重复加（避免 `x_merged_merged.mp4`——重复合并场景的幂等性）。
    #[test]
    fn merged_output_path_no_double_suffix() {
        let p = PathBuf::from("/tmp/x_merged.mp4");
        let m = merged_output_path(&p);
        assert_eq!(m.file_name().unwrap().to_str().unwrap(), "x_merged.mp4");
    }

    /// 回归：保留原目录（merged.mp4 必须与源同目录，便于 reveal/file 管理）。
    #[test]
    fn merged_output_path_preserves_dir() {
        let p = PathBuf::from("/Users/wudarui/.octopus/recordings/abc.mp4");
        let m = merged_output_path(&p);
        assert_eq!(m.parent(), p.parent());
    }
}
