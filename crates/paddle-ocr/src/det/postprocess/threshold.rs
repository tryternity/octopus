use ndarray::ArrayView2;
use rayon::prelude::*;
#[cfg(target_arch = "x86_64")]
use std::sync::OnceLock;

pub(super) fn build_threshold_bitmap(pred: ArrayView2<'_, f32>, thresh: f32) -> Vec<u8> {
    let mut out = vec![0_u8; pred.len()];
    if out.is_empty() {
        return out;
    }

    if let Some(src) = pred.as_slice_memory_order() {
        threshold_slice_to_bitmap(src, thresh, &mut out);
    } else {
        for (dst, src) in out.iter_mut().zip(pred.iter()) {
            *dst = if *src > thresh { 255 } else { 0 };
        }
    }

    out
}

fn threshold_slice_to_bitmap(src: &[f32], thresh: f32, dst: &mut [u8]) {
    debug_assert_eq!(src.len(), dst.len());
    if src.is_empty() {
        return;
    }

    const PAR_THRESHOLD: usize = 8 * 1024;
    if src.len() < PAR_THRESHOLD {
        threshold_chunk_dispatch(src, thresh, dst);
        return;
    }

    let chunk_size = src.len().div_ceil(rayon::current_num_threads()).max(1024);
    dst.par_chunks_mut(chunk_size)
        .enumerate()
        .for_each(|(chunk_idx, dst_chunk)| {
            let start = chunk_idx * chunk_size;
            let end = start + dst_chunk.len();
            threshold_chunk_dispatch(&src[start..end], thresh, dst_chunk);
        });
}

fn threshold_chunk_dispatch(src: &[f32], thresh: f32, dst: &mut [u8]) {
    debug_assert_eq!(src.len(), dst.len());

    #[cfg(target_arch = "x86_64")]
    {
        if std::arch::is_x86_feature_detected!("avx2") {
            unsafe {
                threshold_chunk_avx2(src, thresh, dst);
            }
            return;
        }
        if std::arch::is_x86_feature_detected!("sse4.1") {
            unsafe {
                threshold_chunk_sse41(src, thresh, dst);
            }
            return;
        }
    }

    threshold_chunk_scalar(src, thresh, dst);
}

fn threshold_chunk_scalar(src: &[f32], thresh: f32, dst: &mut [u8]) {
    debug_assert_eq!(src.len(), dst.len());
    for i in 0..src.len() {
        unsafe {
            let value = *src.get_unchecked(i);
            *dst.get_unchecked_mut(i) = if value > thresh { 255 } else { 0 };
        }
    }
}

#[cfg(target_arch = "x86_64")]
fn threshold_mask_lut8() -> &'static [u64; 256] {
    static LUT: OnceLock<[u64; 256]> = OnceLock::new();
    LUT.get_or_init(|| {
        let mut out = [0_u64; 256];
        let mut mask = 0usize;
        while mask < 256 {
            let mut packed = 0_u64;
            let mut lane = 0usize;
            while lane < 8 {
                if (mask >> lane) & 1 == 1 {
                    packed |= (255_u64) << (lane * 8);
                }
                lane += 1;
            }
            out[mask] = packed;
            mask += 1;
        }
        out
    })
}

#[cfg(target_arch = "x86_64")]
fn threshold_mask_lut4() -> &'static [u32; 16] {
    static LUT: OnceLock<[u32; 16]> = OnceLock::new();
    LUT.get_or_init(|| {
        let mut out = [0_u32; 16];
        let mut mask = 0usize;
        while mask < 16 {
            let mut packed = 0_u32;
            let mut lane = 0usize;
            while lane < 4 {
                if (mask >> lane) & 1 == 1 {
                    packed |= (255_u32) << (lane * 8);
                }
                lane += 1;
            }
            out[mask] = packed;
            mask += 1;
        }
        out
    })
}

#[cfg(target_arch = "x86_64")]
#[target_feature(enable = "avx2")]
unsafe fn threshold_chunk_avx2(src: &[f32], thresh: f32, dst: &mut [u8]) {
    use std::arch::x86_64::{
        __m256, _CMP_GT_OQ, _mm256_cmp_ps, _mm256_loadu_ps, _mm256_movemask_ps, _mm256_set1_ps,
    };

    debug_assert_eq!(src.len(), dst.len());
    let mut i = 0usize;
    let thresh_vec: __m256 = _mm256_set1_ps(thresh);
    let simd_len = src.len() / 8 * 8;
    let lut = threshold_mask_lut8();

    while i < simd_len {
        let mask = unsafe {
            let src_ptr = src.as_ptr().add(i);
            let values = _mm256_loadu_ps(src_ptr);
            let cmp = _mm256_cmp_ps(values, thresh_vec, _CMP_GT_OQ);
            _mm256_movemask_ps(cmp) as usize
        };
        unsafe {
            std::ptr::write_unaligned(dst.as_mut_ptr().add(i) as *mut u64, lut[mask]);
        };
        i += 8;
    }

    if i < src.len() {
        threshold_chunk_scalar(&src[i..], thresh, &mut dst[i..]);
    }
}

#[cfg(target_arch = "x86_64")]
#[target_feature(enable = "sse4.1")]
unsafe fn threshold_chunk_sse41(src: &[f32], thresh: f32, dst: &mut [u8]) {
    use std::arch::x86_64::{__m128, _mm_cmpgt_ps, _mm_loadu_ps, _mm_movemask_ps, _mm_set1_ps};

    debug_assert_eq!(src.len(), dst.len());
    let mut i = 0usize;
    let thresh_vec: __m128 = _mm_set1_ps(thresh);
    let simd_len = src.len() / 4 * 4;
    let lut = threshold_mask_lut4();

    while i < simd_len {
        let mask = unsafe {
            let src_ptr = src.as_ptr().add(i);
            let values = _mm_loadu_ps(src_ptr);
            let cmp = _mm_cmpgt_ps(values, thresh_vec);
            _mm_movemask_ps(cmp) as usize
        };
        unsafe {
            std::ptr::write_unaligned(dst.as_mut_ptr().add(i) as *mut u32, lut[mask]);
        };
        i += 4;
    }

    if i < src.len() {
        threshold_chunk_scalar(&src[i..], thresh, &mut dst[i..]);
    }
}
