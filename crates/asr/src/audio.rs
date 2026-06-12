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

/// Resample audio to 16kHz
pub fn resample_to_16k(samples: &[f32], from_rate: u32) -> Result<Vec<f32>> {
    if from_rate == 16000 {
        return Ok(samples.to_vec());
    }
    let resampler = rubato::FftFixedIn::<f32>::new(from_rate as usize, 16000, 1024, 2, 1)?;
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

/// Stateful resampler for streaming audio.
/// Caches the Rubato FftFixedIn planner and buffers leftover samples between chunks
/// to ensure glitch-free audio boundaries and high performance.
pub struct AudioResampler {
    resampler: rubato::FftFixedIn<f32>,
    input_frames: usize,
    buffer: Vec<f32>,
}

impl AudioResampler {
    pub fn new(from_rate: u32) -> Result<Self> {
        let resampler = rubato::FftFixedIn::<f32>::new(from_rate as usize, 16000, 1024, 2, 1)?;
        let input_frames = resampler.input_frames_next();
        Ok(Self {
            resampler,
            input_frames,
            buffer: Vec::new(),
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
}

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
