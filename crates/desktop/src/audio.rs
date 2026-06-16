use anyhow::Result;
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use log::{debug, info};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

/// 音频共享状态：采样缓冲 + 录制标志 + cpal 流（生命周期绑定到本结构）。
///
/// cpal::Stream 在 macOS 为 `!Send + !Sync`。本结构的 `Arc` 仅被 Coordinator 的
/// 单线程 mpsc 循环线程独占持有（main.rs → `Coordinator::new` → `std::thread::spawn`
/// 闭包 move；`audio` 不进 Coordinator 结构体字段），`start`/`stop`/`drain_samples`
/// 全在该线程调用：`start` 建流 + play，`stop` pause + 析构（take 出 Option 在本线程
/// drop），结构体本身也在该循环线程退出时析构——故 Stream 的建/播/停/析构全程同线程、
/// 无跨线程访问。cpal 回调线程只持有独立 clone 的 `Arc<Mutex<Vec>>` / `Arc<AtomicBool>`
/// （标准 Send+Sync），不经本结构。详见 architecture.md「音频采集按需启停」。
pub struct SharedAudioState {
    samples: Arc<Mutex<Vec<f32>>>,
    is_recording: Arc<AtomicBool>,
    sample_rate: std::sync::atomic::AtomicU32,
    device_name: String,
    /// 流式重采样器：denoise 路径=48k→16k；直通路径=原生→16k。会话内 lazy 重建。
    resampler: Mutex<Option<octopus_asr::audio::AudioResampler>>,
    /// 流式下采样器：原生→48k（仅 denoise 路径用，喂给 DenoiseProcessor）。
    down_sampler: Mutex<Option<octopus_asr::audio::AudioResampler>>,
    /// DeepFilterNet3 降噪器：start 时 lazy 建/重置；缺失/失败则 None（直通降级）。
    denoise: Mutex<Option<octopus_asr::denoise::DenoiseProcessor>>,
    stream: Mutex<Option<cpal::Stream>>,
}

impl SharedAudioState {
    pub fn new(device_name: &str) -> Self {
        Self {
            samples: Arc::new(Mutex::new(Vec::new())),
            is_recording: Arc::new(AtomicBool::new(false)),
            sample_rate: std::sync::atomic::AtomicU32::new(16000),
            device_name: device_name.to_string(),
            resampler: Mutex::new(None),
            down_sampler: Mutex::new(None),
            denoise: Mutex::new(None),
            stream: Mutex::new(None),
        }
    }

    /// 流式重采样一块音频：lazy 建采样器（from→to），resample，可选 flush。
    /// from==to 直通；采样器创建失败则降级返回原样（不阻断录音）。
    /// 注：本方法持有 field 锁仅限函数体内，用完即 drop，不与其它锁交织。
    fn stream_resample(
        &self,
        field: &Mutex<Option<octopus_asr::audio::AudioResampler>>,
        samples: &[f32],
        from_rate: u32,
        to_rate: u32,
        flush: bool,
    ) -> Vec<f32> {
        if from_rate == to_rate {
            return samples.to_vec();
        }
        let mut g = field.lock().unwrap();
        if g.is_none() {
            *g = octopus_asr::audio::AudioResampler::new_to(from_rate, to_rate).ok();
        }
        match g.as_mut() {
            Some(r) => {
                let mut out = r.resample(samples).unwrap_or_default();
                if flush {
                    out.extend(r.flush().unwrap_or_default());
                }
                out
            }
            None => {
                // 采样器创建失败（罕见）：降级直通，不阻断
                log::warn!(
                    "重采样器 {}→{} 创建失败，降级直通",
                    from_rate,
                    to_rate
                );
                samples.to_vec()
            }
        }
    }

    /// 音频前处理流水：可选 DeepFilterNet3 降噪 + 重采样到 16kHz。
    ///
    /// - flush=true（stop 会话结束）：denoise 与两个采样器都 flush 尾部残留。
    /// - flush=false（drain 流式）：三者都不 flush，GRU 状态与采样缓冲跨调用连续保持
    ///   （降噪物理连续性，呼应 spec §6「噪声估计跨段保持」，与 VAD 每段 reset 相反）。
    ///
    /// 降级（spec §9）：denoise_enabled=false / 模型缺失 / 实例未就绪 → 走直通（原生→16k），
    /// 仅 warn 日志，绝不 panic、绝不阻断录音。单帧推理失败已由 DenoiseProcessor 内部 bypass。
    ///
    /// 锁顺序：down_sampler（stream_resample 内 lock→用→drop）→ denoise（lock→用→drop
    /// 后才进下一阶段）→ resampler。`enhanced_48k` 的 `let mut g` 作用域在 if 块内，
    /// drop 在 match 前完成，无锁交织。
    fn process_pipeline(&self, raw: &[f32], rate: u32, flush: bool) -> Vec<f32> {
        let cfg = octopus_asr::config::load_app_config_cached();
        let denoise_on = cfg.denoise_enabled;

        // denoise 锁：仅本块持有，用完即 drop（不跨 stream_resample 持有，避免与采样器锁交织）
        let enhanced_48k: Option<Vec<f32>> = if denoise_on {
            let mut g = self.denoise.lock().unwrap();
            if let Some(denoise) = g.as_mut() {
                // 原生 → 48k（流式 down_sampler）
                let s48k = self.stream_resample(&self.down_sampler, raw, rate, 48000, flush);
                // 48k → DF3 → 48k enhanced
                let mut enhanced = denoise.process_samples(&s48k);
                if flush {
                    enhanced.extend(denoise.flush());
                }
                Some(enhanced)
            } else {
                None
            }
        } else {
            None
        };

        match enhanced_48k {
            Some(enhanced) => {
                // 48k enhanced → 16k（流式 resampler）
                self.stream_resample(&self.resampler, &enhanced, 48000, 16000, flush)
            }
            None => {
                // 直通：原生 → 16k（denoise 关闭或未就绪）
                self.stream_resample(&self.resampler, raw, rate, 16000, flush)
            }
        }
    }

    /// Build the input stream for the given device name
    fn build_stream(&self, device_name: &str) -> Result<cpal::Stream> {
        let host = cpal::default_host();
        let device = if device_name.is_empty() {
            host.default_input_device()
                .ok_or_else(|| anyhow::anyhow!("No default input device"))?
        } else {
            host.input_devices()?
                .find(|d| {
                    d.name()
                        .map(|n| n.contains(device_name))
                        .unwrap_or(false)
                })
                .ok_or_else(|| anyhow::anyhow!("Device '{}' not found", device_name))?
        };

        let config = device.default_input_config()?;
        let rate = config.sample_rate().0;
        let channels = config.channels() as usize;
        self.sample_rate.store(rate, Ordering::Relaxed);

        info!(
            "Opened device: {}, rate: {}, channels: {}",
            device.name().unwrap_or_default(),
            rate,
            channels
        );

        let samples = self.samples.clone();
        let is_recording = self.is_recording.clone();

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

        Ok(stream)
    }

    /// Begin capturing: clear buffer, set recording flag, build and play CPAL stream.
    /// 同时初始化 DF3 降噪（会话起点）：enabled 则建/重置实例，否则置 None。
    pub fn start(&self, device_name: &str) -> Result<()> {
        self.samples.lock().unwrap().clear();
        self.is_recording.store(true, Ordering::Relaxed);

        // 降噪初始化：enabled 则建/重置实例；失败降级为 None（直通），仅 warn + 下载提示，
        // 绝不阻断录音、绝不 panic（spec §9 降级）。
        let cfg = octopus_asr::config::load_app_config_cached();
        {
            let mut g = self.denoise.lock().unwrap();
            if cfg.denoise_enabled {
                match octopus_asr::config::find_df3()
                    .and_then(|p| octopus_asr::denoise::DenoiseProcessor::new(&p))
                {
                    Ok(mut p) => {
                        p.reset();
                        *g = Some(p);
                        info!("DF3 环境降噪已启用（48k STFT 链路）");
                    }
                    Err(e) => {
                        log::warn!(
                            "DF3 降噪初始化失败，已降级直通（不阻断录音）：{:?}",
                            e
                        );
                        *g = None;
                    }
                }
            } else {
                *g = None;
            }
        }
        // 流式采样器：新会话起点清空，首次 process_pipeline 时 lazy 重建
        *self.down_sampler.lock().unwrap() = None;
        *self.resampler.lock().unwrap() = None;

        let stream = self.build_stream(device_name)?;
        stream.play()?;

        *self.stream.lock().unwrap() = Some(stream);
        debug!("Recording started");
        Ok(())
    }

    /// Stop capturing, pause and drop CPAL stream, drain samples.
    /// 会话结束：process_pipeline 以 flush=true 取尾部残留（denoise + 两个采样器），
    /// 然后清空流式状态（下次 start lazy 重建）。
    pub fn stop(&self) -> Result<Vec<f32>> {
        self.is_recording.store(false, Ordering::Relaxed);

        let mut stream_guard = self.stream.lock().unwrap();
        if let Some(s) = stream_guard.take() {
            let _ = s.pause();
            debug!("CPAL stream paused and dropped");
        }
        drop(stream_guard);

        let raw = std::mem::take(&mut *self.samples.lock().unwrap());
        debug!("Recording stopped, {} raw samples", raw.len());

        let rate = self.sample_rate.load(Ordering::Relaxed);
        let resampled = if raw.is_empty() {
            Vec::new()
        } else {
            self.process_pipeline(&raw, rate, true)
        };
        // 会话结束：清空流式状态（下次 start lazy 重建）
        *self.resampler.lock().unwrap() = None;
        *self.down_sampler.lock().unwrap() = None;
        Ok(resampled)
    }

    #[allow(dead_code)] // 预留：外部访问设备名（当前仅内部用字段）
    pub fn device_name(&self) -> &str {
        &self.device_name
    }

    /// 获取当前采样率
    #[allow(dead_code)] // 预留：外部访问采样率（当前仅内部 load）
    pub fn sample_rate(&self) -> u32 {
        self.sample_rate.load(Ordering::Relaxed)
    }

    /// 排空已累积的音频样本，重采样（+ 可选降噪）到 16kHz。
    /// 录音不中断，只清空缓冲区。用于流式识别时定期取走音频。
    /// 流式语义：process_pipeline flush=false——denoise 的 GRU 状态与两个采样器缓冲
    /// 跨调用连续保持，不 flush 尾部（降噪物理连续性，spec §6）。
    pub fn drain_samples(&self) -> Vec<f32> {
        let raw = std::mem::take(&mut *self.samples.lock().unwrap());
        if raw.is_empty() {
            return Vec::new();
        }
        let rate = self.sample_rate.load(Ordering::Relaxed);
        self.process_pipeline(&raw, rate, false)
    }
}

// Safety: 见结构体文档注释。SharedAudioState 含 `Mutex<Option<cpal::Stream>>`（Stream
// 本身 !Send）与 `Mutex<Option<AudioResampler>>`，故非自动 Send/Sync。但本结构的 Arc
// 仅被 Coordinator 单线程循环线程独占持有（audio 被 move 进 std::thread::spawn 闭包），
// Stream 的建（start）/ 停（stop take+drop）/ 结构体析构（循环线程退出）全程同线程、
// 无跨线程访问；回调线程只持有独立 clone 的 Arc<Mutex<Vec>>/Arc<AtomicBool>。在此不变量
// 下 Send/Sync 成立。若将来跨多线程共享本 Arc，须改用单一宿主线程 + channel 收敛 Stream。
unsafe impl Send for SharedAudioState {}
unsafe impl Sync for SharedAudioState {}

// 编译期断言：DenoiseProcessor 必须 Send + Sync（SharedAudioState 经 unsafe impl 跨线程）。
// 其字段 `Mutex<Option<DenoiseProcessor>>` 要求 DenoiseProcessor: Send。该断言在编译期固化
// 此前提——若 denoise.rs 重构引入非 Send 字段（如 Rc），此处编译失败，避免 unsafe impl
// 静默退化为未定义行为。与 asr/audio.rs 的 AudioResampler 断言同风格。
const _: () = {
    fn _assert_send_sync<T: Send + Sync>() {}
    fn _assert() {
        _assert_send_sync::<octopus_asr::denoise::DenoiseProcessor>();
    }
};

/// 麦克风录音管理器
/// Owns the cpal::Stream inside SharedAudioState.
pub struct AudioRecorder {
    state: Arc<SharedAudioState>,
}

impl AudioRecorder {
    pub fn new(device_name: &str) -> Result<Self> {
        Ok(Self {
            state: Arc::new(SharedAudioState::new(device_name)),
        })
    }

    /// Get a handle to the shared state (Send-safe) for use by other threads
    pub fn shared(&self) -> Arc<SharedAudioState> {
        self.state.clone()
    }

    /// 打开麦克风设备，验证麦克风是否存在
    pub fn open(&mut self) -> Result<()> {
        let host = cpal::default_host();
        let _device = if self.state.device_name.is_empty() {
            host.default_input_device()
        } else {
            host.input_devices()?.find(|d| {
                d.name()
                    .map(|n| n.contains(&self.state.device_name))
                    .unwrap_or(false)
            })
        };
        if _device.is_none() {
            anyhow::bail!("Microphone device '{}' not found", self.state.device_name);
        }
        Ok(())
    }
}
