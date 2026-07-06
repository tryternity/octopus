use ndarray::ArrayView2;

use super::CandidateScratch;
use super::unclip::fill_polygon_mask;

#[cfg(test)]
pub(super) fn box_score_fast_pure(bitmap: ArrayView2<'_, f32>, box_points: &[[f32; 2]]) -> f32 {
    let mut scratch = CandidateScratch::default();
    box_score_fast_pure_with_scratch(bitmap, box_points, &mut scratch)
}

pub(super) fn box_score_fast_pure_with_scratch(
    bitmap: ArrayView2<'_, f32>,
    box_points: &[[f32; 2]],
    scratch: &mut CandidateScratch,
) -> f32 {
    let h = bitmap.nrows() as i32;
    let w = bitmap.ncols() as i32;
    if h <= 0 || w <= 0 || box_points.is_empty() {
        return 0.0;
    }

    let mut xmin_f = f32::INFINITY;
    let mut xmax_f = f32::NEG_INFINITY;
    let mut ymin_f = f32::INFINITY;
    let mut ymax_f = f32::NEG_INFINITY;
    for p in box_points {
        xmin_f = xmin_f.min(p[0]);
        xmax_f = xmax_f.max(p[0]);
        ymin_f = ymin_f.min(p[1]);
        ymax_f = ymax_f.max(p[1]);
    }

    let xmin = xmin_f.floor().clamp(0.0, (w - 1) as f32) as i32;
    let xmax = xmax_f.ceil().clamp(0.0, (w - 1) as f32) as i32;
    let ymin = ymin_f.floor().clamp(0.0, (h - 1) as f32) as i32;
    let ymax = ymax_f.ceil().clamp(0.0, (h - 1) as f32) as i32;

    if xmin > xmax || ymin > ymax {
        return 0.0;
    }

    let local_w = (xmax - xmin + 1) as usize;
    let local_h = (ymax - ymin + 1) as usize;

    scratch.shifted_poly.clear();
    scratch.shifted_poly.reserve(box_points.len());
    for p in box_points {
        let x = (p[0] - xmin as f32) as i32;
        let y = (p[1] - ymin as f32) as i32;
        scratch.shifted_poly.push([x as f32, y as f32]);
    }

    let mask_len = local_w * local_h;
    if scratch.mask.len() < mask_len {
        scratch.mask.resize(mask_len, 0);
    }
    let mask = &mut scratch.mask[..mask_len];
    fill_polygon_mask(mask, local_w, local_h, &scratch.shifted_poly);
    masked_mean_in_roi(bitmap, xmin as usize, ymin as usize, local_w, local_h, mask)
}

#[cfg(test)]
pub(super) fn contour_score_pure(bitmap: ArrayView2<'_, f32>, contour: &[[i32; 2]]) -> f32 {
    let mut scratch = CandidateScratch::default();
    contour_score_pure_with_scratch(bitmap, contour, &mut scratch)
}

pub(super) fn contour_score_pure_with_scratch(
    bitmap: ArrayView2<'_, f32>,
    contour: &[[i32; 2]],
    scratch: &mut CandidateScratch,
) -> f32 {
    if contour.len() < 3 {
        return 0.0;
    }

    let h = bitmap.nrows() as i32;
    let w = bitmap.ncols() as i32;
    if h <= 0 || w <= 0 {
        return 0.0;
    }

    let mut min_x = i32::MAX;
    let mut max_x = i32::MIN;
    let mut min_y = i32::MAX;
    let mut max_y = i32::MIN;

    for p in contour {
        min_x = min_x.min(p[0]);
        max_x = max_x.max(p[0]);
        min_y = min_y.min(p[1]);
        max_y = max_y.max(p[1]);
    }

    let xmin = min_x.max(0);
    let xmax = (max_x + 1).min(w - 1);
    let ymin = min_y.max(0);
    let ymax = (max_y + 1).min(h - 1);

    if xmin > xmax || ymin > ymax {
        return 0.0;
    }

    let local_w = (xmax - xmin + 1) as usize;
    let local_h = (ymax - ymin + 1) as usize;

    scratch.shifted_poly.clear();
    scratch.shifted_poly.reserve(contour.len());
    for p in contour {
        scratch
            .shifted_poly
            .push([(p[0] - xmin) as f32, (p[1] - ymin) as f32]);
    }

    let mask_len = local_w * local_h;
    if scratch.mask.len() < mask_len {
        scratch.mask.resize(mask_len, 0);
    }
    let mask = &mut scratch.mask[..mask_len];
    fill_polygon_mask(mask, local_w, local_h, &scratch.shifted_poly);
    masked_mean_in_roi(bitmap, xmin as usize, ymin as usize, local_w, local_h, mask)
}

pub(super) fn masked_mean_in_roi(
    bitmap: ArrayView2<'_, f32>,
    xmin: usize,
    ymin: usize,
    local_w: usize,
    local_h: usize,
    mask: &[u8],
) -> f32 {
    debug_assert_eq!(mask.len(), local_w * local_h);
    if local_w == 0 || local_h == 0 || mask.is_empty() {
        return 0.0;
    }

    if let Some(src) = bitmap.as_slice_memory_order() {
        return masked_mean_in_roi_contiguous(
            src,
            bitmap.ncols(),
            xmin,
            ymin,
            local_w,
            local_h,
            mask,
        );
    }

    let mut sum = 0.0_f64;
    let mut count = 0_usize;
    for y in 0..local_h {
        let src_y = ymin + y;
        let row_off = y * local_w;
        for x in 0..local_w {
            if mask[row_off + x] == 0 {
                continue;
            }
            sum += f64::from(bitmap[[src_y, xmin + x]]);
            count += 1;
        }
    }

    if count == 0 {
        0.0
    } else {
        (sum / count as f64) as f32
    }
}

fn masked_mean_in_roi_contiguous(
    bitmap: &[f32],
    bitmap_w: usize,
    xmin: usize,
    ymin: usize,
    local_w: usize,
    local_h: usize,
    mask: &[u8],
) -> f32 {
    let mut sum = 0.0_f64;
    let mut count = 0_usize;
    for y in 0..local_h {
        let src_row_off = (ymin + y) * bitmap_w + xmin;
        let mask_row_off = y * local_w;
        unsafe {
            let src_ptr = bitmap.as_ptr().add(src_row_off);
            let mask_ptr = mask.as_ptr().add(mask_row_off);
            let mut x = 0usize;
            while x < local_w {
                while x < local_w && *mask_ptr.add(x) == 0 {
                    x += 1;
                }
                if x >= local_w {
                    break;
                }
                let run_start = x;
                while x < local_w && *mask_ptr.add(x) != 0 {
                    x += 1;
                }
                let run_len = x - run_start;
                sum += sum_f32_slice(src_ptr.add(run_start), run_len);
                count += run_len;
            }
        }
    }

    if count == 0 {
        0.0
    } else {
        (sum / count as f64) as f32
    }
}

#[inline]
unsafe fn sum_f32_slice(ptr: *const f32, len: usize) -> f64 {
    #[cfg(target_arch = "x86_64")]
    if std::arch::is_x86_feature_detected!("avx2") {
        return unsafe { sum_f32_slice_avx2(ptr, len) };
    }

    let mut sum = 0.0_f64;
    let mut i = 0usize;
    while i < len {
        unsafe {
            sum += f64::from(*ptr.add(i));
        }
        i += 1;
    }
    sum
}

#[cfg(target_arch = "x86_64")]
#[target_feature(enable = "avx2")]
unsafe fn sum_f32_slice_avx2(ptr: *const f32, len: usize) -> f64 {
    use std::arch::x86_64::{_mm256_add_ps, _mm256_loadu_ps, _mm256_setzero_ps, _mm256_storeu_ps};

    let mut i = 0usize;
    let simd_len = len / 8 * 8;
    let mut acc = _mm256_setzero_ps();
    while i < simd_len {
        let v = unsafe { _mm256_loadu_ps(ptr.add(i)) };
        acc = _mm256_add_ps(acc, v);
        i += 8;
    }

    let mut lanes = [0.0_f32; 8];
    unsafe {
        _mm256_storeu_ps(lanes.as_mut_ptr(), acc);
    }
    let mut sum = lanes.iter().map(|v| f64::from(*v)).sum::<f64>();

    while i < len {
        unsafe {
            sum += f64::from(*ptr.add(i));
        }
        i += 1;
    }

    sum
}
