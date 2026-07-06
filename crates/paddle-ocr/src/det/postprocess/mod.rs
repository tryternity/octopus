mod box_score;
mod contour;
mod filter;
mod geometry;
mod threshold;
#[cfg(test)]
mod tests;
mod unclip;

use ndarray::ArrayView2;
use rayon::prelude::*;
#[cfg(test)]
use ndarray::Array2;

use crate::Quad;

#[derive(Debug, Clone)]
pub struct DbPostProcess {
    pub thresh: f32,
    pub box_thresh: f32,
    pub max_candidates: usize,
    pub unclip_ratio: f32,
    pub min_size: usize,
    pub use_dilation: bool,
    pub score_mode: String,
}

impl Default for DbPostProcess {
    fn default() -> Self {
        Self {
            thresh: 0.3,
            box_thresh: 0.5,
            max_candidates: 1000,
            unclip_ratio: 1.6,
            min_size: 3,
            use_dilation: true,
            score_mode: "fast".to_string(),
        }
    }
}

impl DbPostProcess {
    #[cfg(test)]
    pub fn run(&self, pred: &Array2<f32>, src_w: usize, src_h: usize) -> (Vec<Quad>, Vec<f32>) {
        self.run_view(pred.view(), src_w, src_h)
    }

    pub(crate) fn run_view(
        &self,
        pred: ArrayView2<'_, f32>,
        src_w: usize,
        src_h: usize,
    ) -> (Vec<Quad>, Vec<f32>) {
        let height = pred.nrows();
        let width = pred.ncols();
        if height == 0 || width == 0 {
            return (Vec::new(), Vec::new());
        }

        let mut bitmap = threshold::build_threshold_bitmap(pred, self.thresh);
        if self.use_dilation {
            bitmap = contour::dilate_mask_2x2(&bitmap, width, height);
        }
        let contours = contour::find_contours_from_mask_pure(bitmap, width, height);
        let (boxes, scores) =
            self.boxes_from_bitmap_pure(pred, &contours, width, height, src_w, src_h);
        let (mut boxes, mut scores) = filter::filter_det_res(boxes, scores, src_h, src_w);
        filter::sort_boxes_like_python(&mut boxes, &mut scores, 10.0);
        (boxes, scores)
    }

    fn boxes_from_bitmap_pure(
        &self,
        pred: ArrayView2<'_, f32>,
        contours: &[Vec<[i32; 2]>],
        bitmap_w: usize,
        bitmap_h: usize,
        dest_w: usize,
        dest_h: usize,
    ) -> (Vec<Quad>, Vec<f32>) {
        let num_candidates = if self.max_candidates == 0 {
            contours.len()
        } else {
            contours.len().min(self.max_candidates)
        };

        let mut boxes = Vec::new();
        let mut scores = Vec::new();
        if num_candidates == 0 {
            return (boxes, scores);
        }
        let scale_target = ScaleTarget {
            bitmap_w,
            bitmap_h,
            dest_w,
            dest_h,
        };

        const PARALLEL_CANDIDATE_THRESHOLD: usize = 12;
        let run_parallel =
            num_candidates >= PARALLEL_CANDIDATE_THRESHOLD && rayon::current_num_threads() > 1;
        if !run_parallel {
            let mut scratch = CandidateScratch::default();
            for (i, contour) in contours.iter().take(num_candidates).enumerate() {
                if let Some((box1, score)) =
                    self.process_contour_candidate_pure(pred, contour, i, &scale_target, &mut scratch)
                {
                    boxes.push(box1);
                    scores.push(score);
                }
            }
            return (boxes, scores);
        }

        let candidate_results: Vec<Option<(Quad, f32)>> = contours[..num_candidates]
            .par_iter()
            .enumerate()
            .map_init(CandidateScratch::default, |scratch, (i, contour)| {
                self.process_contour_candidate_pure(pred, contour, i, &scale_target, scratch)
            })
            .collect();

        for (box1, score) in candidate_results.into_iter().flatten() {
            boxes.push(box1);
            scores.push(score);
        }

        (boxes, scores)
    }

    fn process_contour_candidate_pure(
        &self,
        pred: ArrayView2<'_, f32>,
        contour: &[[i32; 2]],
        _contour_idx: usize,
        scale_target: &ScaleTarget,
        scratch: &mut CandidateScratch,
    ) -> Option<(Quad, f32)> {
        if contour.len() < 3 {
            return None;
        }

        scratch.contour_f.clear();
        scratch
            .contour_f
            .extend(contour.iter().map(|p| [p[0] as f32, p[1] as f32]));
        let (points, sside) = geometry::mini_box_from_points_pure(&scratch.contour_f)?;
        if sside < self.min_size as f32 {
            return None;
        }

        let score = if self.score_mode.eq_ignore_ascii_case("slow") {
            box_score::contour_score_pure_with_scratch(pred, contour, scratch)
        } else {
            box_score::box_score_fast_pure_with_scratch(pred, &points, scratch)
        };
        if self.box_thresh > score {
            return None;
        }

        unclip::unclip_polygon_pyclipper_into(&points, self.unclip_ratio, &mut scratch.expanded);
        if scratch.expanded.len() < 3 {
            return None;
        }

        let (box1, sside2) = geometry::mini_box_from_points_pure(&scratch.expanded)?;
        let mut box1 = box1;
        if sside2 < self.min_size as f32 + 2.0 {
            return None;
        }
        unclip::scale_box_to_dest(
            &mut box1,
            scale_target.bitmap_w,
            scale_target.bitmap_h,
            scale_target.dest_w,
            scale_target.dest_h,
        );
        Some((box1, score))
    }
}

#[derive(Default)]
pub(super) struct CandidateScratch {
    pub contour_f: Vec<[f32; 2]>,
    pub shifted_poly: Vec<[f32; 2]>,
    pub mask: Vec<u8>,
    pub expanded: Vec<[f32; 2]>,
}

struct ScaleTarget {
    bitmap_w: usize,
    bitmap_h: usize,
    dest_w: usize,
    dest_h: usize,
}
