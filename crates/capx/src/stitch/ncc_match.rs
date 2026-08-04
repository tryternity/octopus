//! NCC（归一化互相关）模板匹配引擎。
//!
//! 拆分自原 stitch.rs（2026-08-04），机械迁移无行为变更。
//! 纯 free function，无 &self 依赖——主匹配/邻帧参考/finalize 都通过这一层。

// ===== NCC 匹配引擎 =====

use imageproc::definitions::Image;
use imageproc::template_matching::{find_extremes, match_template, MatchTemplateMethod};

/// NCC 匹配结果。
pub(crate) struct NccResult {
    /// 最佳偏移（response 坐标，即模板顶部在搜索区域中的 y 偏移）
    pub(crate) best_y: f64,
    /// NCC 分数 [0, 1]，越大越好
    pub(crate) best_score: f64,
    /// 完整 response map
    pub(crate) response: Image<image::Luma<f32>>,
}

/// 主 NCC 结果（大屏两阶段 / 小屏单阶段统一产出）。
pub(crate) enum PrimaryOutcome {
    /// 亚像素 refined_y（原分辨率坐标） + best_score
    Matched(f64, f64),
    /// NCC validate 失败（附 score 供日志/stuck 判断）
    Mismatch(f64),
    /// ncc_match 返回 None（template/search size 不匹配）
    SizeError,
}

/// NCC 匹配：在搜索区域中找模板的最佳对齐位置。
pub(crate) fn ncc_match(
    template: &image::GrayImage,
    search_region: &image::GrayImage,
) -> Option<NccResult> {
    // 模板必须严格小于搜索区域（match_template 的要求）
    if template.width() > search_region.width() || template.height() >= search_region.height() {
        return None;
    }
    let response = match_template(
        search_region,
        template,
        MatchTemplateMethod::CrossCorrelationNormalized,
    );
    let extremes = find_extremes(&response);
    let best_y = extremes.max_value_location.1 as f64;
    let best_score = extremes.max_value as f64;
    Some(NccResult { best_y, best_score, response })
}

/// 保边缘降采样（Triangle 双线性）。NCC+亚像素不能用 Nearest——锯齿破坏 response 峰值。
pub(crate) fn downsample_grayimage(img: &image::GrayImage, scale: f64) -> image::GrayImage {
    let nw = ((img.width() as f64 * scale).max(1.0)).round() as u32;
    let nh = ((img.height() as f64 * scale).max(1.0)).round() as u32;
    image::imageops::resize(img, nw, nh, image::imageops::FilterType::Triangle)
}

/// 限定 y 邻域 [y_min, y_max] 的 NCC + 亚像素 refine（两阶段 stage2 用）。
/// stage1 给出粗 dy_coarse，本函数在原分辨率 ±Npx 内精化，恢复 0.1px 亚像素。
/// 返回 (refined_y 原分辨率坐标, best_score)。范围太小 / size 不匹配 → None。
pub(crate) fn ncc_match_range(
    template: &image::GrayImage,
    search_region: &image::GrayImage,
    y_min: f64,
    y_max: f64,
) -> Option<(f64, f64)> {
    let th = template.height();
    let sh = search_region.height();
    if th >= sh {
        return None;
    }
    let lo = (y_min.max(0.0).floor() as u32).min(sh - th);
    let hi = (y_max.ceil() as u32).saturating_add(th).min(sh);
    if hi <= lo || hi - lo <= th {
        return None;
    }
    let sub = image::imageops::crop_imm(search_region, 0, lo, search_region.width(), hi - lo)
        .to_image();
    let ncc = ncc_match(template, &sub)?;
    let refined_sub = parabolic_refine_from_response(&ncc.response, ncc.best_y);
    Some((refined_sub + lo as f64, ncc.best_score))
}

/// 多道验证 NCC 匹配结果。返回 true 表示匹配可信。
pub(crate) fn validate_ncc_match(
    response: &Image<image::Luma<f32>>,
    _best_y: usize,
    best_score: f32,
    threshold: f32,
) -> bool {
    // 1. 最低分数
    if best_score < threshold {
        return false;
    }

    // 无区分度检测：response 的 max - min 差值 < 0.1 说明所有位置得分几乎相同，
    // NCC 无足够区分力来确定真实偏移（纯色/空白/极低纹理）。拒绝匹配。
    let h = response.height() as usize;
    let mut min_score = f32::MAX;
    let mut max_score = f32::MIN;
    for y in 0..h {
        let v = response.get_pixel(0, y as u32)[0];
        if v < min_score { min_score = v; }
        if v > max_score { max_score = v; }
    }
    if max_score - min_score < 0.1 {
        return false;
    }

    true
}

/// 从 NCC response map 在最佳 y 处做抛物线拟合，返回亚像素偏移。
pub(crate) fn parabolic_refine_from_response(response: &Image<image::Luma<f32>>, best_y: f64) -> f64 {
    let by = best_y as usize;
    if by == 0 || by + 1 >= response.height() as usize {
        return best_y;
    }
    let left = response.get_pixel(0, by as u32 - 1)[0] as f64;
    let center = response.get_pixel(0, by as u32)[0] as f64;
    let right = response.get_pixel(0, by as u32 + 1)[0] as f64;
    let denom = left - 2.0 * center + right;
    if denom.abs() > 1e-10 {
        let delta = 0.5 * (left - right) / denom;
        best_y + delta.clamp(-0.5, 0.5)
    } else {
        best_y
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use super::super::tests::make_frame;
    use crate::stitch::{GrayBuf, StitchConfig, Stitcher, to_feature_map};
    /// 默认测试尺寸：宽度需足够大让 X_START_RATIO..X_END_RATIO 区间有意义，
    /// 高度需远大于 STRIP_H + MAX_SCROLL。
    const TW: u32 = 400;
    const TH: u32 = 600;

    #[test]
    fn test_ncc_matches_known_offset() {
        let f0 = make_frame(TW, TH, 0);
        let f1 = make_frame(TW, TH, 30);
        let canvas_strip = GrayBuf::from_rgba_roi(&f0, (TH - StitchConfig::default().strip_h) as usize, TH as usize);
        let template = canvas_strip.to_gray_image();
        let search_region = GrayBuf::from_rgba_roi(&f1, 0, TH as usize).to_gray_image();
        let result = ncc_match(&template, &search_region);
        assert!(result.is_some(), "NCC 应返回匹配结果");
        let ncc = result.unwrap();
        assert!(ncc.best_score > 0.75, "NCC 分数应 > 0.75: {}", ncc.best_score);
    }

    #[test]
    fn test_ncc_match_range_finds_known_offset() {
        // f0 底部 strip（y∈[TH-strip_h,TH)）在 f1(scroll=30) 中出现在 y=TH-strip_h-30 处
        let strip_h = StitchConfig::default().strip_h;
        let f0 = make_frame(TW, TH, 0);
        let f1 = make_frame(TW, TH, 30);
        let template = GrayBuf::from_rgba_roi(&f0, (TH - strip_h) as usize, TH as usize).to_gray_image();
        let search = GrayBuf::from_rgba_roi(&f1, 0, TH as usize).to_gray_image();
        let expected_y = (TH - strip_h - 30) as f64; // 490
        let (refined_y, score) = ncc_match_range(&template, &search, expected_y - 5.0, expected_y + 5.0)
            .expect("range 内应匹配");
        assert!(
            (refined_y - expected_y).abs() < 2.0,
            "refined_y 应≈{}, 实际 {}", expected_y, refined_y
        );
        assert!(score > 0.5, "range 内匹配 score 应 > 0.5: {}", score);
    }

    #[test]
    fn test_ncc_match_range_rejects_out_of_range_offset() {
        // 真偏移 y=490，range 只给 [0,10] → 返回 range 内峰（≠490），refined_y < 15
        let strip_h = StitchConfig::default().strip_h;
        let f0 = make_frame(TW, TH, 0);
        let f1 = make_frame(TW, TH, 30);
        let template = GrayBuf::from_rgba_roi(&f0, (TH - strip_h) as usize, TH as usize).to_gray_image();
        let search = GrayBuf::from_rgba_roi(&f1, 0, TH as usize).to_gray_image();
        let (refined_y, _) = ncc_match_range(&template, &search, 0.0, 10.0)
            .expect("range 内应有某峰");
        assert!(
            refined_y < 15.0,
            "range 外偏移不应被选, refined_y={}", refined_y
        );
    }

    #[test]
    fn test_two_stage_refine_preserves_subpixel() {
        // 帧宽 TW=400。ncc_downsample_width=9999 → 单阶段；=200 → 两阶段(scale=0.5)。
        // f0 底部 strip 在 f1(scroll=40) 中 y=TH-strip_h-40=480 处。
        // 两阶段与单阶段 refined_y 误差应 < 0.5px（保亚像素）。
        let strip_h = StitchConfig::default().strip_h;
        let f0 = make_frame(TW, TH, 0);
        let f1 = make_frame(TW, TH, 40);
        let canvas_strip = GrayBuf::from_rgba_roi(&f0, (TH - strip_h) as usize, TH as usize);
        let curr_full = GrayBuf::from_rgba_roi(&f1, 0, TH as usize);
        let (tmpl, _) = to_feature_map(&canvas_strip);
        let (search, _) = to_feature_map(&curr_full);

        let s_single = Stitcher::new(f0.clone(), StitchConfig { ncc_downsample_width: 9999, ..Default::default() });
        let s_two = Stitcher::new(f0.clone(), StitchConfig { ncc_downsample_width: 200, ..Default::default() });

        let ry_single = match s_single.primary_ncc(&tmpl, &search, TW) {
            PrimaryOutcome::Matched(y, _) => y,
            _ => panic!("单阶段应匹配成功"),
        };
        let ry_two = match s_two.primary_ncc(&tmpl, &search, TW) {
            PrimaryOutcome::Matched(y, _) => y,
            _ => panic!("两阶段应匹配成功"),
        };
        assert!(
            (ry_two - ry_single).abs() < 0.5,
            "两阶段 refined_y 与单阶段误差应 <0.5px: single={}, two={}", ry_single, ry_two
        );
    }
}
