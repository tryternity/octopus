use crate::{
    cls::classifier::ClassifierConfig,
    config::RecognizerConfig,
    config::RuntimeConfig,
    det::detector::DetectorConfig,
    error::{PaddleOcrError, Result},
};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct GlobalConfig {
    pub text_score: f32,
    pub use_det: bool,
    pub use_cls: bool,
    pub use_rec: bool,
    pub min_height: usize,
    pub width_height_ratio: f32,
    pub max_side_len: usize,
    pub min_side_len: usize,
    pub return_word_box: bool,
    pub return_single_char_box: bool,
}

impl Default for GlobalConfig {
    fn default() -> Self {
        Self {
            text_score: 0.5,
            use_det: true,
            use_cls: true,
            use_rec: true,
            min_height: 30,
            width_height_ratio: 8.0,
            max_side_len: 2000,
            min_side_len: 30,
            return_word_box: false,
            return_single_char_box: false,
        }
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct EngineConfig {
    pub global: GlobalConfig,
    pub det: DetectorConfig,
    pub cls: ClassifierConfig,
    pub rec: RecognizerConfig,
}

impl EngineConfig {
    pub fn validate(&self) -> Result<()> {
        validate_inclusive_range("global.text_score", self.global.text_score, 0.0, 1.0)?;

        if self.global.min_height == 0 {
            return Err(PaddleOcrError::Config(
                "global.min_height must be greater than zero".to_string(),
            ));
        }
        if self.global.min_side_len == 0 {
            return Err(PaddleOcrError::Config(
                "global.min_side_len must be greater than zero".to_string(),
            ));
        }
        if self.global.max_side_len == 0 {
            return Err(PaddleOcrError::Config(
                "global.max_side_len must be greater than zero".to_string(),
            ));
        }
        if self.global.min_side_len > self.global.max_side_len {
            return Err(PaddleOcrError::Config(format!(
                "global.min_side_len ({}) must be <= global.max_side_len ({})",
                self.global.min_side_len, self.global.max_side_len
            )));
        }
        if self.global.width_height_ratio <= 0.0 {
            return Err(PaddleOcrError::Config(
                "global.width_height_ratio must be > 0".to_string(),
            ));
        }

        if self.det.limit_side_len == 0 {
            return Err(PaddleOcrError::Config(
                "det.limit_side_len must be greater than zero".to_string(),
            ));
        }
        validate_inclusive_range("det.thresh", self.det.thresh, 0.0, 1.0)?;
        validate_inclusive_range("det.box_thresh", self.det.box_thresh, 0.0, 1.0)?;
        if self.det.max_candidates == 0 {
            return Err(PaddleOcrError::Config(
                "det.max_candidates must be greater than zero".to_string(),
            ));
        }
        if self.det.unclip_ratio <= 0.0 {
            return Err(PaddleOcrError::Config(
                "det.unclip_ratio must be > 0".to_string(),
            ));
        }

        if self.cls.cls_batch_num == 0 {
            return Err(PaddleOcrError::Config(
                "cls.cls_batch_num must be greater than zero".to_string(),
            ));
        }
        validate_inclusive_range("cls.cls_thresh", self.cls.cls_thresh, 0.0, 1.0)?;
        if self.cls.cls_image_shape.contains(&0) {
            return Err(PaddleOcrError::Config(format!(
                "cls.cls_image_shape must not contain zero values, got {:?}",
                self.cls.cls_image_shape
            )));
        }

        if self.rec.rec_batch_num == 0 {
            return Err(PaddleOcrError::Config(
                "rec.rec_batch_num must be greater than zero".to_string(),
            ));
        }
        if self.rec.rec_img_shape[0] != 3 {
            return Err(PaddleOcrError::Config(format!(
                "rec.rec_img_shape must start with channel=3, got {:?}",
                self.rec.rec_img_shape
            )));
        }
        if self.rec.rec_img_shape.contains(&0) {
            return Err(PaddleOcrError::Config(format!(
                "rec.rec_img_shape must not contain zero values, got {:?}",
                self.rec.rec_img_shape
            )));
        }

        validate_runtime_config("det.runtime", &self.det.runtime)?;
        validate_runtime_config("cls.runtime", &self.cls.runtime)?;
        validate_runtime_config("rec.runtime", &self.rec.runtime)?;

        Ok(())
    }
}

fn validate_runtime_config(prefix: &str, runtime: &RuntimeConfig) -> Result<()> {
    if runtime.intra_threads.is_some_and(|v| v == 0) {
        return Err(PaddleOcrError::Config(format!(
            "{prefix}.intra_threads must be greater than zero when set"
        )));
    }
    if runtime.inter_threads.is_some_and(|v| v == 0) {
        return Err(PaddleOcrError::Config(format!(
            "{prefix}.inter_threads must be greater than zero when set"
        )));
    }
    if runtime.rayon_threads.is_some_and(|v| v == 0) {
        return Err(PaddleOcrError::Config(format!(
            "{prefix}.rayon_threads must be greater than zero when set"
        )));
    }
    Ok(())
}

fn validate_inclusive_range(name: &str, value: f32, min: f32, max: f32) -> Result<()> {
    if !(min..=max).contains(&value) {
        return Err(PaddleOcrError::Config(format!(
            "{name} must be in range [{min}, {max}], got {value}"
        )));
    }
    Ok(())
}
