use anyhow::{Context, Result};
use ndarray::{Array1, Array3, ArrayView2};
use ort::session::Session;
use std::path::Path;

/// Silero VAD v4 via ort (ONNX Runtime)
/// Stateful model with LSTM hidden/cell states
pub struct SileroVad {
    session: Session,
    h: Array3<f32>,  // [2, 1, 64]
    c: Array3<f32>,  // [2, 1, 64]
    sr: Array1<i64>, // scalar: 16000
}

impl SileroVad {
    pub fn new(model_path: &Path) -> Result<Self> {
        let session = Session::builder()
            .context("Failed to create ORT session builder")?
            .commit_from_file(model_path)
            .with_context(|| format!("Failed to load Silero VAD from {:?}", model_path))?;
        Ok(Self {
            session,
            h: Array3::zeros((2, 1, 64)),
            c: Array3::zeros((2, 1, 64)),
            sr: Array1::from_vec(vec![16000i64]),
        })
    }

    /// Input: 480 samples (30ms @ 16kHz). Output: speech probability [0.0, 1.0]
    pub fn compute(&mut self, samples: &[f32]) -> Result<f32> {
        let input = ArrayView2::from_shape((1, samples.len()), samples)?;
        let h_tensor = ort::value::TensorRef::from_array_view(self.h.view())?;
        let c_tensor = ort::value::TensorRef::from_array_view(self.c.view())?;
        let sr_tensor = ort::value::TensorRef::from_array_view(self.sr.view())?;
        let input_tensor = ort::value::TensorRef::from_array_view(input)?;

        let outputs = self.session.run(ort::inputs! {
            "input" => input_tensor,
            "sr" => sr_tensor,
            "h" => h_tensor,
            "c" => c_tensor
        })?;

        // Extract probability
        let (_shape, data) = outputs["output"].try_extract_tensor::<f32>()?;
        let prob = data[0];

        // Update hidden/cell states for next call
        let (_h_shape, h_data) = outputs["hn"].try_extract_tensor::<f32>()?;
        if let Some(h_slice) = self.h.as_slice_mut() {
            h_slice.copy_from_slice(&h_data);
        } else {
            self.h = Array3::from_shape_vec((2, 1, 64), h_data.to_vec())?;
        }

        let (_c_shape, c_data) = outputs["cn"].try_extract_tensor::<f32>()?;
        if let Some(c_slice) = self.c.as_slice_mut() {
            c_slice.copy_from_slice(&c_data);
        } else {
            self.c = Array3::from_shape_vec((2, 1, 64), c_data.to_vec())?;
        }

        Ok(prob)
    }

    /// Reset LSTM states (call between different audio segments)
    pub fn reset(&mut self) {
        self.h = Array3::zeros((2, 1, 64));
        self.c = Array3::zeros((2, 1, 64));
    }
}
