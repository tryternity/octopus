use anyhow::Result;
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use log::{debug, info};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

/// Send-safe shared state between AudioRecorder and coordinator thread.
/// The cpal::Stream is NOT Send on macOS, so it stays on the creating thread;
/// only this shared handle crosses thread boundaries.
pub struct SharedAudioState {
    samples: Arc<Mutex<Vec<f32>>>,
    is_recording: Arc<AtomicBool>,
    sample_rate: std::sync::atomic::AtomicU32,
    device_name: String,
}

impl SharedAudioState {
    pub fn new(device_name: &str) -> Self {
        Self {
            samples: Arc::new(Mutex::new(Vec::new())),
            is_recording: Arc::new(AtomicBool::new(false)),
            sample_rate: std::sync::atomic::AtomicU32::new(16000),
            device_name: device_name.to_string(),
        }
    }

    /// Begin capturing: clear buffer, set recording flag
    pub fn start(&self) -> Result<()> {
        self.samples.lock().unwrap().clear();
        self.is_recording.store(true, Ordering::Relaxed);
        debug!("Recording started");
        Ok(())
    }

    /// Stop capturing, drain samples, resample to 16kHz
    pub fn stop(&self) -> Result<Vec<f32>> {
        self.is_recording.store(false, Ordering::Relaxed);
        let raw = std::mem::take(&mut *self.samples.lock().unwrap());
        debug!("Recording stopped, {} raw samples", raw.len());

        let rate = self.sample_rate.load(Ordering::Relaxed);
        if rate == 16000 {
            Ok(raw)
        } else {
            octopus_asr::audio::resample_to_16k(&raw, rate)
        }
    }

    pub fn device_name(&self) -> &str {
        &self.device_name
    }

    /// 获取当前采样率
    pub fn sample_rate(&self) -> u32 {
        self.sample_rate.load(Ordering::Relaxed)
    }

    /// 排空已累积的音频样本，重采样到 16kHz。
    /// 录音不中断，只清空缓冲区。用于流式识别时定期取走音频。
    pub fn drain_samples(&self) -> Vec<f32> {
        let raw = std::mem::take(&mut *self.samples.lock().unwrap());
        if raw.is_empty() {
            return Vec::new();
        }
        let rate = self.sample_rate.load(Ordering::Relaxed);
        if rate == 16000 {
            raw
        } else {
            octopus_asr::audio::resample_to_16k(&raw, rate).unwrap_or_default()
        }
    }
}

// Safety: SharedAudioState only contains Arc<Mutex<Vec<f32>>>, Arc<AtomicBool>,
// AtomicU32, and String — all Send + Sync.
unsafe impl Send for SharedAudioState {}
unsafe impl Sync for SharedAudioState {}

/// 麦克风录音管理器
/// Owns the cpal::Stream (which is NOT Send). Must live on the main thread
/// or the thread that created it. Only the SharedAudioState handle is shared.
pub struct AudioRecorder {
    state: Arc<SharedAudioState>,
    stream: Option<cpal::Stream>,
}

impl AudioRecorder {
    pub fn new(device_name: &str) -> Result<Self> {
        Ok(Self {
            state: Arc::new(SharedAudioState::new(device_name)),
            stream: None,
        })
    }

    /// Get a handle to the shared state (Send-safe) for use by other threads
    pub fn shared(&self) -> Arc<SharedAudioState> {
        self.state.clone()
    }

    /// 打开麦克风设备，准备录音
    pub fn open(&mut self) -> Result<()> {
        let host = cpal::default_host();
        let device = if self.state.device_name.is_empty() {
            host.default_input_device()
                .ok_or_else(|| anyhow::anyhow!("No default input device"))?
        } else {
            host.input_devices()?
                .find(|d| {
                    d.name()
                        .map(|n| n.contains(&self.state.device_name))
                        .unwrap_or(false)
                })
                .ok_or_else(|| anyhow::anyhow!("Device '{}' not found", self.state.device_name))?
        };

        let config = device.default_input_config()?;
        let rate = config.sample_rate().0;
        let channels = config.channels() as usize;
        self.state.sample_rate.store(rate, Ordering::Relaxed);

        info!(
            "Opened device: {}, rate: {}, channels: {}",
            device.name().unwrap_or_default(),
            rate,
            channels
        );

        let samples = self.state.samples.clone();
        let is_recording = self.state.is_recording.clone();

        let stream = match config.sample_format() {
            cpal::SampleFormat::F32 => device.build_input_stream(
                &config.into(),
                move |data: &[f32], _: &cpal::InputCallbackInfo| {
                    if is_recording.load(Ordering::Relaxed) {
                        let mono: Vec<f32> = data
                            .chunks(channels)
                            .map(|c| c.iter().sum::<f32>() / channels as f32)
                            .collect();
                        samples.lock().unwrap().extend_from_slice(&mono);
                    }
                },
                |err| debug!("Audio error: {}", err),
                None,
            )?,
            cpal::SampleFormat::I16 => device.build_input_stream(
                &config.into(),
                move |data: &[i16], _: &cpal::InputCallbackInfo| {
                    if is_recording.load(Ordering::Relaxed) {
                        let mono: Vec<f32> = data
                            .chunks(channels)
                            .map(|c| {
                                c.iter().map(|&s| s as f32 / i16::MAX as f32).sum::<f32>()
                                    / channels as f32
                            })
                            .collect();
                        samples.lock().unwrap().extend_from_slice(&mono);
                    }
                },
                |err| debug!("Audio error: {}", err),
                None,
            )?,
            fmt => anyhow::bail!("Unsupported sample format: {:?}", fmt),
        };

        self.stream = Some(stream);
        Ok(())
    }

    /// 开始录音
    pub fn start(&self) -> Result<()> {
        self.state.start()?;
        if let Some(stream) = &self.stream {
            stream.play()?;
        }
        Ok(())
    }

    /// 停止录音，返回 16kHz mono f32 样本
    pub fn stop(&self) -> Result<Vec<f32>> {
        if let Some(stream) = &self.stream {
            stream.pause()?;
        }
        self.state.stop()
    }

    /// 关闭设备
    pub fn close(&mut self) -> Result<()> {
        self.state.is_recording.store(false, Ordering::Relaxed);
        self.stream = None;
        self.state.samples.lock().unwrap().clear();
        Ok(())
    }
}
