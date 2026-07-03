use anyhow::{Context, Result};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, OnceLock};

use crate::model;

pub struct OcrEngine {
    inner: ocr_rs::engine::OcrEngine,
}

static INSTANCE: OnceLock<Arc<OcrEngine>> = OnceLock::new();
/// 串行化首次加载（double-checked locking）：保证 MNN 模型只加载一次，省重复加载。
static INIT_LOCK: Mutex<()> = Mutex::new(());

/// 超长图按高度切分阈值（px）。高于此值则切块分别识别。
/// 取 det `max_side_len=960` 的 ~1.7 倍——正常截图不触发切分走原路径，
/// 仅真正的长图（长截图等）走切分，避免整图等比缩放到 960 致短边过小、det 检测不到文本。
const SPLIT_HEIGHT_THRESHOLD: u32 = 1600;
/// 每块目标高度（略大于 det max_side_len，每块 det 时不被压太多）。
const CHUNK_HEIGHT: u32 = 1280;
/// 相邻块重叠（防文字行在块边界被切断；约数行高度）。
const CHUNK_OVERLAP: u32 = 200;

impl OcrEngine {
    /// 全局单例，首次调用时懒加载。
    /// model_name 从 app_config.ocr_model 读取，默认 PP-OCRv6-small。
    ///
    /// 全局单例，首次调用时懒加载（model_name 从 app_config.ocr_model 读，默认 PP-OCRv6-small）。
    ///
    /// double-checked locking 串行化首次加载、保证模型只加载一次。check-then-set
    /// （`get`→手动 `new`→`set`）下两个线程并发首次会各自走完整个 `OcrEngine::new`，
    /// 其中一个被 `OnceLock::set` 丢弃——浪费一次 ~180ms 加载 + 内存；DCL 消除之。
    ///
    /// 注：曾怀疑「并发首次加载致 MNN C++ 死锁」是 OCR 僵死根因，已被
    /// `tests/ocr_concurrent_smoke.rs` **证伪**（4 线程经 Barrier 同时首次
    /// `OcrEngine::new`，全 ok ~180ms 返回）。DCL 在此保留为无害的串行化优化，
    /// **非**「修复并发死锁」。
    pub fn instance() -> Result<Arc<OcrEngine>> {
        // 快路径：已加载，直接返回 clone（不取锁）。
        if let Some(e) = INSTANCE.get() {
            return Ok(e.clone());
        }
        // 慢路径：取锁串行化。
        let _guard = INIT_LOCK.lock().unwrap();
        // double-check：拿到锁前可能已被其他线程加载完。
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

        let dir = model::model_dir(&model_name);
        let det_path = dir.join("det.mnn");
        let rec_path = dir.join("rec.mnn");
        let keys_path = dir.join("keys.txt");

        log::info!("Loading OCR model: {} from {}", model_name, dir.display());
        log::info!(
            "[ocr-engine] before ocr_rs::OcrEngine::new thread={:?}",
            std::thread::current().id()
        );

        let inner = ocr_rs::engine::OcrEngine::new(&det_path, &rec_path, &keys_path, None)
            .map_err(|e| anyhow::anyhow!("Failed to init ocr_rs::OcrEngine: {:?}", e))?;

        log::info!("[ocr-engine] after ocr_rs::OcrEngine::new — MNN 模型加载完成");

        let engine = Arc::new(OcrEngine { inner });
        let _ = INSTANCE.set(engine.clone());
        Ok(engine)
    }

    /// 识别图片字节，返回识别文本（多行用 \n 连接）。
    /// 支持 WebP / PNG 等常见格式（image crate 自动检测）。
    pub fn recognize(&self, image_bytes: &[u8]) -> Result<String> {
        let img = ::image::load_from_memory(image_bytes)
            .context("Failed to decode image")?;
        let lines = if img.height() > SPLIT_HEIGHT_THRESHOLD {
            log::info!(
                "[ocr-engine] long image {}x{} → 切分识别（块高 {} 重叠 {}）",
                img.width(),
                img.height(),
                CHUNK_HEIGHT,
                CHUNK_OVERLAP
            );
            self.recognize_long_image(&img)?
        } else {
            self.recognize_image(&img)?
        };
        Ok(lines.join("\n"))
    }

    /// 单图识别，返回文本行列表（保持 ocr_rs 的逐行从上到下顺序）。
    fn recognize_image(&self, img: &::image::DynamicImage) -> Result<Vec<String>> {
        let results = self
            .inner
            .recognize(img)
            .map_err(|e| anyhow::anyhow!("OCR recognize failed: {:?}", e))?;
        Ok(results.into_iter().map(|r| r.text).collect())
    }

    /// 超长图按高度切分（带重叠）逐块识别，合并文本行。
    /// 重叠区可能让同一行在相邻块重复识别 → 用「跳过与上一块末行相同的起始连续行」去重。
    fn recognize_long_image(&self, img: &::image::DynamicImage) -> Result<Vec<String>> {
        let (w, h) = (img.width(), img.height());
        let mut all_lines: Vec<String> = Vec::new();
        for (idx, &(top, chunk_h)) in Self::plan_chunks(h).iter().enumerate() {
            // crop_imm 只复制块区域像素，不 clone 全图。
            let sub = ::image::imageops::crop_imm(img, 0, top, w, chunk_h);
            let chunk = ::image::DynamicImage::from(sub.to_image());
            let lines = self.recognize_image(&chunk)?;
            log::info!(
                "[ocr-engine] chunk#{} top={} h={} → {} lines",
                idx,
                top,
                chunk_h,
                lines.len()
            );
            // 去重：跳过本块起始处与上一块末行完全相同的连续行（重叠区重复识别）。
            let skip = if idx > 0 && !all_lines.is_empty() {
                let last = all_lines.last().unwrap();
                lines.iter().position(|l| l != last).unwrap_or(lines.len())
            } else {
                0
            };
            all_lines.extend(lines.into_iter().skip(skip));
        }
        Ok(all_lines)
    }

    /// 纯函数：把总高 h 划分为若干块 (top, chunk_h)。h ≤ 阈值返回空（不切分）。
    /// 步长 = 块高 − 重叠；末块不足整块时补齐到 h 并结束。
    fn plan_chunks(h: u32) -> Vec<(u32, u32)> {
        if h <= SPLIT_HEIGHT_THRESHOLD {
            return Vec::new();
        }
        let step = CHUNK_HEIGHT - CHUNK_OVERLAP;
        let mut plan = Vec::new();
        let mut top = 0u32;
        while top < h {
            let chunk_h = std::cmp::min(CHUNK_HEIGHT, h - top);
            plan.push((top, chunk_h));
            if chunk_h < CHUNK_HEIGHT {
                break;
            }
            top += step;
        }
        plan
    }
}

/// OCR 全局互斥：同一时刻仅允许一个 OCR 任务。
/// 任一 OCR 入口（ocr_image / ocr_screenshot）须先 `try_acquire`，忙则报
/// "正在 OCR 中，请稍后重试"；guard drop（含 async future 被 cancel）时自动释放。
static OCR_BUSY: AtomicBool = AtomicBool::new(false);

/// OCR 互斥 guard：drop 释放 busy。`try_acquire` 忙时返回 None。
pub struct OcrLockGuard(());

impl OcrLockGuard {
    /// 占住全局 OCR 锁；已忙返回 None（调用方应报"正在 OCR 中，请稍后重试"）。
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
        // ≤ 阈值不切分。
        assert!(OcrEngine::plan_chunks(1600).is_empty());
        assert!(OcrEngine::plan_chunks(1000).is_empty());
    }

    #[test]
    fn plan_chunks_covers_full_height() {
        // 每块拼接必须正好覆盖 [0, h)（首块 top=0，末块结束=h），无遗漏无越界。
        for &h in &[2000u32, 3000, 5000, 9999, 12000] {
            let plan = OcrEngine::plan_chunks(h);
            assert!(!plan.is_empty(), "h={} 应切分", h);
            assert_eq!(plan[0].0, 0, "h={} 首块 top 应为 0", h);
            let last = plan.last().unwrap();
            assert_eq!(last.0 + last.1, h, "h={} 末块应正好结束于 h", h);
        }
    }

    #[test]
    fn plan_chunks_step_and_size() {
        // 步长 = 块高 − 重叠 = 1080；相邻块 top 差 = 步长（末块除外）；块高 ≤ CHUNK_HEIGHT。
        let plan = OcrEngine::plan_chunks(3000);
        let step = CHUNK_HEIGHT - CHUNK_OVERLAP;
        for win in plan.windows(2) {
            assert_eq!(win[1].0 - win[0].0, step, "相邻块步长应为 {}", step);
        }
        for &(_, ch) in &plan {
            assert!(ch <= CHUNK_HEIGHT, "块高 {} 超过 {}", ch, CHUNK_HEIGHT);
        }
    }
}
