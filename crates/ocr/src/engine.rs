use anyhow::{Context, Result};
use parking_lot::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, OnceLock};

use crate::model;

/// OCR 引擎——封装 octopus-paddle-ocr（基于 ONNX Runtime 的 PaddleOCR）。
///
/// RapidOcr::run 需要 &mut self，OcrEngine 包在 Arc 里共享。
/// 用 Mutex 保护内部可变性——OCR 全局互斥（OcrLockGuard）保证同一时刻
/// 只有一个调用方，Mutex 不会产生实际竞争。
pub struct OcrEngine {
    /// None=已 idle 释放，下次 run_ocr 自动重载。
    inner: Mutex<Option<octopus_paddle_ocr::RapidOcr>>,
    /// 最近一次 run_ocr 入口时间戳，守护线程据此判 idle。
    last_used: Mutex<Option<std::time::Instant>>,
    /// 当前加载的模型名，守护线程释放时拼 probe id 用。
    model_name: String,
    use_word_segmentation: bool,
}

#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct OcrBlock {
    pub text: String,
    pub x: f64,
    pub y: f64,
    pub w: f64,
    pub h: f64,
    pub score: f64,
}

static INSTANCE: OnceLock<Arc<OcrEngine>> = OnceLock::new();
static INIT_LOCK: Mutex<()> = Mutex::new(());

const SPLIT_HEIGHT_THRESHOLD: u32 = 1600;
const CHUNK_HEIGHT: u32 = 1280;
const CHUNK_OVERLAP: u32 = 200;

/// OCR idle 多久后释放模型内存（drop ort session + mmap 权重）。ASR/VAD 不在此机制范围。
const OCR_IDLE_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(60);
/// 守护线程采样间隔（≤ OCR_IDLE_TIMEOUT 的一半，保证及时释放又不频繁检查）。
const OCR_DAEMON_TICK: std::time::Duration = std::time::Duration::from_secs(30);

impl OcrEngine {
    pub fn instance() -> Result<Arc<OcrEngine>> {
        if let Some(e) = INSTANCE.get() {
            return Ok(e.clone());
        }
        let _guard = INIT_LOCK.lock();
        if let Some(e) = INSTANCE.get() {
            return Ok(e.clone());
        }

        let model_name = octopus_infra::db::load_config_key("ocr_model")
            .ok()
            .flatten()
            .unwrap_or_else(|| model::DEFAULT_OCR_MODEL.to_string());

        if !model::is_model_ready(&model_name) {
            anyhow::bail!(
                "OCR 模型未就绪: {}（请检查 ~/.octopus/models/ocr/{}/）",
                model_name,
                model_name
            );
        }

        octopus_infra::model_probe::probe(
            octopus_infra::model_probe::LoadPhase::Before,
            &format!("ocr:{model_name}"),
        );
        let inner = Self::load_rapid_ocr(&model_name)?;

        // v6 的 CTC space token 被正确激活，输出自带英文空格，不需要后处理分词。
        // v5 及更早版本需要 words_alpha 词库做贪心分词。
        let use_word_segmentation = !model_name.starts_with("PP-OCRv6");

        log::info!("[ocr-engine] RapidOcr loaded — model={}, word_segmentation={}", model_name, use_word_segmentation);

        let engine = Arc::new(OcrEngine {
            inner: Mutex::new(Some(inner)),
            last_used: Mutex::new(Some(std::time::Instant::now())),
            model_name: model_name.clone(),
            use_word_segmentation,
        });
        octopus_infra::model_probe::probe(
            octopus_infra::model_probe::LoadPhase::After,
            &format!("ocr:{model_name}"),
        );
        // set 成功（首次）才 spawn 守护线程，保证全局唯一。
        if INSTANCE.set(engine.clone()).is_ok() {
            Self::spawn_idle_daemon(engine.clone());
        }
        Ok(engine)
    }

    /// 构建 RapidOcr 实例（不含 probe，纯加载）。
    /// instance() 首次加载与 run_ocr idle 后重载共用——重载【不】调 probe
    /// （避免刷新 registry 首次估算值；模型内存估算只记首次，record_once 不覆盖）。
    fn load_rapid_ocr(model_name: &str) -> Result<octopus_paddle_ocr::RapidOcr> {
        let dir = crate::model::model_dir(model_name);
        log::info!("Loading OCR model: {} from {}", model_name, dir.display());
        let config = build_engine_config(&dir)?;
        octopus_paddle_ocr::RapidOcr::new(config)
            .map_err(|e| anyhow::anyhow!("Failed to init RapidOcr: {e}"))
    }

    /// 起 idle 监控守护线程（全局唯一，只在 instance 首次 set 成功后 spawn 一次）。
    /// 用 std::thread 而非 tokio——ocr crate 被 cli/server/desktop 共享，不能假设有 runtime。
    fn spawn_idle_daemon(engine: Arc<OcrEngine>) {
        std::thread::spawn(move || loop {
            std::thread::sleep(OCR_DAEMON_TICK);
            // 守护线程采样失败不影响主进程——catch_unwind 兜住任何 panic。
            let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                engine.check_and_release_if_idle();
            }));
        });
    }

    /// 守护线程调用：若距上次使用超过 OCR_IDLE_TIMEOUT，drop 内部 RapidOcr（释放模型内存）
    /// 并 probe(Unload) 通知状态页清条目。下次 run_ocr 自动重载。
    fn check_and_release_if_idle(&self) {
        if !Self::is_idle(&self.last_used.lock()) {
            return;
        }
        let mut inner = self.inner.lock();
        if inner.is_some() {
            *inner = None; // drop RapidOcr → 释放 ort session + mmap 权重
            drop(inner);   // 先释放 inner 锁，再调 probe 闭包（不持锁调外部代码）
            let id = format!("ocr:{}", self.model_name);
            octopus_infra::model_probe::probe(
                octopus_infra::model_probe::LoadPhase::Unload,
                &id,
            );
            log::info!(
                "[ocr-engine] OCR idle {}s, released model {}",
                OCR_IDLE_TIMEOUT.as_secs(),
                self.model_name
            );
        }
    }

    /// 纯函数：last_used 距今是否超过 OCR_IDLE_TIMEOUT。抽出来便于单测（无需真实模型）。
    fn is_idle(last_used: &Option<std::time::Instant>) -> bool {
        match last_used {
            Some(t) => std::time::Instant::now().duration_since(*t) > OCR_IDLE_TIMEOUT,
            None => false, // 从未使用（理论不会，instance 即标 now）；不释放
        }
    }

    pub fn recognize(&self, image_bytes: &[u8]) -> Result<String> {
        let img = ::image::load_from_memory(image_bytes)
            .context("Failed to decode image")?;
        let lines = if img.height() > SPLIT_HEIGHT_THRESHOLD {
            self.recognize_long_image(&img)?
        } else {
            self.recognize_image(&img)?
        };
        Ok(lines.join("\n"))
    }

    pub fn recognize_with_blocks(&self, image_bytes: &[u8]) -> Result<(String, Vec<OcrBlock>)> {
        let img = ::image::load_from_memory(image_bytes)
            .context("Failed to decode image")?;
        let blocks = if img.height() > SPLIT_HEIGHT_THRESHOLD {
            self.recognize_long_image_with_blocks(&img)?
        } else {
            self.recognize_image_with_blocks(&img)?
        };
        let text = blocks.iter().map(|b| b.text.as_str()).collect::<Vec<_>>().join("\n");
        Ok((text, blocks))
    }

    fn run_ocr(&self, img: &::image::DynamicImage) -> Result<Vec<OcrBlock>> {
        // 入口先刷新 last_used：防止守护线程在重载期间误判 idle。
        *self.last_used.lock() = Some(std::time::Instant::now());

        let rec_img = dynamic_to_rec_image(img)?;
        let opts = octopus_paddle_ocr::OcrCallOptions::default();

        // 重载 + run 在同一 inner lock 作用域：重载期间持有 inner 锁，守护线程
        // 无法在「重载后、run 前」窗口抢锁释放刚重载的模型（否则 run 时 inner=None → expect panic）。
        // OcrLockGuard 已保证同时只有一个 run_ocr，持锁重载数秒不阻塞其他 OCR 调用；
        // 守护线程仅在 sleep loop 里偶尔多等几秒，无害。
        // 重载【不】调 probe Before/After（避免刷新 registry 首次估算值，record_once 不覆盖）。
        let result = {
            let mut guard = self.inner.lock();
            if guard.is_none() {
                log::info!("[ocr-engine] OCR model {} reloaded after idle release", self.model_name);
                let new_engine = Self::load_rapid_ocr(&self.model_name)?;
                *guard = Some(new_engine);
            }
            let engine_ref = guard.as_mut().expect("inner just reloaded or was Some");
            engine_ref.run(rec_img, opts)
                .map_err(|e| anyhow::anyhow!("OCR run failed: {e}"))?
        }; // guard 在此 drop，释放 inner 锁

        let mut blocks = ocr_output_to_blocks(&result);
        blocks = merge_same_line_blocks(blocks);
        if self.use_word_segmentation {
            for b in &mut blocks {
                b.text = segment_english_words(&b.text);
            }
        }
        Ok(blocks)
    }

    fn recognize_image_with_blocks(&self, img: &::image::DynamicImage) -> Result<Vec<OcrBlock>> {
        self.run_ocr(img)
    }

    fn recognize_long_image_with_blocks(&self, img: &::image::DynamicImage) -> Result<Vec<OcrBlock>> {
        let (w, h) = (img.width(), img.height());
        let mut all_blocks: Vec<OcrBlock> = Vec::new();
        // 相邻 chunk 有 CHUNK_OVERLAP 高度的重叠区，重叠区的行会被两块都识别到。
        // 用「已收录到的最大绝对 y 底部」做坐标去重：下一块中 y 中心 ≤ 该值的行
        // 落在重叠区、已被前一块收录，丢弃。此前按文本逐字相等去重，OCR 轻微波动
        // （"hello" vs "hello!"）即致去重失败 → 重复行；也易误删天然重复行。
        let mut covered_until_y: f64 = 0.0;
        for (top, chunk_h) in Self::plan_chunks(h) {
            let sub = ::image::imageops::crop_imm(img, 0, top, w, chunk_h);
            let chunk = ::image::DynamicImage::from(sub.to_image());
            let mut blocks = self.run_ocr(&chunk)?;
            for b in &mut blocks { b.y += top as f64; }
            drop_overlapped_blocks(covered_until_y, &mut blocks);
            // 取当前 chunk 剩余 block 的最大底边（fold max），而非 blocks.last().y+h：
            // det 框按 y 中心升序排序，末尾 block 中心最大但底边不一定最大——
            // 极端混排（贯穿性大图框 + 底部矮行）下 last() 会少记 covered_until_y，
            // 致下一 chunk 重叠区行逃过去重 → 重复行。fold 从 covered_until_y 起步
            // 保证单调不减。正常单/双栏纯文本行高一致时与 last() 等价。
            covered_until_y = blocks.iter()
                .map(|b| b.y + b.h)
                .fold(covered_until_y, f64::max);
            all_blocks.extend(blocks);
        }
        Ok(all_blocks)
    }

    fn recognize_image(&self, img: &::image::DynamicImage) -> Result<Vec<String>> {
        let blocks = self.run_ocr(img)?;
        Ok(blocks.into_iter().map(|b| b.text).collect())
    }

    fn recognize_long_image(&self, img: &::image::DynamicImage) -> Result<Vec<String>> {
        // 复用 with_blocks 的坐标去重逻辑——纯文本版没有坐标，无法独立去重。
        let blocks = self.recognize_long_image_with_blocks(img)?;
        Ok(blocks.into_iter().map(|b| b.text).collect())
    }

    fn plan_chunks(h: u32) -> Vec<(u32, u32)> {
        if h <= SPLIT_HEIGHT_THRESHOLD { return Vec::new(); }
        let step = CHUNK_HEIGHT - CHUNK_OVERLAP;
        let mut plan = Vec::new();
        let mut top = 0u32;
        while top < h {
            let chunk_h = std::cmp::min(CHUNK_HEIGHT, h - top);
            plan.push((top, chunk_h));
            if chunk_h < CHUNK_HEIGHT { break; }
            top += step;
        }
        plan
    }
}

fn build_engine_config(dir: &std::path::Path) -> Result<octopus_paddle_ocr::EngineConfig> {
    use octopus_paddle_ocr::*;

    let det_path = dir.join("det.onnx");
    let rec_path = dir.join("rec.onnx");
    let keys_path = dir.join("keys.txt");
    let cls_path = dir.join("cls.onnx");

    let mut config = EngineConfig::default();

    config.det.model_path = Some(det_path);
    config.det.allow_download = false;

    config.rec.model.model_path = Some(rec_path);
    config.rec.model.rec_keys_path = Some(keys_path);
    config.rec.model.allow_download = false;

    if cls_path.exists() {
        config.cls.model_path = Some(cls_path);
        config.cls.allow_download = false;
    } else {
        config.global.use_cls = false;
    }

    Ok(config)
}

fn dynamic_to_rec_image(img: &::image::DynamicImage) -> Result<octopus_paddle_ocr::RecImage> {
    let rgb = img.to_rgb8();
    let (w, h) = rgb.dimensions();
    let raw = rgb.into_raw();
    let mut bgr = vec![0u8; raw.len()];
    for (src, dst) in raw.chunks_exact(3).zip(bgr.chunks_exact_mut(3)) {
        dst[0] = src[2];
        dst[1] = src[1];
        dst[2] = src[0];
    }
    octopus_paddle_ocr::RecImage::from_bgr_u8(w as usize, h as usize, bgr)
        .map_err(|e| anyhow::anyhow!("Failed to create RecImage: {e}"))
}

fn ocr_output_to_blocks(output: &octopus_paddle_ocr::OcrOutput) -> Vec<OcrBlock> {
    let boxes = output.boxes.as_deref().unwrap_or(&[]);
    let txts = output.txts.as_deref().unwrap_or(&[]);
    let scores = output.scores.as_deref().unwrap_or(&[]);

    boxes.iter().enumerate().map(|(i, quad)| {
        let xs: Vec<f32> = quad.iter().map(|p| p[0]).collect();
        let ys: Vec<f32> = quad.iter().map(|p| p[1]).collect();
        let x0 = xs.iter().copied().fold(f32::INFINITY, f32::min) as f64;
        let y0 = ys.iter().copied().fold(f32::INFINITY, f32::min) as f64;
        let x1 = xs.iter().copied().fold(f32::NEG_INFINITY, f32::max) as f64;
        let y1 = ys.iter().copied().fold(f32::NEG_INFINITY, f32::max) as f64;
        OcrBlock {
            text: txts.get(i).cloned().unwrap_or_default(),
            x: x0,
            y: y0,
            w: x1 - x0,
            h: y1 - y0,
            score: scores.get(i).copied().unwrap_or(0.0) as f64,
        }
    }).collect()
}

/// 合并同一视觉行的文本块：相邻块 y 中心距离 < 行高一半 → 同行合并。
/// det 检测器常把同一行文字拆成多个独立框（尤其中英混排），导致输出多余换行。
/// 保持 det 原始输出顺序（从上到下、从左到右），仅在相邻块同行时合并。
fn merge_same_line_blocks(blocks: Vec<OcrBlock>) -> Vec<OcrBlock> {
    if blocks.len() <= 1 {
        return blocks;
    }

    let mut merged: Vec<OcrBlock> = Vec::with_capacity(blocks.len());
    for block in blocks {
        let last = merged.last_mut();
        if let Some(last) = last {
            let last_cy = last.y + last.h / 2.0;
            let block_cy = block.y + block.h / 2.0;
            let avg_h = (last.h + block.h) / 2.0;
            if (last_cy - block_cy).abs() < avg_h * 0.5 {
                // 两个块之间的水平间隙 = block 左边 − last 右边。
                // 间隙大于平均字宽（≈avg_h）的 0.3 倍时插入空格——
                // 中英混排紧邻（如「你好世界Hello」）不插；
                // 间距明显的（如「Hello   World」）补空格。
                let gap = block.x - (last.x + last.w);
                if gap > avg_h * 0.3 {
                    last.text.push(' ');
                }
                last.text.push_str(&block.text);
                let x1 = (last.x + last.w).max(block.x + block.w);
                let y0 = last.y.min(block.y);
                let y1 = (last.y + last.h).max(block.y + block.h);
                last.x = last.x.min(block.x);
                last.w = x1 - last.x;
                last.y = y0;
                last.h = y1 - y0;
                last.score = last.score.max(block.score);
                continue;
            }
        }
        merged.push(block);
    }
    merged
}

/// 丢弃 y 中心 ≤ covered_until_y 的行——这些行落在与前一块的 CHUNK_OVERLAP 重叠区、
/// 已被前一块收录。按坐标（而非文本）去重：相邻 chunk 在重叠区会重复识别同一物理行，
/// 而文本可能因 OCR 波动（"hello" vs "hello!"）不严格相等。纯函数，便于单测。
fn drop_overlapped_blocks(covered_until_y: f64, blocks: &mut Vec<OcrBlock>) {
    blocks.retain(|b| b.y + b.h / 2.0 > covered_until_y);
}

/// PP-OCR 中文 rec 模型不输出英文单词间的空格（CTC space token 未被激活）。
/// 此函数对文本中的连续 ASCII 字母段做贪心最长匹配分词，在单词间补空格。
/// 非ASCII（中文等）、标点、已有空格保持不变。
fn segment_english_words(text: &str) -> String {
    /// 编译期内嵌 37 万英文词表（words_common.txt，~4MB → 二进制内）。
    const WORDS_RAW: &str = include_str!("../assets/words_common.txt");

    use std::collections::HashSet;
    static WORD_SET: std::sync::OnceLock<HashSet<&'static str>> = std::sync::OnceLock::new();
    let ws = WORD_SET.get_or_init(|| WORDS_RAW.lines().map(|l| l.trim()).filter(|l| !l.is_empty()).collect());

    let chars: Vec<char> = text.chars().collect();
    let mut result = String::with_capacity(text.len() + 16);
    let mut i = 0;

    while i < chars.len() {
        let c = chars[i];

        // 非 ASCII 字母 → 原样输出
        if !c.is_ascii_alphabetic() {
            result.push(c);
            i += 1;
            continue;
        }

        // 收集连续 ASCII 字母段 [i..end)
        let mut end = i;
        while end < chars.len() && chars[end].is_ascii_alphabetic() {
            end += 1;
        }

        // 对字母段做贪心最长匹配分词
        let segment: String = chars[i..end].iter().collect();
        let lower = segment.to_lowercase();
        let lower_bytes = lower.as_bytes();
        let seg_len = lower_bytes.len();

        // 1. 整段在词表中 → 直接输出，不拆
        if ws.contains(lower.as_str()) {
            result.push_str(&segment);
            i = end;
            continue;
        }
        // 2. 短段（≤6 字母）不拆分
        if seg_len <= 6 {
            result.push_str(&segment);
            i = end;
            continue;
        }

        // 3. 贪心最长匹配
        let mut pos = 0usize;
        let mut words: Vec<String> = Vec::new();

        while pos < seg_len {
            let mut found = false;
            let max_len = (seg_len - pos).min(20);
            for len in (3..=max_len).rev() {
                let sub = std::str::from_utf8(&lower_bytes[pos..pos + len]).unwrap();
                if ws.contains(sub) {
                    words.push(segment[pos..pos + len].to_string());
                    pos += len;
                    found = true;
                    break;
                }
            }
            if !found {
                // 无匹配：取一个字符
                words.push(segment[pos..pos + 1].to_string());
                pos += 1;
            }
        }

        // 4. 合并尾部 ≤2 字母的碎片到前一个词
        if words.len() >= 2 {
            let last_w = words.last().unwrap();
            if last_w.len() <= 2 {
                let merged = format!("{}{}", words[words.len() - 2], last_w);
                words.truncate(words.len() - 2);
                words.push(merged);
            }
        }

        // 5. 用空格连接输出
        for (idx, w) in words.iter().enumerate() {
            if idx > 0 {
                result.push(' ');
            }
            result.push_str(w);
        }

        i = end;
    }

    result
}

static OCR_BUSY: AtomicBool = AtomicBool::new(false);

pub struct OcrLockGuard(());

impl OcrLockGuard {
    pub fn try_acquire() -> Option<Self> {
        OCR_BUSY
            .compare_exchange(false, true, Ordering::Acquire, Ordering::Acquire)
            .ok()
            .map(|_| OcrLockGuard(()))
    }
}

impl Drop for OcrLockGuard {
    fn drop(&mut self) {
        OCR_BUSY.store(false, Ordering::Release);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plan_chunks_normal_height_no_split() {
        assert!(OcrEngine::plan_chunks(1600).is_empty());
    }

    #[test]
    fn plan_chunks_covers_full_height() {
        for &h in &[2000u32, 3000, 5000] {
            let plan = OcrEngine::plan_chunks(h);
            assert!(!plan.is_empty());
            assert_eq!(plan[0].0, 0);
            let last = plan.last().unwrap();
            assert_eq!(last.0 + last.1, h);
        }
    }

    #[test]
    fn drop_overlapped_blocks_removes_rows_at_or_below_covered_bottom() {
        // covered_until_y=100：y 中心 ≤100 的行丢弃，>100 的保留
        let mut blocks = vec![
            OcrBlock { text: "A".into(), x: 0.0, y: 80.0,  w: 10.0, h: 20.0, score: 0.9 }, // 中心 90 ≤100 → 丢
            OcrBlock { text: "B".into(), x: 0.0, y: 95.0,  w: 10.0, h: 10.0, score: 0.9 }, // 中心 100 ≤100 → 丢
            OcrBlock { text: "C".into(), x: 0.0, y: 96.0,  w: 10.0, h: 10.0, score: 0.9 }, // 中心 101 >100 → 留
            OcrBlock { text: "D".into(), x: 0.0, y: 200.0, w: 10.0, h: 20.0, score: 0.9 }, // 中心 210 → 留
        ];
        drop_overlapped_blocks(100.0, &mut blocks);
        let texts: Vec<String> = blocks.into_iter().map(|b| b.text).collect();
        assert_eq!(texts, vec!["C".to_string(), "D".to_string()]);
    }

    #[test]
    fn drop_overlapped_blocks_zero_coverage_keeps_positive_center() {
        // 首块 covered_until_y=0：只要 y 中心 >0（正常行）全保留
        let mut blocks = vec![
            OcrBlock { text: "A".into(), x: 0.0, y: 5.0, w: 10.0, h: 10.0, score: 0.9 },
        ];
        drop_overlapped_blocks(0.0, &mut blocks);
        assert_eq!(blocks.len(), 1);
    }

    #[test]
    fn is_idle_false_when_recently_used() {
        assert!(!OcrEngine::is_idle(&Some(std::time::Instant::now())));
    }

    #[test]
    fn is_idle_true_when_beyond_timeout() {
        let old = std::time::Instant::now() - std::time::Duration::from_secs(61);
        assert!(OcrEngine::is_idle(&Some(old)));
    }

    #[test]
    fn is_idle_false_just_under_timeout() {
        let recent = std::time::Instant::now() - std::time::Duration::from_secs(59);
        assert!(!OcrEngine::is_idle(&Some(recent)));
    }

    #[test]
    fn is_idle_false_when_none() {
        assert!(!OcrEngine::is_idle(&None));
    }
}
