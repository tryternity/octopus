use super::{
    DbPostProcess,
    box_score::{box_score_fast_pure, contour_score_pure, masked_mean_in_roi},
    contour::dilate_mask_2x2,
    filter::sort_boxes_like_python,
    geometry::min_area_rect_from_points_pure,
    threshold::build_threshold_bitmap,
    unclip::{fill_polygon_mask, unclip_polygon_like_opencv_db},
};
use ndarray::Array2;

#[test]
fn unclip_polygon_like_opencv_db_expands_square() {
    let square = [
        [10.0_f32, 10.0_f32],
        [10.0_f32, 20.0_f32],
        [20.0_f32, 20.0_f32],
        [20.0_f32, 10.0_f32],
    ];
    let out = unclip_polygon_like_opencv_db(&square, 1.6);
    assert_eq!(out.len(), 4);
    for p in out {
        assert!(p[0].is_finite());
        assert!(p[1].is_finite());
    }
}

#[test]
fn contour_score_pure_non_zero_on_filled_region() {
    let mut pred = Array2::<f32>::zeros((10, 10));
    for y in 2..6 {
        for x in 2..6 {
            pred[[y, x]] = 1.0;
        }
    }

    let contour = vec![[2, 2], [2, 5], [5, 5], [5, 2]];
    let s = contour_score_pure(pred.view(), &contour);
    assert!(s > 0.5);
}

#[test]
fn min_area_rect_from_points_pure_handles_simple_quad() {
    let pts = vec![
        [0.0_f32, 0.0_f32],
        [10.0_f32, 0.0_f32],
        [10.0_f32, 5.0_f32],
        [0.0_f32, 5.0_f32],
    ];
    let rect = min_area_rect_from_points_pure(&pts).expect("rect should exist");
    let max_side = rect.size[0].max(rect.size[1]);
    let min_side = rect.size[0].min(rect.size[1]);
    assert!(max_side >= 9.0);
    assert!(min_side >= 4.0);
}

#[test]
fn pure_backend_detects_synthetic_text_blob() {
    let mut pred = Array2::<f32>::zeros((32, 64));
    for y in 8..24 {
        for x in 10..54 {
            pred[[y, x]] = 0.9;
        }
    }

    let post = DbPostProcess {
        thresh: 0.3,
        box_thresh: 0.5,
        ..DbPostProcess::default()
    };

    let (boxes, scores) = post.run(&pred, 640, 320);
    assert!(!boxes.is_empty());
    assert_eq!(boxes.len(), scores.len());
}

#[test]
fn threshold_bitmap_matches_scalar_reference() {
    let data: Vec<f32> = (0..137)
        .map(|i| ((i as f32 * 1.37).sin() * 0.5 + 0.5) * 2.0 - 0.8)
        .collect();
    let pred = Array2::from_shape_vec((1, data.len()), data).expect("shape should match");
    let out = build_threshold_bitmap(pred.view(), 0.3);
    let expected: Vec<u8> = pred
        .iter()
        .map(|v| if *v > 0.3 { 255_u8 } else { 0_u8 })
        .collect();
    assert_eq!(out, expected);
}

#[test]
fn box_score_fast_quad_matches_mask_reference() {
    let mut pred = Array2::<f32>::zeros((27, 39));
    for y in 0..pred.nrows() {
        for x in 0..pred.ncols() {
            pred[[y, x]] = ((x * 7 + y * 13) % 97) as f32 / 97.0;
        }
    }

    let quads = vec![
        vec![[4.2, 3.1], [20.7, 3.0], [21.2, 10.8], [3.8, 11.2]],
        vec![[2.0, 5.0], [14.0, 2.0], [19.0, 13.0], [6.0, 16.0]],
        vec![[0.3, 0.4], [10.8, 0.1], [11.4, 6.9], [0.5, 7.2]],
    ];

    for quad in quads {
        let fast = box_score_fast_pure(pred.view(), &quad);

        let h = pred.nrows() as i32;
        let w = pred.ncols() as i32;
        let mut xmin_f = f32::INFINITY;
        let mut xmax_f = f32::NEG_INFINITY;
        let mut ymin_f = f32::INFINITY;
        let mut ymax_f = f32::NEG_INFINITY;
        for p in &quad {
            xmin_f = xmin_f.min(p[0]);
            xmax_f = xmax_f.max(p[0]);
            ymin_f = ymin_f.min(p[1]);
            ymax_f = ymax_f.max(p[1]);
        }

        let xmin = xmin_f.floor().clamp(0.0, (w - 1) as f32) as i32;
        let xmax = xmax_f.ceil().clamp(0.0, (w - 1) as f32) as i32;
        let ymin = ymin_f.floor().clamp(0.0, (h - 1) as f32) as i32;
        let ymax = ymax_f.ceil().clamp(0.0, (h - 1) as f32) as i32;
        let local_w = (xmax - xmin + 1) as usize;
        let local_h = (ymax - ymin + 1) as usize;

        let mut shifted = Vec::with_capacity(quad.len());
        for p in &quad {
            shifted.push([p[0] - xmin as f32, p[1] - ymin as f32]);
        }
        let mut mask = vec![0_u8; local_w * local_h];
        fill_polygon_mask(&mut mask, local_w, local_h, &shifted);
        let reference = masked_mean_in_roi(
            pred.view(),
            xmin as usize,
            ymin as usize,
            local_w,
            local_h,
            &mask,
        );
        assert!(
            (fast - reference).abs() <= 1e-6,
            "quad score mismatch (mask): fast={fast} ref={reference} quad={quad:?}"
        );
    }
}

#[test]
fn dilate_mask_matches_reference() {
    fn reference(mask: &[u8], width: usize, height: usize) -> Vec<u8> {
        let mut out = vec![0_u8; width * height];
        for y in 0..height {
            for x in 0..width {
                if mask[y * width + x] == 0 {
                    continue;
                }
                for dy in 0..=1 {
                    let ny = y + dy;
                    if ny >= height {
                        continue;
                    }
                    for dx in 0..=1 {
                        let nx = x + dx;
                        if nx >= width {
                            continue;
                        }
                        out[ny * width + nx] = 255;
                    }
                }
            }
        }
        out
    }

    let width = 23usize;
    let height = 17usize;
    let mut mask = vec![0_u8; width * height];
    for y in 0..height {
        for x in 0..width {
            if (x * 3 + y * 5) % 7 == 0 {
                mask[y * width + x] = 255;
            }
        }
    }

    let expected = reference(&mask, width, height);
    let out = dilate_mask_2x2(&mask, width, height);
    assert_eq!(out, expected);
}

#[test]
fn sort_boxes_like_python_orders_by_x_within_same_line() {
    let mut boxes = vec![
        [[10.0, 10.0], [20.0, 10.0], [20.0, 20.0], [10.0, 20.0]],
        [[9.5, 11.0], [19.5, 11.0], [19.5, 21.0], [9.5, 21.0]],
        [[100.0, 40.0], [120.0, 40.0], [120.0, 50.0], [100.0, 50.0]],
    ];
    let mut scores = vec![0.9, 0.8, 0.7];

    sort_boxes_like_python(&mut boxes, &mut scores, 10.0);

    assert!(boxes[0][0][0] <= boxes[1][0][0]);
    assert_eq!(boxes[2][0][0], 100.0);
    assert_eq!(scores.len(), boxes.len());
}

#[test]
fn sort_boxes_like_python_bubble_break_matches_python_non_transitive() {
    // 反例：A(x10,y10) B(x15,y0) C(x22,y5)，阈值 10。
    // 按 (y,x) 预排：B(y0), C(y5), A(y10)。
    // Python 冒泡（非传递，break）：C 与 A 的 Y 差 5≤10 且 C.x22>A.x10 → 把 A 前移到 C 前；
    // 再比 B 与（交换后的）C：Y 差 5≤10 但 B.x15≯C.x22 → break。结果 [B,A,C]。
    // 旧的传递性 line_id 累加会把三者全判同一行再按 x 排 → [A,B,C]，与 Python 不符。
    let mut boxes = vec![
        [[10.0, 10.0], [11.0, 10.0], [11.0, 11.0], [10.0, 11.0]], // A
        [[15.0, 0.0], [16.0, 0.0], [16.0, 1.0], [15.0, 1.0]], // B
        [[22.0, 5.0], [23.0, 5.0], [23.0, 6.0], [22.0, 6.0]], // C
    ];
    let mut scores = vec![0.9, 0.8, 0.7];
    sort_boxes_like_python(&mut boxes, &mut scores, 10.0);
    let xs: Vec<f32> = boxes.iter().map(|b| b[0][0]).collect();
    assert_eq!(xs, vec![15.0, 10.0, 22.0], "expected [B,A,C] got {:?}", boxes);
    assert_eq!(scores.len(), boxes.len());
}
