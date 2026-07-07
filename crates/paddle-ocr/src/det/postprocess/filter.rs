use crate::Quad;
use crate::vision::numeric::l2;

pub(super) fn filter_det_res(
    dt_boxes: Vec<Quad>,
    scores: Vec<f32>,
    img_height: usize,
    img_width: usize,
) -> (Vec<Quad>, Vec<f32>) {
    let mut out_boxes = Vec::with_capacity(dt_boxes.len());
    let mut out_scores = Vec::with_capacity(scores.len());

    for (box_, score) in dt_boxes.into_iter().zip(scores) {
        let mut box_ = order_points_clockwise(box_);
        box_ = clip_det_res(box_, img_height, img_width);

        let rect_width = l2(box_[0], box_[1]) as i32;
        let rect_height = l2(box_[0], box_[3]) as i32;
        if rect_width <= 3 || rect_height <= 3 {
            continue;
        }

        out_boxes.push(box_);
        out_scores.push(score);
    }

    (out_boxes, out_scores)
}

fn clip_det_res(mut points: Quad, img_height: usize, img_width: usize) -> Quad {
    let max_x = img_width.saturating_sub(1) as f32;
    let max_y = img_height.saturating_sub(1) as f32;

    for p in &mut points {
        p[0] = p[0].clamp(0.0, max_x).floor();
        p[1] = p[1].clamp(0.0, max_y).floor();
    }

    points
}

fn order_points_clockwise(pts: Quad) -> Quad {
    let mut x_sorted = pts.to_vec();
    x_sorted.sort_by(|a, b| a[0].total_cmp(&b[0]));

    let mut left = [x_sorted[0], x_sorted[1]];
    let mut right = [x_sorted[2], x_sorted[3]];
    left.sort_by(|a, b| a[1].total_cmp(&b[1]));
    right.sort_by(|a, b| a[1].total_cmp(&b[1]));

    let tl = left[0];
    let bl = left[1];
    let tr = right[0];
    let br = right[1];
    [tl, tr, br, bl]
}

pub(super) fn sort_boxes_like_python(boxes: &mut Vec<Quad>, scores: &mut Vec<f32>, y_threshold: f32) {
    if boxes.is_empty() {
        return;
    }

    let n = boxes.len();

    // 第一步：按 (y, x) 稳定排序，对齐 Python `sorted(key=lambda x: (x[0][1], x[0][0]))`。
    let mut order: Vec<usize> = (0..n).collect();
    order.sort_by(|&a, &b| {
        boxes[a][0][1]
            .total_cmp(&boxes[b][0][1])
            .then_with(|| boxes[a][0][0].total_cmp(&boxes[b][0][0]))
            .then_with(|| a.cmp(&b))
    });

    // 第二步：复刻 PaddleOCR `tools/infer/predict_system.py` 的 `sorted_boxes`——
    // 相邻插入排序。对每个 i+1，从 i 往前扫相邻对 (j, j+1)：若两者 Y 差 ≤ 阈值
    // （判定同行）且 j+1 在 j 左边（x 更小）就交换，否则 break（前序已有序）。
    // 关键：必须比较相邻 j / j+1，而非固定 i+1。固定 i+1 时 order.swap(j, i+1)
    // 会让 order[i+1] 变成旧 order[j]，下一轮 j-1 比的是错位元素，目标框无法
    // 跨越多位冒泡，同行 N≥3 逆序框排不完全。前两轮误把此 bug 当「上游非传递
    // 行为」并钉测试守护，第三轮审计据 PaddleOCR 官方源码纠偏（j+1 相邻交换，
    // 标准插入排序，完全有序）。
    for i in 0..n.saturating_sub(1) {
        for j in (0..=i).rev() {
            let y_diff = (boxes[order[j + 1]][0][1] - boxes[order[j]][0][1]).abs();
            let x_left = boxes[order[j + 1]][0][0];
            let x_right = boxes[order[j]][0][0];
            if y_diff <= y_threshold && x_left < x_right {
                order.swap(j, j + 1);
            } else {
                break;
            }
        }
    }

    let mut new_boxes = Vec::with_capacity(n);
    let mut new_scores = Vec::with_capacity(n);
    for &src_idx in &order {
        new_boxes.push(boxes[src_idx]);
        new_scores.push(scores[src_idx]);
    }

    *boxes = new_boxes;
    *scores = new_scores;
}
