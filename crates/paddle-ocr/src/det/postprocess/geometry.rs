use crate::Quad;

#[derive(Clone, Copy, Debug)]
pub(super) struct PureRotatedRect {
    pub center: [f32; 2],
    pub size: [f32; 2],
    pub angle: f32,
}

pub(super) fn min_area_rect_from_points_pure(points: &[[f32; 2]]) -> Option<PureRotatedRect> {
    if points.is_empty() {
        return None;
    }

    let hull = convex_hull_like_opencv(points, false);
    if hull.is_empty() {
        return None;
    }

    let mut angle = -std::f64::consts::FRAC_PI_2;

    if hull.len() > 2 {
        let (corner, vec1, vec2) = rotating_calipers_min_area_rect(&hull)?;
        let center = [
            corner[0] + (vec1[0] + vec2[0]) * 0.5,
            corner[1] + (vec1[1] + vec2[1]) * 0.5,
        ];

        let mut width =
            (((vec2[0] as f64 * vec2[0] as f64) + (vec2[1] as f64 * vec2[1] as f64)).sqrt()) as f32;
        let mut height =
            (((vec1[0] as f64 * vec1[0] as f64) + (vec1[1] as f64 * vec1[1] as f64)).sqrt()) as f32;
        let special_case_vertical = vec1[0] == 0.0 && vec1[1] > 0.0;
        if special_case_vertical {
            std::mem::swap(&mut width, &mut height);
        } else {
            angle = -f64::atan2(vec1[0] as f64, vec1[1] as f64);
        }

        let rect = PureRotatedRect {
            center,
            size: [width, height],
            angle: (angle * 180.0 / std::f64::consts::PI) as f32,
        };
        return Some(rect);
    }

    if hull.len() == 2 {
        let p0 = hull[0];
        let p1 = hull[1];

        let center = [(p0[0] + p1[0]) * 0.5, (p0[1] + p1[1]) * 0.5];
        let dx = p0[0] as f64 - p1[0] as f64;
        let dy = p0[1] as f64 - p1[1] as f64;

        let mut width = 0.0_f32;
        let mut height = (dx * dx + dy * dy).sqrt() as f32;
        if dx == 0.0 {
            std::mem::swap(&mut width, &mut height);
        } else if dy < 0.0 {
            angle = f64::atan2(dy, dx);
            std::mem::swap(&mut width, &mut height);
        } else if dy > 0.0 {
            angle = -f64::atan2(dx, dy);
        }

        return Some(PureRotatedRect {
            center,
            size: [width, height],
            angle: (angle * 180.0 / std::f64::consts::PI) as f32,
        });
    }

    Some(PureRotatedRect {
        center: hull[0],
        size: [0.0, 0.0],
        angle: (angle * 180.0 / std::f64::consts::PI) as f32,
    })
}

fn rotating_calipers_min_area_rect(points: &[[f32; 2]]) -> Option<([f32; 2], [f32; 2], [f32; 2])> {
    let n = points.len();
    if n < 3 {
        return None;
    }

    let mut inv_vect_length = vec![0.0_f32; n];
    let mut vect = vec![[0.0_f32; 2]; n];

    let mut left = 0_usize;
    let mut bottom = 0_usize;
    let mut right = 0_usize;
    let mut top = 0_usize;

    let mut left_x = points[0][0];
    let mut right_x = points[0][0];
    let mut top_y = points[0][1];
    let mut bottom_y = points[0][1];

    for i in 0..n {
        let p0 = points[i];
        if p0[0] < left_x {
            left_x = p0[0];
            left = i;
        }
        if p0[0] > right_x {
            right_x = p0[0];
            right = i;
        }
        if p0[1] > top_y {
            top_y = p0[1];
            top = i;
        }
        if p0[1] < bottom_y {
            bottom_y = p0[1];
            bottom = i;
        }

        let p1 = points[(i + 1) % n];
        let dx = p1[0] - p0[0];
        let dy = p1[1] - p0[1];
        vect[i] = [dx, dy];
        let norm = (dx * dx + dy * dy).sqrt();
        if norm <= 1e-12 {
            return None;
        }
        inv_vect_length[i] = 1.0 / norm;
    }

    let mut seq = [bottom, right, top, left];
    let mut min_area = f32::MAX;

    let mut best_left = 0_usize;
    let mut best_bottom = 0_usize;
    let mut best_a1 = 0.0_f32;
    let mut best_b1 = 0.0_f32;
    let mut best_width = 0.0_f32;
    let mut best_height = 0.0_f32;
    let mut found = false;

    for _ in 0..n {
        let rot_vect = [
            vect[seq[0]],
            rotate90_cw(vect[seq[1]]),
            rotate180(vect[seq[2]]),
            rotate90_ccw(vect[seq[3]]),
        ];

        let mut main_element = 0_usize;
        for i in 1..4 {
            if first_vec_is_right(rot_vect[i], rot_vect[main_element]) {
                main_element = i;
            }
        }

        let pindex = seq[main_element];
        let lead_x = vect[pindex][0] * inv_vect_length[pindex];
        let lead_y = vect[pindex][1] * inv_vect_length[pindex];
        let (base_a, base_b) = match main_element {
            0 => (lead_x, lead_y),
            1 => (lead_y, -lead_x),
            2 => (-lead_x, -lead_y),
            3 => (-lead_y, lead_x),
            _ => return None,
        };

        seq[main_element] = (seq[main_element] + 1) % n;

        let mut dx = points[seq[1]][0] - points[seq[3]][0];
        let mut dy = points[seq[1]][1] - points[seq[3]][1];
        let width = dx * base_a + dy * base_b;

        dx = points[seq[2]][0] - points[seq[0]][0];
        dy = points[seq[2]][1] - points[seq[0]][1];
        let height = -dx * base_b + dy * base_a;

        let area = width * height;
        if area <= min_area {
            min_area = area;
            best_left = seq[3];
            best_bottom = seq[0];
            best_a1 = base_a;
            best_b1 = base_b;
            best_width = width;
            best_height = height;
            found = true;
        }
    }

    if !found {
        return None;
    }

    let a1 = best_a1;
    let b1 = best_b1;
    let a2 = -best_b1;
    let b2 = best_a1;

    let c1 = a1 * points[best_left][0] + points[best_left][1] * b1;
    let c2 = a2 * points[best_bottom][0] + points[best_bottom][1] * b2;

    let det = a1 * b2 - a2 * b1;
    if det.abs() <= 1e-12 {
        return None;
    }

    let px = (c1 * b2 - c2 * b1) / det;
    let py = (a1 * c2 - a2 * c1) / det;

    Some((
        [px, py],
        [a1 * best_width, b1 * best_width],
        [a2 * best_height, b2 * best_height],
    ))
}

fn rotate90_ccw(v: [f32; 2]) -> [f32; 2] {
    [-v[1], v[0]]
}

fn rotate90_cw(v: [f32; 2]) -> [f32; 2] {
    [v[1], -v[0]]
}

fn rotate180(v: [f32; 2]) -> [f32; 2] {
    [-v[0], -v[1]]
}

fn first_vec_is_right(vec1: [f32; 2], vec2: [f32; 2]) -> bool {
    let tmp = rotate90_cw(vec1);
    tmp[0] * vec2[0] + tmp[1] * vec2[1] < 0.0
}

pub(super) fn rotated_rect_to_points_pure(rect: PureRotatedRect) -> Quad {
    let angle = f64::from(rect.angle) * std::f64::consts::PI / 180.0;
    let b = (angle.cos() as f32) * 0.5;
    let a = (angle.sin() as f32) * 0.5;

    let ah = a * rect.size[1];
    let aw = a * rect.size[0];
    let bh = b * rect.size[1];
    let bw = b * rect.size[0];

    [
        [rect.center[0] - ah - bw, rect.center[1] + bh - aw],
        [rect.center[0] + ah - bw, rect.center[1] - bh - aw],
        [rect.center[0] + ah + bw, rect.center[1] - bh + aw],
        [rect.center[0] - ah + bw, rect.center[1] + bh + aw],
    ]
}

#[inline]
fn cv_sign(v: f64) -> i32 {
    if v > 0.0 {
        1
    } else if v < 0.0 {
        -1
    } else {
        0
    }
}

#[inline]
fn normalize_vec2(v: [f32; 2]) -> [f32; 2] {
    let n = ((v[0] as f64 * v[0] as f64) + (v[1] as f64 * v[1] as f64)).sqrt();
    if n == 0.0 {
        [0.0, 0.0]
    } else {
        [(v[0] as f64 / n) as f32, (v[1] as f64 / n) as f32]
    }
}

fn sklansky_like_opencv(
    points: &[[f32; 2]],
    pointer: &[usize],
    start: i32,
    end: i32,
    stack: &mut [i32],
    nsign: i32,
    sign2: i32,
) -> usize {
    let incr = if end > start { 1 } else { -1 };
    let mut pprev = start;
    let mut pcur = pprev + incr;
    let mut pnext = pcur + incr;
    let mut stacksize = 3_usize;

    let p_start = points[pointer[start as usize]];
    let p_end = points[pointer[end as usize]];
    if start == end || (p_start[0] == p_end[0] && p_start[1] == p_end[1]) {
        stack[0] = start;
        return 1;
    }

    stack[0] = pprev;
    stack[1] = pcur;
    stack[2] = pnext;

    let end_after = end + incr;
    while pnext != end_after {
        let cury = points[pointer[pcur as usize]][1];
        let nexty = points[pointer[pnext as usize]][1];
        let by = nexty - cury;

        if cv_sign(by as f64) != nsign {
            let pcur_pt = points[pointer[pcur as usize]];
            let pprev_pt = points[pointer[pprev as usize]];
            let pnext_pt = points[pointer[pnext as usize]];

            let mut a = [pcur_pt[0] - pprev_pt[0], pcur_pt[1] - pprev_pt[1]];
            let mut b = [pnext_pt[0] - pcur_pt[0], by];
            a = normalize_vec2(a);
            b = normalize_vec2(b);

            let convexity = (a[1] as f64 * b[0] as f64) - (a[0] as f64 * b[1] as f64);
            if cv_sign(convexity) == sign2 && (a[0] != 0.0 || a[1] != 0.0) {
                pprev = pcur;
                pcur = pnext;
                pnext += incr;
                stack[stacksize] = pnext;
                stacksize += 1;
            } else if pprev == start {
                pcur = pnext;
                stack[1] = pcur;
                pnext += incr;
                stack[2] = pnext;
            } else {
                stack[stacksize - 2] = pnext;
                pcur = pprev;
                pprev = stack[stacksize - 4];
                stacksize -= 1;
            }
        } else {
            pnext += incr;
            stack[stacksize - 1] = pnext;
        }
    }

    stacksize - 1
}

fn convex_hull_like_opencv(points: &[[f32; 2]], clockwise: bool) -> Vec<[f32; 2]> {
    let total = points.len();
    if total == 0 {
        return Vec::new();
    }

    let mut pointer: Vec<usize> = (0..total).collect();
    pointer.sort_by(|&a, &b| {
        points[a][0]
            .total_cmp(&points[b][0])
            .then_with(|| points[a][1].total_cmp(&points[b][1]))
            .then_with(|| a.cmp(&b))
    });

    let mut miny_ind = 0usize;
    let mut maxy_ind = 0usize;
    for i in 1..total {
        let y = points[pointer[i]][1];
        if points[pointer[miny_ind]][1] > y {
            miny_ind = i;
        }
        if points[pointer[maxy_ind]][1] < y {
            maxy_ind = i;
        }
    }

    let mut hullbuf: Vec<i32> = Vec::with_capacity(total);
    let p0 = points[pointer[0]];
    let p_last = points[pointer[total - 1]];
    if p0[0] == p_last[0] && p0[1] == p_last[1] {
        hullbuf.push(0);
    } else {
        let mut tl_buf = vec![0_i32; total + 2];
        let tl_count =
            sklansky_like_opencv(points, &pointer, 0, maxy_ind as i32, &mut tl_buf, -1, 1);
        let mut tr_buf = vec![0_i32; total + 2];
        let tr_count = sklansky_like_opencv(
            points,
            &pointer,
            total as i32 - 1,
            maxy_ind as i32,
            &mut tr_buf,
            -1,
            -1,
        );

        let mut tl_stack = tl_buf[..tl_count].to_vec();
        let mut tr_stack = tr_buf[..tr_count].to_vec();
        if !clockwise {
            std::mem::swap(&mut tl_stack, &mut tr_stack);
        }

        if tl_stack.len() >= 2 {
            for &idx in tl_stack.iter().take(tl_stack.len() - 1) {
                hullbuf.push(idx);
            }
        }
        if tr_stack.len() >= 2 {
            for i in (1..tr_stack.len()).rev() {
                hullbuf.push(tr_stack[i]);
            }
        }
        let stop_idx = if tr_stack.len() > 2 {
            tr_stack[1]
        } else if tl_stack.len() > 2 {
            tl_stack[tl_stack.len() - 2]
        } else {
            -1
        };

        let mut bl_buf = vec![0_i32; total + 2];
        let bl_count =
            sklansky_like_opencv(points, &pointer, 0, miny_ind as i32, &mut bl_buf, 1, -1);
        let mut br_buf = vec![0_i32; total + 2];
        let br_count = sklansky_like_opencv(
            points,
            &pointer,
            total as i32 - 1,
            miny_ind as i32,
            &mut br_buf,
            1,
            1,
        );
        let mut bl_stack = bl_buf[..bl_count].to_vec();
        let mut br_stack = br_buf[..br_count].to_vec();
        if clockwise {
            std::mem::swap(&mut bl_stack, &mut br_stack);
        }

        let mut bl_emit = bl_stack.len();
        let mut br_emit = br_stack.len();
        if stop_idx >= 0 {
            let check_idx = if bl_stack.len() > 2 {
                bl_stack[1]
            } else if bl_stack.len() + br_stack.len() > 2 {
                br_stack[2 - bl_stack.len()]
            } else {
                -1
            };
            if check_idx == stop_idx
                || (check_idx >= 0
                    && stop_idx >= 0
                    && points[pointer[check_idx as usize]][0]
                        == points[pointer[stop_idx as usize]][0]
                    && points[pointer[check_idx as usize]][1]
                        == points[pointer[stop_idx as usize]][1])
            {
                bl_emit = bl_emit.min(2);
                br_emit = br_emit.min(2);
            }
        }

        if bl_emit >= 2 {
            for &idx in bl_stack.iter().take(bl_emit - 1) {
                hullbuf.push(idx);
            }
        }
        if br_emit >= 2 {
            for i in (1..br_emit).rev() {
                hullbuf.push(br_stack[i]);
            }
        }

        for idx in &mut hullbuf {
            *idx = pointer[*idx as usize] as i32;
        }

        let nout = hullbuf.len();
        if nout >= 3 {
            let mut min_idx = 0usize;
            let mut max_idx = 0usize;
            let mut lt = 0_i32;
            for i in 1..nout {
                let idx = hullbuf[i];
                lt += i32::from(hullbuf[i - 1] < idx);
                if lt > 1 && lt <= i as i32 - 2 {
                    break;
                }
                if idx < hullbuf[min_idx] {
                    min_idx = i;
                }
                if idx > hullbuf[max_idx] {
                    max_idx = i;
                }
            }

            let mmdist = (max_idx as i32 - min_idx as i32).unsigned_abs() as usize;
            if (mmdist == 1 || mmdist == nout - 1)
                && (lt <= 1 || lt >= nout.saturating_sub(2) as i32)
            {
                let ascending = (max_idx + 1) % nout == min_idx;
                let i0 = if ascending { min_idx } else { max_idx };
                if i0 > 0 {
                    let mut rotated = vec![0_i32; nout];
                    let mut j = i0;
                    let mut i = 0usize;
                    while i < nout {
                        let curr_idx = hullbuf[j];
                        rotated[i] = curr_idx;
                        let next_j = if j + 1 < nout { j + 1 } else { 0 };
                        let next_idx = hullbuf[next_j];
                        if i < nout - 1 && (ascending != (curr_idx < next_idx)) {
                            break;
                        }
                        j = next_j;
                        i += 1;
                    }
                    if i == nout {
                        hullbuf.copy_from_slice(&rotated);
                    }
                }
            }
        }
    }

    hullbuf
        .into_iter()
        .filter_map(|idx| points.get(idx as usize).copied())
        .collect()
}

fn order_min_box_points_like_python(points: Quad) -> Quad {
    let mut points = points.to_vec();
    points.sort_by(|a, b| a[0].total_cmp(&b[0]));

    let (index_1, index_4) = if points[1][1] > points[0][1] {
        (0_usize, 1_usize)
    } else {
        (1_usize, 0_usize)
    };

    let (index_2, index_3) = if points[3][1] > points[2][1] {
        (2_usize, 3_usize)
    } else {
        (3_usize, 2_usize)
    };

    [
        points[index_1],
        points[index_2],
        points[index_3],
        points[index_4],
    ]
}

pub(super) fn mini_box_from_points_pure(points: &[[f32; 2]]) -> Option<(Quad, f32)> {
    if points.len() < 3 {
        return None;
    }
    let rect = min_area_rect_from_points_pure(points)?;
    let raw = rotated_rect_to_points_pure(rect);
    let ordered = order_min_box_points_like_python(raw);
    let sside = rect.size[0].min(rect.size[1]);
    Some((ordered, sside))
}
