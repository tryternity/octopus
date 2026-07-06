use nalgebra::{SMatrix, SVector};
use std::sync::OnceLock;

use crate::{
    Quad,
    config::RecImage,
    error::Result,
};

use super::numeric::{
    clamp_i32_inclusive, interpolate_cubic_coeffs, l2,
    saturate_cast_i16, saturate_cast_i16_from_f32, saturate_cast_i32_round,
};

pub fn rotate_crop_image(img: &RecImage, points: Quad) -> Result<RecImage> {
    rotate_crop_image_pure(img, points)
}

fn rotate_crop_image_pure(img: &RecImage, points: Quad) -> Result<RecImage> {
    if let Some(crop) = try_axis_aligned_crop(img, points)? {
        return Ok(crop);
    }

    let img_crop_width = l2(points[0], points[1]).max(l2(points[2], points[3]));
    let img_crop_height = l2(points[0], points[3]).max(l2(points[1], points[2]));
    let crop_w = img_crop_width.max(1.0) as usize;
    let crop_h = img_crop_height.max(1.0) as usize;

    let pts_std = [
        [0.0_f32, 0.0_f32],
        [crop_w as f32, 0.0_f32],
        [crop_w as f32, crop_h as f32],
        [0.0_f32, crop_h as f32],
    ];

    let h = homography_from_4pt(points, pts_std);
    let inv_h = h
        .try_inverse()
        .unwrap_or_else(SMatrix::<f64, 3, 3>::identity);

    let src = img.as_bgr_cow();
    let src = src.as_ref();
    let mut dst = vec![0_u8; crop_w * crop_h * 3];
    let tab = bicubic_remap_tab();
    let src_w = img.width() as i32;
    let src_h = img.height() as i32;
    let h00 = inv_h[(0, 0)];
    let h01 = inv_h[(0, 1)];
    let h02 = inv_h[(0, 2)];
    let h10 = inv_h[(1, 0)];
    let h11 = inv_h[(1, 1)];
    let h12 = inv_h[(1, 2)];
    let h20 = inv_h[(2, 0)];
    let h21 = inv_h[(2, 1)];
    let h22 = inv_h[(2, 2)];

    for y in 0..crop_h {
        let yy = y as f64;
        let mut fx_num = h01 * yy + h02;
        let mut fy_num = h11 * yy + h12;
        let mut fw_num = h21 * yy + h22;
        for x in 0..crop_w {
            let inv = if fw_num.abs() > f64::EPSILON {
                INTER_TAB_SIZE_F64 / fw_num
            } else {
                0.0
            };

            let fx = fx_num * inv;
            let fy = fy_num * inv;

            let x_scaled = saturate_cast_i32_round(fx);
            let y_scaled = saturate_cast_i32_round(fy);

            let x_base = saturate_cast_i16(x_scaled >> INTER_BITS) as i32;
            let y_base = saturate_cast_i16(y_scaled >> INTER_BITS) as i32;
            let frac_x = (x_scaled & INTER_TAB_MASK) as usize;
            let frac_y = (y_scaled & INTER_TAB_MASK) as usize;
            let wtab = &tab[frac_y * INTER_TAB_SIZE + frac_x];

            let sx = x_base - 1;
            let sy = y_base - 1;

            let dst_idx = (y * crop_w + x) * 3;
            let mut sum_b = 0_i32;
            let mut sum_g = 0_i32;
            let mut sum_r = 0_i32;

            for ky in 0..4 {
                let yy_src = clamp_i32_inclusive(sy + ky as i32, 0, src_h - 1) as usize;
                for kx in 0..4 {
                    let xx_src = clamp_i32_inclusive(sx + kx as i32, 0, src_w - 1) as usize;
                    let src_idx = (yy_src * img.width() + xx_src) * 3;
                    let weight = wtab[ky * 4 + kx] as i32;
                    // Safety: clamped coordinates guarantee in-bounds source access.
                    unsafe {
                        sum_b += *src.get_unchecked(src_idx) as i32 * weight;
                        sum_g += *src.get_unchecked(src_idx + 1) as i32 * weight;
                        sum_r += *src.get_unchecked(src_idx + 2) as i32 * weight;
                    }
                }
            }

            let v_b = (sum_b + (1 << (INTER_REMAP_COEF_BITS - 1))) >> INTER_REMAP_COEF_BITS;
            let v_g = (sum_g + (1 << (INTER_REMAP_COEF_BITS - 1))) >> INTER_REMAP_COEF_BITS;
            let v_r = (sum_r + (1 << (INTER_REMAP_COEF_BITS - 1))) >> INTER_REMAP_COEF_BITS;
            dst[dst_idx] = v_b.clamp(0, 255) as u8;
            dst[dst_idx + 1] = v_g.clamp(0, 255) as u8;
            dst[dst_idx + 2] = v_r.clamp(0, 255) as u8;

            fx_num += h00;
            fy_num += h10;
            fw_num += h20;
        }
    }

    RecImage::from_bgr_u8(crop_w, crop_h, dst)
}

fn try_axis_aligned_crop(img: &RecImage, points: Quad) -> Result<Option<RecImage>> {
    const EPS: f32 = 1e-3;
    let is_axis_aligned = (points[0][1] - points[1][1]).abs() <= EPS
        && (points[2][1] - points[3][1]).abs() <= EPS
        && (points[0][0] - points[3][0]).abs() <= EPS
        && (points[1][0] - points[2][0]).abs() <= EPS;
    if !is_axis_aligned {
        return Ok(None);
    }

    let img_w = img.width() as i32;
    let img_h = img.height() as i32;
    if img_w <= 0 || img_h <= 0 {
        return Ok(None);
    }

    let left = points[0][0].min(points[3][0]).round_ties_even() as i32;
    let right = points[1][0].max(points[2][0]).round_ties_even() as i32;
    let top = points[0][1].min(points[1][1]).round_ties_even() as i32;
    let bottom = points[2][1].max(points[3][1]).round_ties_even() as i32;

    // Keep parity with cv2.warpPerspective + PaddleOCR dest points for axis-aligned boxes:
    // [x0, x1) and [y0, y1), where x1/y1 are the right/bottom vertices.
    let x0 = left.clamp(0, img_w - 1);
    let x1 = right.clamp(0, img_w);
    let y0 = top.clamp(0, img_h - 1);
    let y1 = bottom.clamp(0, img_h);
    if x1 <= x0 || y1 <= y0 {
        return Ok(None);
    }

    let crop_w = (x1 - x0) as usize;
    let crop_h = (y1 - y0) as usize;
    let mut out = vec![0_u8; crop_w * crop_h * 3];

    let src = img.as_bgr_cow();
    let src = src.as_ref();
    let src_row_stride = img.width() * 3;
    let dst_row_stride = crop_w * 3;
    for row in 0..crop_h {
        let src_row_start = ((y0 as usize + row) * src_row_stride) + x0 as usize * 3;
        let dst_row_start = row * dst_row_stride;
        let src_row = &src[src_row_start..src_row_start + dst_row_stride];
        let dst_row = &mut out[dst_row_start..dst_row_start + dst_row_stride];
        dst_row.copy_from_slice(src_row);
    }

    Ok(Some(RecImage::from_bgr_u8(crop_w, crop_h, out)?))
}

const INTER_BITS: i32 = 5;
const INTER_TAB_SIZE: usize = 1usize << INTER_BITS;
const INTER_TAB_MASK: i32 = (INTER_TAB_SIZE as i32) - 1;
const INTER_TAB_SIZE_F64: f64 = INTER_TAB_SIZE as f64;
const INTER_REMAP_COEF_BITS: i32 = 15;
const INTER_REMAP_COEF_SCALE: i32 = 1 << INTER_REMAP_COEF_BITS;

fn build_bicubic_remap_tab() -> Vec<[i16; 16]> {
    let mut tab = vec![[0_i16; 16]; INTER_TAB_SIZE * INTER_TAB_SIZE];
    for fy in 0..INTER_TAB_SIZE {
        let y_coeff = interpolate_cubic_coeffs(fy as f32 / INTER_TAB_SIZE as f32);
        for fx in 0..INTER_TAB_SIZE {
            let x_coeff = interpolate_cubic_coeffs(fx as f32 / INTER_TAB_SIZE as f32);
            let mut isum = 0_i32;
            for ky in 0..4 {
                for kx in 0..4 {
                    let v = y_coeff[ky] * x_coeff[kx];
                    let it = saturate_cast_i16_from_f32(v * INTER_REMAP_COEF_SCALE as f32);
                    tab[fy * INTER_TAB_SIZE + fx][ky * 4 + kx] = it;
                    isum += it as i32;
                }
            }

            if isum != INTER_REMAP_COEF_SCALE {
                let diff = isum - INTER_REMAP_COEF_SCALE;
                let idx = fy * INTER_TAB_SIZE + fx;
                let mut mk = 2 * 4 + 2;
                let mut mk_v = tab[idx][mk];
                let mut mk_max = mk;
                let mut mk_max_v = mk_v;
                for ky in 2..4 {
                    for kx in 2..4 {
                        let pos = ky * 4 + kx;
                        let v = tab[idx][pos];
                        if v < mk_v {
                            mk = pos;
                            mk_v = v;
                        } else if v > mk_max_v {
                            mk_max = pos;
                            mk_max_v = v;
                        }
                    }
                }

                if diff < 0 {
                    let nv =
                        (tab[idx][mk_max] as i32 - diff).clamp(i16::MIN as i32, i16::MAX as i32);
                    tab[idx][mk_max] = nv as i16;
                } else {
                    let nv = (tab[idx][mk] as i32 - diff).clamp(i16::MIN as i32, i16::MAX as i32);
                    tab[idx][mk] = nv as i16;
                }
            }
        }
    }
    tab
}

fn bicubic_remap_tab() -> &'static Vec<[i16; 16]> {
    static TAB: OnceLock<Vec<[i16; 16]>> = OnceLock::new();
    TAB.get_or_init(build_bicubic_remap_tab)
}

fn homography_from_4pt(src: Quad, dst: Quad) -> SMatrix<f64, 3, 3> {
    let mut a = SMatrix::<f64, 8, 8>::zeros();
    let mut b = SVector::<f64, 8>::zeros();

    for i in 0..4 {
        let x = src[i][0] as f64;
        let y = src[i][1] as f64;
        let x_cap = dst[i][0] as f64;
        let y_cap = dst[i][1] as f64;

        let r0 = i * 2;
        let r1 = r0 + 1;

        a[(r0, 0)] = x;
        a[(r0, 1)] = y;
        a[(r0, 2)] = 1.0;
        a[(r0, 6)] = -x * x_cap;
        a[(r0, 7)] = -y * x_cap;
        b[r0] = x_cap;

        a[(r1, 3)] = x;
        a[(r1, 4)] = y;
        a[(r1, 5)] = 1.0;
        a[(r1, 6)] = -x * y_cap;
        a[(r1, 7)] = -y * y_cap;
        b[r1] = y_cap;
    }

    if let Some(h) = a.full_piv_lu().solve(&b) {
        SMatrix::<f64, 3, 3>::from_row_slice(&[h[0], h[1], h[2], h[3], h[4], h[5], h[6], h[7], 1.0])
    } else {
        SMatrix::<f64, 3, 3>::identity()
    }
}


