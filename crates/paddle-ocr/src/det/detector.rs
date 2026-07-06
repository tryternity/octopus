use std::{path::PathBuf, time::Instant};

use ndarray::{ArrayView4, Axis, s};
use serde::{Deserialize, Serialize};

use crate::{
    Quad,
    config::{LangDet, ModelType, OcrVersion, RecImage, RuntimeConfig},
    error::{PaddleOcrError, Result},
    runtime::provider::ProviderResolution,
    runtime::session::{OrtSession, SessionContract},
};

use super::{
    postprocess::DbPostProcess,
    preprocess::{DetPreProcess, DetPreprocessScratch},
};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct DetectorConfig {
    pub lang: LangDet,
    pub ocr_version: OcrVersion,
    pub model_type: ModelType,
    pub model_path: Option<PathBuf>,
    pub allow_download: bool,
    pub runtime: RuntimeConfig,
    pub limit_side_len: usize,
    pub limit_type: String,
    pub std: [f32; 3],
    pub mean: [f32; 3],
    pub thresh: f32,
    pub box_thresh: f32,
    pub max_candidates: usize,
    pub unclip_ratio: f32,
    pub use_dilation: bool,
    pub score_mode: String,
    pub model_store_dir: Option<PathBuf>,
}

impl Default for DetectorConfig {
    fn default() -> Self {
        Self {
            lang: LangDet::Ch,
            ocr_version: OcrVersion::PPocrV4,
            model_type: ModelType::Mobile,
            model_path: None,
            allow_download: true,
            runtime: RuntimeConfig::default(),
            limit_side_len: 736,
            limit_type: "min".to_string(),
            std: [0.5, 0.5, 0.5],
            mean: [0.5, 0.5, 0.5],
            thresh: 0.3,
            box_thresh: 0.5,
            max_candidates: 1000,
            unclip_ratio: 1.6,
            use_dilation: true,
            score_mode: "fast".to_string(),
            model_store_dir: None,
        }
    }
}

#[derive(Debug, Clone, Default)]
pub struct DetOutput {
    pub boxes: Vec<Quad>,
    pub scores: Vec<f32>,
    pub elapsed_ms: f32,
    pub breakdown: Option<DetTimingBreakdown>,
}

#[derive(Debug, Clone, Copy, Default)]
pub struct DetTimingBreakdown {
    pub preprocess_ms: f32,
    pub infer_ms: f32,
    pub postprocess_ms: f32,
}

#[derive(Debug)]
pub struct Detector {
    pre: DetPreProcess,
    post: DbPostProcess,
    session: OrtSession,
    batch_scratch: Vec<f32>,
    preprocess_scratch: DetPreprocessScratch,
}

impl Detector {
    pub fn new(config: DetectorConfig) -> Result<Self> {
        let model_path = config
            .model_path
            .as_ref()
            .ok_or_else(|| PaddleOcrError::Config("detector model_path is not set".to_string()))?;
        if !model_path.exists() {
            return Err(PaddleOcrError::FileNotFound(model_path.clone()));
        }

        let pre = DetPreProcess {
            limit_side_len: config.limit_side_len,
            limit_type: config.limit_type,
            mean: config.mean,
            std: config.std,
        };
        let post = DbPostProcess {
            thresh: config.thresh,
            box_thresh: config.box_thresh,
            max_candidates: config.max_candidates,
            unclip_ratio: config.unclip_ratio,
            use_dilation: config.use_dilation,
            score_mode: config.score_mode,
            ..DbPostProcess::default()
        };
        let session =
            OrtSession::new_with_contract(&model_path, &config.runtime, SessionContract::Det)?;
        Ok(Self {
            pre,
            post,
            session,
            batch_scratch: Vec::new(),
            preprocess_scratch: DetPreprocessScratch::default(),
        })
    }

    pub fn detect(&mut self, img: &RecImage) -> Result<DetOutput> {
        let start = Instant::now();
        // `DBPostProcess` expects destination size in the detector input image space
        // (before detector-side resize), matching RapidOCR Python behavior.
        let det_limit_side_len = resolve_limit_side_len_like_python(
            self.pre.limit_type.as_str(),
            self.pre.limit_side_len,
            img.width().max(img.height()),
        );
        let pre_start = Instant::now();
        let (resized_h, resized_w) = self.pre.run_into_buffer_with_scratch(
            img,
            &mut self.batch_scratch,
            &mut self.preprocess_scratch,
            Some(det_limit_side_len),
        )?;
        let preprocess_ms = pre_start.elapsed().as_secs_f32() * 1000.0;
        let batch_view = ArrayView4::from_shape(
            (1, 3, resized_h, resized_w),
            &self.batch_scratch[..3 * resized_h * resized_w],
        )
        .map_err(|e| PaddleOcrError::InvalidInput(format!("invalid det batch shape: {e}")))?;
        let post = &self.post;
        let infer_start = Instant::now();
        let mut postprocess_ms = 0.0_f32;
        let (boxes, scores) = self.session.run_array4_view_with(batch_view, |preds| {
            if preds.len_of(Axis(0)) == 0 || preds.len_of(Axis(1)) == 0 {
                return Ok((Vec::new(), Vec::new()));
            }
            let post_start = Instant::now();
            let map = preds.slice(s![0, 0, .., ..]);
            let out = post.run_view(map, img.width(), img.height());
            postprocess_ms = post_start.elapsed().as_secs_f32() * 1000.0;
            Ok(out)
        })?;
        let infer_total_ms = infer_start.elapsed().as_secs_f32() * 1000.0;
        let infer_ms = (infer_total_ms - postprocess_ms).max(0.0);

        Ok(DetOutput {
            boxes,
            scores,
            elapsed_ms: start.elapsed().as_secs_f32() * 1000.0,
            breakdown: Some(DetTimingBreakdown {
                preprocess_ms,
                infer_ms,
                postprocess_ms,
            }),
        })
    }

    pub fn update_postprocess(&mut self, box_thresh: Option<f32>, unclip_ratio: Option<f32>) {
        if let Some(v) = box_thresh {
            self.post.box_thresh = v;
        }
        if let Some(v) = unclip_ratio {
            self.post.unclip_ratio = v;
        }
    }

    pub fn provider_resolution(&self) -> ProviderResolution {
        self.session.provider_resolution()
    }
}

fn resolve_limit_side_len_like_python(
    limit_type: &str,
    configured_limit_side_len: usize,
    max_wh: usize,
) -> usize {
    if limit_type == "min" {
        configured_limit_side_len
    } else if max_wh < 960 {
        960
    } else if max_wh < 1500 {
        1500
    } else {
        2000
    }
}

#[cfg(test)]
mod tests {
    use super::resolve_limit_side_len_like_python;

    #[test]
    fn limit_side_len_for_min_uses_configured_value() {
        assert_eq!(resolve_limit_side_len_like_python("min", 736, 2048), 736);
    }

    #[test]
    fn limit_side_len_for_non_min_matches_python_buckets() {
        assert_eq!(resolve_limit_side_len_like_python("max", 736, 800), 960);
        assert_eq!(resolve_limit_side_len_like_python("max", 736, 1200), 1500);
        assert_eq!(resolve_limit_side_len_like_python("max", 736, 1800), 2000);
    }
}
