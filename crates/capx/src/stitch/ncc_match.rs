//! NCC（归一化互相关）模板匹配引擎。
//!
//! 拆分自原 stitch.rs（2026-08-04），机械迁移无行为变更。
//! 纯 free function，无 &self 依赖——主匹配/邻帧参考/finalize 都通过这一层。

#[allow(unused_imports)]
use super::*;

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
