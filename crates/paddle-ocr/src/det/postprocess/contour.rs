use rayon::prelude::*;

pub(super) fn find_contours_from_mask_pure(mask: Vec<u8>, width: usize, height: usize) -> Vec<Vec<[i32; 2]>> {
    if width == 0 || height == 0 {
        return Vec::new();
    }

    // Mirror cv::findContours preprocess path:
    // 1) copyMakeBorder(..., 1,1,1,1, BORDER_CONSTANT=0)
    // 2) threshold(..., 0, 1, THRESH_BINARY)
    let pad_w = width + 2;
    let pad_h = height + 2;
    let mut image = vec![0_i8; pad_w * pad_h];
    for y in 0..height {
        let src = &mask[y * width..(y + 1) * width];
        let dst_row = &mut image[(y + 1) * pad_w + 1..(y + 1) * pad_w + 1 + width];
        for (dst, src_v) in dst_row.iter_mut().zip(src.iter()) {
            *dst = if *src_v > 0 { MASK8_BLACK as i8 } else { 0_i8 };
        }
    }

    let mut scanner = CvContourScanner8::new(image, pad_w, pad_h, -1, -1);
    let mut out = Vec::new();
    while let Some(contour) = scanner.find_next_contour() {
        out.push(contour);
    }
    out
}

const MASK8_RIGHT: i32 = -128; // 0x80 as signed char
const MASK8_NEW: i32 = 2; // 0x02
const MASK8_FLAGS: i32 = -2; // 0xFE as signed char
const MASK8_BLACK: i32 = 1; // 0x01

const CHAIN_DELTAS: [[i32; 2]; 8] = [
    [1, 0],
    [1, -1],
    [0, -1],
    [-1, -1],
    [-1, 0],
    [-1, 1],
    [0, 1],
    [1, 1],
];

#[inline]
fn chain_delta_index(dir: i32, step: usize) -> isize {
    let d = CHAIN_DELTAS[(dir & 7) as usize];
    d[0] as isize + d[1] as isize * step as isize
}

#[derive(Debug)]
struct CvContourScanner8 {
    image: Vec<i8>,
    width: usize,
    height: usize,
    offset_x: i32,
    offset_y: i32,
    pt_x: usize,
    pt_y: usize,
    lnbd_x: usize,
    lnbd_y: usize,
    nbd: i32,
}

impl CvContourScanner8 {
    fn new(image: Vec<i8>, width: usize, height: usize, offset_x: i32, offset_y: i32) -> Self {
        Self {
            image,
            width,
            height,
            offset_x,
            offset_y,
            pt_x: 1,
            pt_y: 1,
            lnbd_x: 0,
            lnbd_y: 1,
            nbd: 2,
        }
    }

    #[inline]
    fn idx(&self, x: usize, y: usize) -> usize {
        y * self.width + x
    }

    #[inline]
    fn at_i32(&self, x: usize, y: usize) -> i32 {
        self.image[self.idx(x, y)] as i32
    }

    fn find_next_x(
        &self,
        mut x: usize,
        y: usize,
        prev: i32,
        p_out: &mut i32,
        width_bound: usize,
    ) -> usize {
        while x < width_bound {
            let p = self.at_i32(x, y);
            *p_out = p;
            if p != prev {
                return x;
            }
            x += 1;
        }
        x
    }

    fn find_next_contour(&mut self) -> Option<Vec<[i32; 2]>> {
        if self.width < 2 || self.height < 2 {
            return None;
        }

        let width_bound = self.width - 1;
        let height_bound = self.height - 1;
        let mut x = self.pt_x;
        let mut y = self.pt_y;
        if y >= self.height || x >= self.width {
            return None;
        }

        let mut last_pos_x = self.lnbd_x as i32;
        let mut last_pos_y = self.lnbd_y as i32;
        let mut prev = self.at_i32(x.saturating_sub(1), y);

        while y < height_bound {
            let mut p = 0_i32;
            while x < width_bound {
                x = self.find_next_x(x, y, prev, &mut p, width_bound);
                if x >= width_bound {
                    break;
                }

                if let Some(contour) = self.contour_scan(prev, p, &mut last_pos_x, x, y) {
                    self.lnbd_x = last_pos_x.max(0) as usize;
                    self.lnbd_y = last_pos_y.max(0) as usize;
                    return Some(contour);
                }

                prev = p;
                if (prev & MASK8_FLAGS) != 0 {
                    last_pos_x = x as i32;
                }
                x += 1;
            }

            y += 1;
            if y >= height_bound {
                break;
            }
            x = 1;
            prev = 0;
            last_pos_x = 0;
            last_pos_y = y as i32;
        }

        None
    }

    fn contour_scan(
        &mut self,
        prev: i32,
        p: i32,
        last_pos_x: &mut i32,
        x: usize,
        y: usize,
    ) -> Option<Vec<[i32; 2]>> {
        let mut is_hole = false;

        // RETR_LIST + 8-bit logic from contours_new.cpp::contourScan
        if !(prev == 0 && p == MASK8_BLACK) {
            if p != 0 || prev < MASK8_BLACK {
                return None;
            }
            if (prev & MASK8_FLAGS) != 0 {
                *last_pos_x = x as i32 - 1;
            }
            is_hole = true;
        }

        *last_pos_x = x as i32 - if is_hole { 1 } else { 0 };
        let mut nbd_ = self.nbd;
        let contour = self.make_contour(&mut nbd_, is_hole, x as i32, y as i32);
        self.pt_x = x + 1;
        self.pt_y = y;
        self.nbd = nbd_;
        Some(contour)
    }

    fn make_contour(&mut self, _nbd: &mut i32, is_hole: bool, x: i32, y: i32) -> Vec<[i32; 2]> {
        let start_x = x - if is_hole { 1 } else { 0 };
        let start_y = y;
        let origin_x = start_x + self.offset_x;
        let origin_y = start_y + self.offset_y;
        self.fetch_contour_ex(
            [start_x as usize, start_y as usize],
            is_hole,
            false,
            [origin_x, origin_y],
            MASK8_NEW,
        )
    }

    fn fetch_contour_ex(
        &mut self,
        start: [usize; 2],
        is_hole: bool,
        is_direct: bool,
        pt: [i32; 2],
        nbd: i32,
    ) -> Vec<[i32; 2]> {
        let start_x = start[0];
        let start_y = start[1];
        let mut pt_x = pt[0];
        let mut pt_y = pt[1];
        let step = self.width;
        let i0 = self.idx(start_x, start_y) as isize;
        let mut points = Vec::<[i32; 2]>::new();

        let mut s_end: i32 = if is_hole { 0 } else { 4 };
        let mut s = s_end;
        let i1: isize;
        loop {
            s = (s - 1) & 7;
            let ni = i0 + chain_delta_index(s, step);
            if self.image[ni as usize] != 0 || s == s_end {
                i1 = ni;
                break;
            }
        }

        if s == s_end {
            self.image[i0 as usize] = (nbd | MASK8_RIGHT) as i8;
            points.push([pt_x, pt_y]);
            return points;
        }

        let mut i3 = i0;
        let mut prev_s = s ^ 4;
        loop {
            s_end = s;
            s = s.min(15);
            let i4: isize;
            loop {
                if s >= 15 {
                    i4 = i3 + chain_delta_index(s, step);
                    break;
                }
                s += 1;
                let ni = i3 + chain_delta_index(s, step);
                if self.image[ni as usize] != 0 {
                    i4 = ni;
                    break;
                }
            }
            s &= 7;

            if ((s - 1) as u32) < (s_end as u32) {
                self.image[i3 as usize] = (nbd | MASK8_RIGHT) as i8;
            } else if self.image[i3 as usize] as i32 == MASK8_BLACK {
                self.image[i3 as usize] = nbd as i8;
            }

            if s != prev_s || is_direct {
                points.push([pt_x, pt_y]);
            }

            prev_s = s;
            let d = CHAIN_DELTAS[s as usize];
            pt_x += d[0];
            pt_y += d[1];

            if i4 == i0 && i3 == i1 {
                break;
            }

            i3 = i4;
            s = (s + 4) & 7;
        }

        points
    }
}

pub(super) fn dilate_mask_2x2(mask: &[u8], width: usize, height: usize) -> Vec<u8> {
    if width == 0 || height == 0 {
        return Vec::new();
    }

    let mut out = vec![0_u8; width * height];
    #[cfg(target_arch = "x86_64")]
    let use_avx2 = std::arch::is_x86_feature_detected!("avx2");
    out.par_chunks_mut(width).enumerate().for_each(|(y, row)| {
        let row_start = y * width;
        let cur = &mask[row_start..row_start + width];
        let prev = if y == 0 {
            None
        } else {
            Some(&mask[row_start - width..row_start])
        };

        #[cfg(target_arch = "x86_64")]
        if use_avx2 {
            unsafe {
                dilate_row_2x2_avx2(cur, prev, row);
            }
            return;
        }

        dilate_row_2x2_scalar(cur, prev, row);
    });

    out
}

#[inline]
fn dilate_row_2x2_scalar(cur: &[u8], prev: Option<&[u8]>, out: &mut [u8]) {
    let width = cur.len();
    if width == 0 {
        return;
    }

    let prev_row = prev.unwrap_or(&[]);
    out[0] = if let Some(p) = prev {
        cur[0] | p[0]
    } else {
        cur[0]
    };

    for x in 1..width {
        let top = if prev_row.is_empty() { 0 } else { prev_row[x] };
        let top_left = if prev_row.is_empty() {
            0
        } else {
            prev_row[x - 1]
        };
        out[x] = cur[x] | cur[x - 1] | top | top_left;
    }
}

#[cfg(target_arch = "x86_64")]
#[target_feature(enable = "avx2")]
unsafe fn dilate_row_2x2_avx2(cur: &[u8], prev: Option<&[u8]>, out: &mut [u8]) {
    use std::arch::x86_64::{
        __m256i, _mm256_loadu_si256, _mm256_or_si256, _mm256_setzero_si256, _mm256_storeu_si256,
    };

    let width = cur.len();
    if width == 0 {
        return;
    }

    let cur_ptr = cur.as_ptr();
    let out_ptr = out.as_mut_ptr();
    let prev_ptr = prev.map_or(std::ptr::null(), |p| p.as_ptr());

    unsafe {
        *out_ptr = if prev_ptr.is_null() {
            *cur_ptr
        } else {
            *cur_ptr | *prev_ptr
        };
    }

    if width == 1 {
        return;
    }

    let mut x = 1usize;
    let simd_end = width.saturating_sub(31);
    let zero = _mm256_setzero_si256();
    while x < simd_end {
        unsafe {
            let cur_v = _mm256_loadu_si256(cur_ptr.add(x) as *const __m256i);
            let cur_l_v = _mm256_loadu_si256(cur_ptr.add(x - 1) as *const __m256i);
            let prev_v = if prev_ptr.is_null() {
                zero
            } else {
                _mm256_loadu_si256(prev_ptr.add(x) as *const __m256i)
            };
            let prev_l_v = if prev_ptr.is_null() {
                zero
            } else {
                _mm256_loadu_si256(prev_ptr.add(x - 1) as *const __m256i)
            };
            let out_v = _mm256_or_si256(
                _mm256_or_si256(cur_v, cur_l_v),
                _mm256_or_si256(prev_v, prev_l_v),
            );
            _mm256_storeu_si256(out_ptr.add(x) as *mut __m256i, out_v);
        }
        x += 32;
    }

    while x < width {
        unsafe {
            let top = if prev_ptr.is_null() {
                0
            } else {
                *prev_ptr.add(x)
            };
            let top_left = if prev_ptr.is_null() {
                0
            } else {
                *prev_ptr.add(x - 1)
            };
            *out_ptr.add(x) = *cur_ptr.add(x) | *cur_ptr.add(x - 1) | top | top_left;
        }
        x += 1;
    }
}
