use anyhow::Result;
use rubato::Resampler;

/// 从 WAV 文件读取并转为 16kHz mono f32 样本
pub fn read_wav_16k(path: &str) -> Result<Vec<f32>> {
    let mut reader = hound::WavReader::open(path)?;
    let spec = reader.spec();
    let sample_rate = spec.sample_rate;
    let samples: Vec<f32> = match spec.sample_format {
        hound::SampleFormat::Float => reader.samples::<f32>().collect::<Result<Vec<_>, _>>()?,
        hound::SampleFormat::Int => reader
            .samples::<i16>()
            .map(|s| s.unwrap() as f32 / i16::MAX as f32)
            .collect(),
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

/// 从 WAV 字节流读取并转为 16kHz mono f32 样本
pub fn read_wav_16k_from_bytes(data: &[u8]) -> Result<Vec<f32>> {
    let cursor = std::io::Cursor::new(data);
    let mut reader = hound::WavReader::new(cursor)?;
    let spec = reader.spec();
    let sample_rate = spec.sample_rate;
    let samples: Vec<f32> = match spec.sample_format {
        hound::SampleFormat::Float => reader.samples::<f32>().collect::<Result<Vec<_>, _>>()?,
        hound::SampleFormat::Int => reader
            .samples::<i16>()
            .map(|s| s.unwrap() as f32 / i16::MAX as f32)
            .collect(),
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
        self.buffer.extend(std::iter::repeat(0.0).take(needed));
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

/// Apply VAD filtering: returns only speech frames above threshold
pub fn filter_speech(
    samples: &[f32],
    vad: &mut crate::vad::SileroVad,
    frame_size: usize,
    threshold: f32,
) -> Vec<f32> {
    let mut speech = Vec::new();
    for chunk in samples.chunks(frame_size) {
        if chunk.len() < frame_size {
            break;
        }
        if let Ok(prob) = vad.compute(chunk) {
            if prob > threshold {
                speech.extend_from_slice(chunk);
            }
        }
    }
    speech
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
                current_segment_start = start_idx;
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
                let speech_end = (i + 1 - silence_frames_count) * frame_size;
                if speech_end > current_segment_start {
                    segments.push(samples[current_segment_start..speech_end].to_vec());
                }
                in_speech = false;
            }
            // Check for max duration split
            else if current_duration_ms >= max_segment_ms {
                let speech_end = if silence_frames_count > 0 {
                    (i + 1 - silence_frames_count) * frame_size
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
            speech_end = samples.len() - silence_frames_count * frame_size;
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
            diff >= 0 && diff < 4096,
            "流式 {} 应 ≥ 一次性 {} 且差值 < 4096，实际 diff={}",
            acc.len(),
            one.len(),
            diff
        );
    }
}
