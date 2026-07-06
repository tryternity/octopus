use geo_clipper::{ClipperInt, EndType as ClipperEndType, JoinType as ClipperJoinType};
use geo_types::{Coord, LineString, Polygon};

use crate::Quad;
#[cfg(test)]
use crate::vision::numeric::l2;

pub(super) fn unclip_polygon_pyclipper_into(in_poly: &[[f32; 2]], unclip_ratio: f32, out: &mut Vec<[f32; 2]>) {
    out.clear();
    if in_poly.len() < 3 {
        return;
    }

    let area = polygon_area_f64(in_poly).abs();
    let length = polygon_perimeter_f64(in_poly);
    if !area.is_finite() || !length.is_finite() || length <= 1e-6 {
        return;
    }

    let distance = area * f64::from(unclip_ratio) / length;
    if !distance.is_finite() {
        return;
    }

    let mut ring: Vec<Coord<i64>> = in_poly
        .iter()
        .map(|p| Coord {
            x: (p[0] as f64).trunc() as i64,
            y: (p[1] as f64).trunc() as i64,
        })
        .collect();
    if ring.len() < 3 {
        return;
    }

    if ring.first() != ring.last() {
        ring.push(*ring.first().unwrap_or(&ring[0]));
    }

    let poly = Polygon::new(LineString::from(ring), vec![]);
    let expanded = poly.offset(
        distance,
        ClipperJoinType::Round(0.25),
        ClipperEndType::ClosedPolygon,
    );

    for polygon in expanded.0 {
        let ext = polygon.exterior();
        let ext_len = ext.0.len();
        let ext_end = if ext_len > 1 && ext.0.first() == ext.0.last() {
            ext_len - 1
        } else {
            ext_len
        };
        for c in ext.0.iter().take(ext_end) {
            out.push([c.x as f32, c.y as f32]);
        }

        for hole in polygon.interiors() {
            let h_len = hole.0.len();
            let h_end = if h_len > 1 && hole.0.first() == hole.0.last() {
                h_len - 1
            } else {
                h_len
            };
            for c in hole.0.iter().take(h_end) {
                out.push([c.x as f32, c.y as f32]);
            }
        }
    }
}

pub(super) fn scale_box_to_dest(
    box_points: &mut Quad,
    bitmap_w: usize,
    bitmap_h: usize,
    dest_w: usize,
    dest_h: usize,
) {
    if bitmap_w == 0 || bitmap_h == 0 {
        return;
    }

    let bw = bitmap_w as f32;
    let bh = bitmap_h as f32;
    let dw = dest_w as f32;
    let dh = dest_h as f32;

    for p in box_points {
        p[0] = (p[0] / bw * dw).round_ties_even().clamp(0.0, dw);
        p[1] = (p[1] / bh * dh).round_ties_even().clamp(0.0, dh);
    }
}

#[cfg(test)]
pub(super) fn unclip_polygon_like_opencv_db(in_poly: &[[f32; 2]], unclip_ratio: f32) -> Vec<[f32; 2]> {
    if in_poly.len() < 3 {
        return Vec::new();
    }

    let area = polygon_area(in_poly).abs();
    let length = polygon_perimeter(in_poly);
    if length <= 1e-6 {
        return Vec::new();
    }

    let distance = area * unclip_ratio / length;

    let n = in_poly.len();
    let mut new_lines: Vec<[[f32; 2]; 2]> = Vec::with_capacity(n);
    for i in 0..n {
        let pt1 = in_poly[i];
        let pt2 = in_poly[(i + n - 1) % n];

        let vec = [pt1[0] - pt2[0], pt1[1] - pt2[1]];
        let vec_norm = (vec[0] * vec[0] + vec[1] * vec[1]).sqrt();
        if vec_norm <= 1e-6 {
            return Vec::new();
        }

        let unclip_dis = distance / vec_norm;
        let rotate_vec = [vec[1] * unclip_dis, -vec[0] * unclip_dis];

        new_lines.push([
            [pt1[0] + rotate_vec[0], pt1[1] + rotate_vec[1]],
            [pt2[0] + rotate_vec[0], pt2[1] + rotate_vec[1]],
        ]);
    }

    let mut out = Vec::with_capacity(n);
    for i in 0..n {
        let a = new_lines[i][0];
        let b = new_lines[i][1];
        let c = new_lines[(i + 1) % n][0];
        let d = new_lines[(i + 1) % n][1];

        let v1 = [b[0] - a[0], b[1] - a[1]];
        let v2 = [d[0] - c[0], d[1] - c[1]];
        let v1n = (v1[0] * v1[0] + v1[1] * v1[1]).sqrt();
        let v2n = (v2[0] * v2[0] + v2[1] * v2[1]).sqrt();

        if v1n <= 1e-6 || v2n <= 1e-6 {
            out.push([(b[0] + c[0]) * 0.5, (b[1] + c[1]) * 0.5]);
            continue;
        }

        let cos_angle = (v1[0] * v2[0] + v1[1] * v2[1]) / (v1n * v2n);
        if cos_angle.abs() > 0.7 {
            out.push([(b[0] + c[0]) * 0.5, (b[1] + c[1]) * 0.5]);
            continue;
        }

        let denom = a[0] * (d[1] - c[1])
            + b[0] * (c[1] - d[1])
            + d[0] * (b[1] - a[1])
            + c[0] * (a[1] - b[1]);
        if denom.abs() <= 1e-6 {
            out.push([(b[0] + c[0]) * 0.5, (b[1] + c[1]) * 0.5]);
            continue;
        }

        let num = a[0] * (d[1] - c[1]) + c[0] * (a[1] - d[1]) + d[0] * (c[1] - a[1]);
        let s = num / denom;

        out.push([a[0] + s * (b[0] - a[0]), a[1] + s * (b[1] - a[1])]);
    }

    out
}

#[cfg(test)]
fn polygon_area(poly: &[[f32; 2]]) -> f32 {
    if poly.len() < 3 {
        return 0.0;
    }

    let mut sum = 0.0_f32;
    for i in 0..poly.len() {
        let p0 = poly[i];
        let p1 = poly[(i + 1) % poly.len()];
        sum += p0[0] * p1[1] - p1[0] * p0[1];
    }
    0.5 * sum
}

fn polygon_area_f64(poly: &[[f32; 2]]) -> f64 {
    if poly.len() < 3 {
        return 0.0;
    }

    let mut sum = 0.0_f64;
    for i in 0..poly.len() {
        let p0 = poly[i];
        let p1 = poly[(i + 1) % poly.len()];
        sum += f64::from(p0[0]) * f64::from(p1[1]) - f64::from(p1[0]) * f64::from(p0[1]);
    }
    0.5 * sum
}

#[cfg(test)]
fn polygon_perimeter(poly: &[[f32; 2]]) -> f32 {
    if poly.is_empty() {
        return 0.0;
    }

    let mut length = 0.0_f32;
    for i in 0..poly.len() {
        length += l2(poly[i], poly[(i + 1) % poly.len()]);
    }
    length
}

fn polygon_perimeter_f64(poly: &[[f32; 2]]) -> f64 {
    if poly.is_empty() {
        return 0.0;
    }

    let mut length = 0.0_f64;
    for i in 0..poly.len() {
        let p0 = poly[i];
        let p1 = poly[(i + 1) % poly.len()];
        let dx = f64::from(p0[0] - p1[0]);
        let dy = f64::from(p0[1] - p1[1]);
        length += (dx * dx + dy * dy).sqrt();
    }
    length
}

pub(super) fn fill_polygon_mask(mask: &mut [u8], width: usize, height: usize, poly: &[[f32; 2]]) {
    if poly.len() < 3 || width == 0 || height == 0 || mask.is_empty() {
        return;
    }
    if mask.len() != width * height {
        return;
    }
    mask.fill(0);

    let mut vertices = Vec::with_capacity(poly.len());
    let mut min_y = i32::MAX;
    let mut max_y = i32::MIN;
    for p in poly {
        let x = p[0] as i32;
        let y = p[1] as i32;
        min_y = min_y.min(y);
        max_y = max_y.max(y);
        vertices.push([x, y]);
    }
    if vertices.len() < 3 {
        return;
    }

    let y_start = min_y.max(0).min(height as i32 - 1);
    let y_end = max_y.max(0).min(height as i32 - 1);
    if y_start > y_end {
        return;
    }

    let mut intersections = Vec::<f32>::with_capacity(vertices.len());
    for y in y_start..=y_end {
        intersections.clear();

        let mut prev = vertices[vertices.len() - 1];
        for &curr in &vertices {
            let (x0, y0) = (prev[0], prev[1]);
            let (x1, y1) = (curr[0], curr[1]);

            if (y0 <= y && y < y1) || (y1 <= y && y < y0) {
                let dy = (y1 - y0) as f32;
                if dy.abs() > f32::EPSILON {
                    let t = (y - y0) as f32 / dy;
                    intersections.push(x0 as f32 + (x1 - x0) as f32 * t);
                }
            }
            prev = curr;
        }

        if intersections.len() < 2 {
            continue;
        }
        intersections.sort_by(|a, b| a.total_cmp(b));

        let row_off = y as usize * width;
        let mut i = 0usize;
        while i + 1 < intersections.len() {
            let xs = intersections[i].ceil() as i32;
            let xe = intersections[i + 1].floor() as i32;
            if xs <= xe {
                let x0 = xs.max(0).min(width as i32 - 1) as usize;
                let x1 = xe.max(0).min(width as i32 - 1) as usize;
                if x0 <= x1 {
                    unsafe {
                        std::ptr::write_bytes(
                            mask.as_mut_ptr().add(row_off + x0),
                            1_u8,
                            x1 - x0 + 1,
                        );
                    }
                }
            }
            i += 2;
        }
    }
}
