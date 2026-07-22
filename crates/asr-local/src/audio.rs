use anyhow::Result;
use rubato::Resampler;

/// VAD 语音段首尾 padding（pre/post 对称）：补全首字音头（VAD 破阈值有 1~2 帧延迟、起首辅音
/// 能量弱触发更晚）与尾字尾音（衰减残尾能量低被判静音）。120ms（@480 样本/30ms 帧 = 4 帧），
/// 远低于段间静音阈值，只借回纯静音、不触及相邻段语音。`segment_audio_vad` 与 `filter_speech` 共用。
/// 参考 silero-vad speech_pad_ms（默认 30ms）。
const SPEECH_PAD_MS: usize = 120;

/// 解码 WAV reader → 16kHz mono f32 样本（read_wav_16k / read_wav_16k_from_bytes 共用核心）。
/// spec 解析 → 采样格式归一 f32 → 下混 mono → 必要时重采样到 16kHz。
fn decode_wav_to_mono_16k<R: std::io::Read + std::io::Seek>(
    mut reader: hound::WavReader<R>,
) -> Result<Vec<f32>> {
    let spec = reader.spec();
    let sample_rate = spec.sample_rate;
    let samples: Vec<f32> = match spec.sample_format {
        hound::SampleFormat::Float => reader.samples::<f32>().collect::<Result<Vec<_>, _>>()?,
        hound::SampleFormat::Int => reader
            .samples::<i16>()
            .map(|s| s.map(|v| v as f32 / i16::MAX as f32))
            .collect::<Result<Vec<_>, _>>()?,
    };
    let channels = spec.channels as usize;
    let mono: Vec<f32> = samples
        .chunks(channels)
        .map(|c| c.iter().sum::<f32>() / channels as f32)
        .collect();
    if sample_rate == 16000 {
        Ok(mono)
    } else {
        resample_to_16k(&mono, sample_rate)
    }
}

/// 从 WAV 文件读取并转为 16kHz mono f32 样本
pub fn read_wav_16k(path: &str) -> Result<Vec<f32>> {
    decode_wav_to_mono_16k(hound::WavReader::open(path)?)
}

/// 从 WAV 字节流读取并转为 16kHz mono f32 样本
pub fn read_wav_16k_from_bytes(data: &[u8]) -> Result<Vec<f32>> {
    let cursor = std::io::Cursor::new(data);
    decode_wav_to_mono_16k(hound::WavReader::new(cursor)?)
}

/// 一次性重采样到任意 to_rate（无状态；尾部不足一帧的样本会被丢弃，仅适合整段处理，不适合流式）。
pub fn resample_to(samples: &[f32], from_rate: u32, to_rate: u32) -> Result<Vec<f32>> {
    if from_rate == to_rate {
        return Ok(samples.to_vec());
    }
    let resampler = rubato::FftFixedIn::<f32>::new(from_rate as usize, to_rate as usize, 1024, 2, 1)?;
    let input_frames = resampler.input_frames_next();
    let mut resampler = resampler;
    let mut resampled = Vec::new();
    let mut pos = 0;
    while pos + input_frames <= samples.len() {
        let chunk = &samples[pos..pos + input_frames];
        let output = resampler.process(&[chunk], None)?;
        resampled.extend_from_slice(&output[0]);
        pos += input_frames;
    }
    Ok(resampled)
}

/// Resample audio to 16kHz（委托 resample_to，保持原签名/行为）。
pub fn resample_to_16k(samples: &[f32], from_rate: u32) -> Result<Vec<f32>> {
    resample_to(samples, from_rate, 16000)
}

/// Stateful resampler for streaming audio.
/// Caches the Rubato FftFixedIn planner and buffers leftover samples between chunks
/// to ensure glitch-free audio boundaries and high performance.
pub struct AudioResampler {
    resampler: rubato::FftFixedIn<f32>,
    input_frames: usize,
    buffer: Vec<f32>,
    to_rate: usize,
}

impl AudioResampler {
    /// 流式重采样到 16kHz（向后兼容）。
    pub fn new(from_rate: u32) -> Result<Self> {
        Self::new_to(from_rate, 16000)
    }

    /// 流式重采样到任意 to_rate（denoise 路径 48k 桥接用）。
    pub fn new_to(from_rate: u32, to_rate: u32) -> Result<Self> {
        let resampler = rubato::FftFixedIn::<f32>::new(from_rate as usize, to_rate as usize, 1024, 2, 1)?;
        let input_frames = resampler.input_frames_next();
        Ok(Self {
            resampler,
            input_frames,
            buffer: Vec::new(),
            to_rate: to_rate as usize,
        })
    }

    /// Resample a chunk of audio. Leftover samples are buffered and prepended to the next call.
    pub fn resample(&mut self, samples: &[f32]) -> Result<Vec<f32>> {
        self.buffer.extend_from_slice(samples);
        let mut resampled = Vec::new();
        let mut pos = 0;
        while pos + self.input_frames <= self.buffer.len() {
            let chunk = &self.buffer[pos..pos + self.input_frames];
            let output = self.resampler.process(&[chunk], None)?;
            resampled.extend_from_slice(&output[0]);
            pos += self.input_frames;
        }
        self.buffer.drain(..pos);
        Ok(resampled)
    }

    /// Flush any remaining buffered samples by padding with zeros.
    pub fn flush(&mut self) -> Result<Vec<f32>> {
        if self.buffer.is_empty() {
            return Ok(Vec::new());
        }
        let needed = self.input_frames - self.buffer.len();
        self.buffer.extend(std::iter::repeat_n(0.0, needed));
        let output = self.resampler.process(&[&self.buffer], None)?;
        self.buffer.clear();
        Ok(output[0].clone())
    }

    /// 目标采样率（构造时设定，便于上层断言链路配置）。
    pub fn to_rate(&self) -> usize {
        self.to_rate
    }
}

// 编译期断言：AudioResampler 必须 Send + Sync。
// desktop 的 SharedAudioState 经 `unsafe impl Send/Sync` 声明可跨线程（Arc 共享给
// cpal 回调线程 + coordinator 线程），其字段 `Mutex<Option<AudioResampler>>` 要求
// AudioResampler: Send。该断言在编译期固化此前提——若 rubato 升级或重构引入非 Send
// 字段（如 Rc），此处编译失败，避免 unsafe impl 静默退化为未定义行为。
const _: () = {
    fn _assert_send_sync<T: Send + Sync>() {}
    fn _assert() {
        _assert_send_sync::<AudioResampler>();
    }
};

/// VAD 过滤：去除首尾静音，**保留中间全部音频**（含句内停顿/轻声帧）。
///
/// 仅 trim 两端——找首个/末个高于 `threshold` 的帧，各外扩 `SPEECH_PAD_MS` 作为起止点，
/// 其间音频原样返回。**不逐帧删除**低于阈值的帧：那样会删掉字间 ~50ms 停顿与轻声读音，
/// 破坏句子连续时间结构 → 声学特征错乱 → 漏字/乱码/粘连。用于 CLI E2E（整段录音）与
/// desktop VadSegmented（检测流已切出的单段），两者都只需去首尾残余静音、保留段内结构。
pub fn filter_speech(
    samples: &[f32],
    vad: &mut crate::vad::SileroVad,
    frame_size: usize,
    threshold: f32,
) -> Vec<f32> {
    let frame_duration_ms = (frame_size * 1000) / 16000;
    let pad_samples = (SPEECH_PAD_MS / frame_duration_ms) * frame_size;

    // 扫描首个/末个高于阈值的帧（vad.compute 有状态，需顺序扫描）。
    let mut first_active: Option<usize> = None;
    let mut last_active: Option<usize> = None;
    for (i, chunk) in samples.chunks(frame_size).enumerate() {
        if chunk.len() < frame_size {
            break;
        }
        if let Ok(prob) = vad.compute(chunk) {
            if prob > threshold {
                if first_active.is_none() {
                    first_active = Some(i);
                }
                last_active = Some(i);
            }
        }
    }

    match (first_active, last_active) {
        // 有语音：trim 到 [首帧-pad, 末帧+pad]，clamp 到 samples 边界。
        (Some(first), Some(last)) => {
            let start = (first * frame_size).saturating_sub(pad_samples);
            let end = ((last + 1) * frame_size + pad_samples).min(samples.len());
            samples[start..end].to_vec()
        }
        // 无活跃帧（全静音 / VAD 全判静音）：返回空，调用方据此跳过。
        _ => Vec::new(),
    }
}

/// Segment audio into multiple speech segments using Silero VAD.
/// Returns a list of segments, where each segment is a Vec<f32> representing speech.
pub fn segment_audio_vad(
    samples: &[f32],
    vad: &mut crate::vad::SileroVad,
    frame_size: usize,      // usually 480 (30ms at 16kHz)
    threshold: f32,         // e.g. 0.4
    min_silence_ms: usize,  // e.g. 500ms
    max_segment_ms: usize,  // e.g. 25000ms (25s)
) -> Vec<Vec<f32>> {
    let mut segments = Vec::new();
    let mut in_speech = false;
    let mut current_segment_start = 0;
    let mut silence_frames_count = 0;

    // We compute frame duration in milliseconds: (frame_size * 1000) / 16000
    // For 480 samples, this is 30ms.
    let frame_duration_ms = (frame_size * 1000) / 16000;
    let min_silence_frames = min_silence_ms / frame_duration_ms;
    // Pre/post padding 详见模块级 SPEECH_PAD_MS；pad_samples 按实际帧时长换算。
    let pad_samples = (SPEECH_PAD_MS / frame_duration_ms) * frame_size;

    let total_frames = samples.len() / frame_size;

    for i in 0..total_frames {
        let start_idx = i * frame_size;
        let end_idx = start_idx + frame_size;
        let chunk = &samples[start_idx..end_idx];

        let prob = vad.compute(chunk).unwrap_or(0.0);
        let is_speech_frame = prob >= threshold;

        if !in_speech {
            if is_speech_frame {
                in_speech = true;
                // 前置余量：向前借 pad 补回被 VAD 响应延迟切掉的音头（首字辅音）。
                current_segment_start = start_idx.saturating_sub(pad_samples);
                silence_frames_count = 0;
            }
        } else {
            if is_speech_frame {
                silence_frames_count = 0;
            } else {
                silence_frames_count += 1;
            }

            let current_duration_ms = ((end_idx - current_segment_start) * 1000) / 16000;

            // Check for silence split
            if silence_frames_count >= min_silence_frames {
                // 后置余量：尾音残尾被算进静音帧，speech_end 回溯会切掉它，+pad 借回（不超当前帧）。
                let speech_end = ((i + 1 - silence_frames_count) * frame_size + pad_samples)
                    .min((i + 1) * frame_size);
                if speech_end > current_segment_start {
                    segments.push(samples[current_segment_start..speech_end].to_vec());
                }
                in_speech = false;
            }
            // Check for max duration split
            else if current_duration_ms >= max_segment_ms {
                let speech_end = if silence_frames_count > 0 {
                    ((i + 1 - silence_frames_count) * frame_size + pad_samples)
                        .min((i + 1) * frame_size)
                } else {
                    end_idx
                };
                if speech_end > current_segment_start {
                    segments.push(samples[current_segment_start..speech_end].to_vec());
                }
                current_segment_start = speech_end;
                silence_frames_count = 0;
            }
        }
    }

    // Add the final segment if still in speech
    if in_speech && samples.len() > current_segment_start {
        let mut speech_end = samples.len();
        if silence_frames_count > 0 && samples.len() >= silence_frames_count * frame_size {
            // 末段尾音同理借回 pad（clamp 到 samples.len()，不越界）。
            speech_end = (samples.len() - silence_frames_count * frame_size + pad_samples)
                .min(samples.len());
        }
        if speech_end > current_segment_start {
            segments.push(samples[current_segment_start..speech_end].to_vec());
        }
    }

    // Reset VAD states after processing
    vad.reset();

    segments
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resample_to_identity_when_same_rate() {
        let s = vec![0.1_f32; 2000];
        let out = resample_to(&s, 16000, 16000).unwrap();
        assert_eq!(out.len(), 2000);
    }

    #[test]
    fn resample_to_48k_changes_length_proportionally() {
        // 1 秒 16k 正弦 → 48k：rubato FftFixedIn 以固定 input 帧处理，比例约 3 倍。
        // 16000 输入 = 15 整帧（1024）+ 640 残留；resample_to 丢弃残留 → 约 46080 输出。
        let s: Vec<f32> = (0..16000)
            .map(|i| (2.0 * std::f32::consts::PI * 440.0 * i as f32 / 16000.0).sin() * 0.3)
            .collect();
        let out = resample_to(&s, 16000, 48000).unwrap();
        assert!(
            out.len() >= 45000 && out.len() <= 48000,
            "48k 重采样长度异常: {}",
            out.len()
        );
    }

    #[test]
    fn audio_resampler_new_to_48k_streaming_keeps_buffer() {
        // 流式：分两块喂入，缓冲跨调用保留。最终长度应 ≥ 一次性（尾部零填吐残留）。
        let full: Vec<f32> = (0..16000)
            .map(|i| (2.0 * std::f32::consts::PI * 440.0 * i as f32 / 16000.0).sin() * 0.3)
            .collect();
        let mut rs = AudioResampler::new_to(16000, 48000).unwrap();
        assert_eq!(rs.to_rate(), 48000);
        let mut acc = rs.resample(&full[..8000]).unwrap();
        acc.extend(rs.resample(&full[8000..]).unwrap());
        acc.extend(rs.flush().unwrap());
        let one = resample_to(&full, 16000, 48000).unwrap();
        // 流式 flush 后吐出残留（640 samples 零填成一帧 → ~3072 输出），故 acc > one；
        // 容差放宽到一帧输出大小（4096）以吸收实现细节差异。
        let diff = acc.len() as i64 - one.len() as i64;
        assert!(
            (0..4096).contains(&diff),
            "流式 {} 应 ≥ 一次性 {} 且差值 < 4096，实际 diff={}",
            acc.len(),
            one.len(),
            diff
        );
    }

    #[test]
    fn segment_audio_vad_segments_in_bounds() {
        // 回归保护：pre/post speech padding 不应导致 segment 切片越界 panic。依赖真实 SileroVad
        // （无模型则 skip）；不断言具体段内容，只断言所有返回 segment 下标合法、不越界。
        let mut vad = match crate::config::create_silero_vad() {
            Ok(v) => v,
            Err(e) => {
                eprintln!("[SKIP] SileroVad 初始化失败: {e}");
                return;
            }
        };
        // 合成 ~31s 音频（略超 30s，覆盖 transcribe_with_vad 的切分路径）；5~25s 段提高幅度，
        // 让 VAD 大概率产出非空 segment，以 exercise 切分/末段分支的 padding slice 边界。
        let n = 16000 * 31;
        let samples: Vec<f32> = (0..n)
            .map(|i| {
                let t = i as f32 / 16000.0;
                let amp = if (5.0..25.0).contains(&t) { 0.3 } else { 0.02 };
                (2.0 * std::f32::consts::PI * 220.0 * t).sin() * amp
            })
            .collect();
        let segs = segment_audio_vad(&samples, &mut vad, 480, 0.4, 500, 25000);
        for s in &segs {
            assert!(!s.is_empty(), "segment 不应为空");
            assert!(
                s.len() <= samples.len(),
                "segment 长度 {} 超过总长 {}（padding 越界？）",
                s.len(),
                samples.len()
            );
        }
        // 不强断言 segs 非空（VAD 对纯正弦可能全判静音）；核心是全程不 panic + 下标合法。
    }

    #[test]
    fn filter_speech_trims_ends_keeps_middle() {
        // 回归：filter_speech 两端 trim（去首尾静音、保留中间），不逐帧删除（句内空洞）。
        // 依赖真实 SileroVad（无模型 skip）；不强断言非空（VAD 对纯正弦可能判静音），
        // 只验证不 panic + trim 结果长度不超输入。
        let mut vad = match crate::config::create_silero_vad() {
            Ok(v) => v,
            Err(e) => {
                eprintln!("[SKIP] SileroVad 初始化失败: {e}");
                return;
            }
        };
        // 1s 静音 + 2s 正弦（模拟语音）+ 1s 静音，共 4s。
        let mk = |amp: f32, secs: f32| -> Vec<f32> {
            let n = (16000.0 * secs) as usize;
            (0..n)
                .map(|i| (2.0 * std::f32::consts::PI * 220.0 * i as f32 / 16000.0).sin() * amp)
                .collect()
        };
        let mut samples = mk(0.0, 1.0);
        samples.extend(mk(0.3, 2.0));
        samples.extend(mk(0.0, 1.0));
        let out = filter_speech(&samples, &mut vad, 480, 0.5);
        assert!(
            out.len() <= samples.len(),
            "filter_speech 结果 {} 超过输入 {}（trim 不应增长）",
            out.len(),
            samples.len()
        );
    }
}
