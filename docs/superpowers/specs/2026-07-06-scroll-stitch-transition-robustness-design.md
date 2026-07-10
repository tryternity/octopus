# 滚动拼接内容突变鲁棒性改进 — 设计

- 日期：2026-07-06
- 分支：`scroll-stitch-borrow`（从 `feature-0706` 分叉）
- 范围：capx 滚动截图 fallback 链增强 —— 解决「白底黑字文字 → 图片」等内容突变场景的死亡螺旋，**目标成功率 99%**
- 关联文档：
  - 根因分析见本文件 §1
  - 前序 A+B：`docs/superpowers/specs/2026-07-06-scroll-stitch-borrow-A-B-design.md`（A 队列解耦已落地；B 主次比已验证对 NCC 不成立、回退）
  - 借鉴依据：`docs/superpowers/specs/2026-07-06-scroll-stitch-snow-shot-engineering-comparison.md`
- 改动文件：`crates/capx/src/stitch.rs`（`Stitcher` 公共接口零变更）

> **实现状态（2026-07-06 收尾）**：✅ 方向1 已实现合 main（`7cb9bb6`），capx 21 测绿 + e2e 通过（「文字→图片」突变场景拼接完整）。方向3（多锚点双向索引）仍为备选、未实施。
>
> **后续增强（2026-07-10）**：主路径 NCC 由单 Sobel 改为 **Sobel + 灰度双候选**（`stitch.rs::best_ncc_match`）。**诊断修正**：原以为暗色编辑器 sobel 失效（debug build 下 score=0.45 stuck 的误判——实为 debug 性能拖慢消费循环导致丢帧），release 实测暗色场景 sobel 大部分直接 0.99 命中。双候选最终形态：双侧有特征时 Sobel 优先、Sobel 失配再灰度兜底；**任一侧退化（底部 strip 落纯黑空白 = 常数，`max_gradient==0`）时不兜底**（常数模板灰度 NCC 必 score≈1.0 假匹配，release 实测 `dy=-644.4` 重复假帧污染画布），直接交降级链（相邻帧 `prev_gray` 有内容可救）。降级链架构不变；其中第 1 级 `try_match_prev_frame`（相邻帧参考）同样补退化保护——prev 底部 strip 退化（选区下半截恒纯黑）时返回 None 不走灰度，避免常数模板 score≈1.0 假匹配（release 实测 `dy=-247.5` 每帧采纳、滚轮未动画布疯涨）。详见 `docs/features/screenshot.md` §4/§5。
>
> **根因补强（2026-07-10 content_tail）**：上述两处退化保护（`best_ncc_match` + `try_match_prev_frame`）治标——堵住假匹配疯涨，但选区下半截**恒定纯黑**时画布底部 strip 永远常数（`canvas_has=false`）→ 主匹配永远 `Mismatch(0.0)` → `ncc_stuck_count≥5` 直接 `return Ok(false)` **跳过整个降级链** → 滚轮滚动画布不增长（真回归）。根因：canvas-anchored 假设「画布底部=最新有信息内容」，选区底部恒定纯黑时假设崩塌，所有底部 strip 锚定机制（主匹配 + 全部 fallback + stationary）同时失效。根治：新增 `detect_content_tail`（首帧从画布底部往上逐行算灰度 max-min，跳过 `sticky_bottom` 区，连续 ≤30 的无内容行 = 常数尾），与 `sticky_bottom` 同套「裁掉 + finalize 补回」机制——但它不依赖首/次帧逐像素相等（光标/渲染差异致 `sticky_bottom` 漏检纯黑尾），直接看单行内容，更鲁棒。裁掉后画布底部停在真实内容底（有特征），主匹配恢复。回归测试 `test_content_tail_black_bottom_still_stitches`。
>
> **二次根因升级（2026-07-10 content_tail 每帧动态）**：上述 `detect_content_tail` **首帧检测**仍不够——纯黑尾会动态出现（前期内容填满选区无暗尾、滚动后期内容上移露出暗背景时暗尾才出现/增长），首帧 content_tail=0 后期失效 → eff_bottom 不变 → append 带暗尾污染画布底部 → canvas strip 退化 → stuck 死锁（release 实测「拼接一部分后停止」：画布长到 2078px 后卡住，持续 `sobel degenerated canvas_has=false` + `NCC stuck count=5`）。升级：`detect_content_tail` 改**每帧基于当前帧**检测（非首帧画布缓存），eff_bottom 每帧动态止于真实内容底，append 永不带暗尾。另加**亮度判定**：行需 max-min≤30 **且** 最亮 luma<`CONTENT_TAIL_MAX_LUMA`(40) 才算暗尾——纯 max-min 会误判高 luma 低对比渐变行（每行常数但亮，如 make_frame 渐变底部）为纯黑尾。54 测绿（+`test_detect_content_tail_frame_based` / `test_content_tail_updates_each_frame` 2 测）。
>
> **三次根因升级（2026-07-10 strip 自适应）**：content_tail 每帧动态仍不够——`detect_content_tail` 的 clamp `min_keep=strip_h*3=240`，当选区物理高 < 240（如 162px 含 80px 暗尾）时 `max_tail = fh - 240 ≤ 0` → **content_tail 强制为 0**，暗尾裁不掉 → 画布底部 strip 落暗尾 → `canvas_has=false` 首帧即死锁（release 实测「滚动没拼接」：finalize 只拼 210 行，画布几乎不增长）。双重根因：①固定 `strip_h=80` 对矮选区（content_h=82）太大，裁掉暗尾后 ROI 仅 82px、strip 吃掉 98%、搜索范围≈2px；②`*3` clamp 反过来阻止裁剪。根治：strip 改**按 content_h 自适应** `eff_strip_h = min(strip_h, content_h/3).max(MIN_STRIP=8)`（新增字段 `eff_strip_h`，每帧基于 content_h 更新，模板提取 + 匹配几何 + 降级链统一读它而非 `config.strip_h`），留 2/3 作搜索范围；**移除** `detect_content_tail` 的 `*3` clamp（自适应后 content_h=3*strip 天然满足，整帧纯黑退化由 `eff_bottom<=eff_top` 兜底）；加 ROI<eff_strip 跳帧防御（防 `quick_stationary_check` 越界 panic——sticky_top+content_tail 几乎吃光整帧时 ROI 可低至 2 行）。正常选区（content_h≥240）eff_strip_h=80 零变化。55 测绿（+`test_short_selection_with_dark_tail_stitches`）。
>
> **四次根因升级（2026-07-10 画布种子暗尾）**：strip 自适应仍不够——init 裁画布用的是 `self.content_tail`（=**当前第二帧**的暗尾），却裁**首帧**画布。首帧在 app 聚焦/滚动开始前由 setup 单独捕获（`screenshot_commands.rs:1146`），暗尾常大于已滚动后的第二帧（内容上移、暗尾缩小）；用第二帧小暗尾裁首帧大暗尾 → 残余暗尾留画布底部 → canvas strip 常数 → `canvas_has=false` 首帧即死锁（release 实测 296×160 矮选区「滚动不拼接」：画布全程不增长，finalize 只拼 170 行）。根治：init 改读**画布种子缓冲自身**的暗尾——抽出 `scan_content_tail_in(buf, h)`（帧/画布缓冲共用检测核心），init 用 `scan_content_tail_in(&self.canvas_buf, canvas_h)` 测首帧自身暗尾裁剪，保证画布底部停在首帧真实内容底；每帧 curr ROI 仍读当前帧（`detect_content_tail`）。56 测绿（+`test_seed_dark_tail_trimmed_by_own_measurement`，构造首帧暗尾 100 > 第二帧暗尾 40，断言画布按种子自身裁到 60 而非 120）。**教训四升级**：content_tail「每帧基于当前帧」只解决 curr ROI 侧；画布种子侧的首帧是另一个独立输入，必须用其**自身**缓冲测暗尾——裁剪对象与检测对象必须是同一帧。
>
> **五次根因升级（2026-07-10 画布死锚恢复 reseed）**：种子暗尾用首帧自身检测仍不够——它前提是首帧"有内容、只是带暗尾"。但首帧在 app **聚焦/前置之前**捕获时可能是**整帧空白**（捕到桌面/未抬升内容/排除 overlay 后的空洞）：content_tail 无暗尾可裁（整帧常数），画布底部永远常数 → canvas-anchored 锚点永久死锁（每帧 `canvas_has=false`，画布不增长，finalize 只拼残余）。日志时序铁证：`activated app for scroll focus` 出现在首条 stitch 日志**之后**（app 聚焦在首帧拼接之后才完成）。根治：**死锚恢复**——确认画布有内容前（`canvas_content_confirmed` 一次性闸门，置位后终身跳过、零稳态开销）每帧用 `canvas_bottom_constant()`（采样画布底部 strip max-min < 阈值）检测锚点是否常数；常数则 `reseed_canvas_from(frame, eff_top, eff_bottom)` 用当前帧内容区重建画布、重置 dy_history/stuck，首个内容帧到达即恢复。这是 canvas-anchored 架构的兜底：无论锚点为何变常数（种子空白 / 异常裁剪 / 边缘 case）都从当前内容帧重建，而非永久死锁。57 测绿（+`test_blank_seed_reseeded_from_content_frame`）。**教训五升级**：前四轮都在"防锚点变常数"（content_tail/strip 自适应/种子自身暗尾），但首帧整帧空白是无法预防的（app 聚焦时序不可控）；须补"锚点已变常数则恢复"的兜底——canvas-anchored 架构必须假设锚点可能失效并能自愈。init 加 `seed_constant` 诊断日志便于定位。

> **六次根因升级（2026-07-10 画布常数尾每帧自愈 trim）**：第五轮的 `canvas_content_confirmed` **一次性闸门**本身成了新死锁源——它"确认有内容后终身跳过死锚检查"。但滚动中画布底部会【再次】变常数（reseed/前五轮都只覆盖首帧死锚）：滚到内容末尾露出纯色背景、或 1D 假匹配（`fallback: 1D projection match dy=-171 conf=1.0`）append 了常数块。底部再次常数 + 闸门已置位 → `best_ncc_match` Sobel 退化 `Mismatch(0.0)` → `ncc_stuck_count≥5` → `Ok(false)` stationary **永久死锁**到 finalize（release 实测画布长到 1140px 后卡住 5 秒，`sobel degenerated canvas_has=false` 重复；finalize 灰度兜底对常数画布 score≈1.0 假匹配 `stitching remaining 356 rows` 拼错）。根治：**删除一次性闸门，改每帧自愈**——每帧 `canvas_bottom_constant()` 轻量判定；常数则 `scan_canvas_constant_tail()`（逐行往上累加抽样像素**运行 min/max**，diff≥阈值即停；运行 min/max 而非单行 max-min，防垂直渐变——每行横向常数但行间亮度递增、有 Sobel 垂直梯度——被误判常数）测常数尾 `tail`：裁后仍 ≥ `keep_min`(eff_strip_h) → **非破坏性 `truncate` 常数尾**（只丢空白/纯色，不丢内容，`canvas_h -= tail`，重置 stuck/best-guess），锚点回到真实内容底、本帧继续匹配；仅画布几乎全常数（无内容可留）才 `reseed_canvas_from` 重建（破坏性，保留第五轮逻辑）。58 测绿（+`test_canvas_constant_tail_trimmed_mid_stream`：拼接增长后注入 150 行常数尾模拟污染 → 下一帧裁尾自愈继续拼接，非死锁）。**教训六升级**：第五轮"恢复"用一次性闸门是错的——死锚不只首帧一次，滚动中可反复出现；canvas-anchored 的锚点维护必须是**每帧持续**的（trim 优先非破坏、reseed 兜底破坏），而非"确认一次就不管"。运行 min/max 的渐变识别也适用其他"常数检测"场景。

---

## 1. 背景与根因

e2e 暴露：「文字 → 图片」视觉突变场景拼接易**一断到底**（滚动到一半停止拼接）。确诊根因在 `stitch.rs` fallback 死亡螺旋，**与 A 队列解耦无关**（main 用同一套 `stitch.rs`，同病——成功靠突变帧 dy 恰好≈历史中位数的运气）。

死亡螺旋四步：

1. **突变帧 NCC 失配**：画布底部 80px strip（旧内容=文字）与当前帧同位置（新内容=图片）归一化互相关 score 0.51 < `NCC_SCORE_THRESHOLD` 0.65（`stitch.rs:334`）
2. **1D 投影 fallback 失败**：文字行投影 vs 图片行投影，SAD 大、置信度 < 0.25 → `None`（`try_match_1d_projection:637`）
3. **best-guess 盲 append**：`estimate_dy_hint` 取 `dy_history` 中位数（如 -18），`apply_fallback_match`（`:647`）**无正确性校验**直接把当前帧底部 new_rows 行接进画布：
   - dy 偏离时（突变瞬间手速变化）→ append 错位 → **画布底部被污染成错位片段**
4. **熔断后永久放弃**：best-guess 连错 3 次触发 `streak >= 3`（`:479`）→ 之后每帧走到 "all fallbacks exhausted, skipping"；`streak` 仅 NCC 成功才重置，而画布已被污染 NCC 再也不会成功 → **画布永久卡在突变点，之后内容全丢**

**致死双因**：① best-guess 盲 append 污染画布底部模板 ② 熔断后永久放弃（无自愈路径）。

---

## 2. 目标 / 非目标

**目标**
- 突变场景成功率 → **99%**（产品级门槛）
- 不丢突变过渡区内容
- append 必须基于真实匹配（非盲猜），不污染画布
- 保留 canvas-anchored 主路径（亚像素精度、不漂移、现有测试不回归）

**非目标**
- 不做双向滚动（向上滚也拼接）—— 方向 3 备选范畴，本阶段不做
- 不替换 NCC 主匹配算法（亚像素精度、降级链、测试覆盖保持）
- 不动 `Stitcher` 公共接口（`process_frame`/`finalize`/`canvas` 等签名不变）

---

## 3. 主方案：相邻帧参考 fallback（方向 1）

NCC 失配且非静止时，在 1D 投影 fallback **之前**插入一层「前一帧参考」匹配：

```
process_frame:
  NCC（画布底部 strip 锚定）          ← 主路径，完全不变
    失配 ↓
  相邻帧 NCC（前一帧有效区当模板，匹配当前帧）  ← 新增 fallback 层
    失配 ↓
  1D 投影 → best-guess（现有链，不动）
```

### 3.1 为何有效

前一帧已正确 append 进画布，与当前帧只差一个 dy；**突变边界在相邻两帧都存在，重叠区最大**。用前一帧匹配当前帧，突变边界成为共同特征，NCC 能锁定正确 dy → append 正确内容 → 画布不被污染 → 下一帧 canvas-anchored NCC 自愈。

把突变帧从「靠运气（dy≈中位数才成功）」变成「能算出正确 dy 的普通帧」。

### 3.2 数据与接口

- `Stitcher` 加字段 `prev_gray: Option<GrayBuf>`（前一帧有效区灰度，与 `curr_gray` 同来源 `GrayBuf::from_rgba_roi`）
- 每帧 `process_frame` 末尾（return 前）更新 `prev_gray = Some(curr_gray.clone())`
- 新增私有方法 `try_match_prev_frame(&self, prev_gray, curr_gray, eff_top, eff_bottom) -> Option<NccResult>`：复用 `ncc_match`，模板取 `prev_gray` 全有效区（或其底部 strip），搜索区取 `curr_gray`
- `try_fallback` 在 1D 投影之前插入此层；命中 → `apply_fallback_match`（复用现有 append + dy 校验，此时 dy 来自真实匹配非盲猜）

### 3.3 关键约束

- 相邻帧 NCC **只在主 NCC 失配时触发**，正常帧零开销
- 命中后走 `apply_fallback_match`，dy 校验（`min_scroll_px` ≤ new_rows < 90% ROI）照常，不通过则继续降级
- 未命中 → 继续走 1D → best-guess（best-guess 仍是最后兜底，但相邻帧层已拦截绝大多数突变）
- `prev_gray` 内存：有效区灰度 ~width×height 字节（1440×900 ≈ 1.3MB）/录制，可接受

---

## 4. 备选方案：多锚点双向索引（方向 3，不实施）

> **2026-07-06 收口**：用户确认不支持双向滚动（大部分场景向下滚动）；单向 + 方向 1 突变鲁棒（相邻帧 fallback）+ D 大屏两阶段 refine 已覆盖产品需求。方向 3 正式定为**不实施**，除非未来出现明确的双向滚动需求。下方内容保留作设计参考。

借鉴 snow-shot：top + bottom 内容列表 + FAST 角点（fast12/9 自适应）+ 行/列池化描述子 + `hora` HNSW 近邻索引 + 偏移投票 + `try_rollback` 反向。

### 4.1 何时考虑

- 方向 1 验证后突变场景仍 < 99%
- 或需要双向滚动（向上滚也拼接，`dy>0` 不再 skip）

### 4.2 为何暂缓

- 替换匹配核心栈：引入 `hora`/`rayon` 依赖，重写角点提取/描述子/索引/投票，~500+ 行新代码
- 牺牲现有优势：亚像素精度（角点投票是整数像素）、~15 个合成帧测试覆盖、不 panic 降级纪律需重建
- 回归风险高；单向突变场景下 ROI 远低于方向 1

### 4.3 与方向 1 的关系（非互斥）

方向 1 是给现有 NCC 加 fallback 层，方向 3 是替换匹配核心。若未来上方向 3，方向 1 的相邻帧 fallback 层可保留作「角点法之外的第二兜底」，两者递进而非二选一。

---

## 5. 方向 1 vs 方向 3 预期效果对比

| 维度 | 方向 1 相邻帧参考 | 方向 3 多锚点双向索引 |
|---|---|---|
| 突变场景 | ✅ 根治（相邻帧重叠最大，突变边界共同特征，求出正确 dy 不盲 append） | ✅ 强（FAST 角点局部特征对内容类型不敏感，重叠区角点投票） |
| 正常场景 | 不变（canvas-anchored NCC + 亚像素精度全保留） | 算法栈替换，亚像素精度需用角点重做（投票整数像素，精度下降） |
| 低纹理/纯色 | 继承现有 1D 投影 fallback | 角点稀少 → 需额外 fallback 兜底 |
| 双向滚动 | ❌ 仍单向（`dy>0` skip） | ✅ 原生（top/bottom list + try_rollback） |
| 工程量 | ~几十行，+1 字段 +1 fallback 层，纯增量 | ~500+ 行，新依赖（hora/rayon），重写匹配核心 + 重建测试 |
| 回归风险 | 低（不动主路径） | 高（替换核心算法） |
| 到 99% 把握 | 高（消除致死双因：盲 append + 永久熔断） | 高，但代价不成比例 |
| 关系 | 主方案 | 备选；与方向 1 不互斥 |

**量化判断**：
- 方向 1：突变帧从「靠运气（~75%）」→「求出正确 dy」，预计突变场景 → 95%+；剩余边缘 case（连续多帧失配、前一帧也无特征）由现有 best-guess 兜底，整体逼近 99%
- 方向 3：突变 + 双向理论均强，但单向突变场景下 ROI 远低于方向 1（~10× 工程量、精度下降、回归风险高）

---

## 6. 测试

| 项 | 做法 |
|---|---|
| **相邻帧 fallback 单测**（stitch.rs `#[cfg(test)]`） | 构造「文字帧序列 → 图片帧」过渡的合成帧：① 主 NCC 失配时相邻帧 NCC 命中、dy 正确、append 正确；② 画布底部不被污染（append 后底部 strip 与真实内容一致）；③ 不触发 best-guess 熔断卡死 |
| **边缘 case** | 前一帧也失配（prev_gray 不可信）→ 退化到 1D/best-guess，行为不劣于现状 |
| **回归** | 现有 stitch.rs 全部合成帧测试守护主路径（canvas-anchored NCC）不变；A 队列解耦的 e2e 行为不回归 |

---

## 7. 风险

- **相邻帧也失配的递归**：概率低（前一帧通常 NCC 成功 append）；发生时退化到现有 best-guess，不比现状差
- **失配帧 NCC 计算翻倍**：仅 fallback 路径，正常帧无开销；失配本就是少数帧
- **`prev_gray` 内存**：~1.3MB/录制（1440×900 有效区灰度），可接受
- **模板取全有效区 vs 底部 strip**：实现时取 `prev_gray` 底部 strip（与 canvas strip 同源同尺寸）匹配 curr，复用 `ncc_match` 路径，避免新写匹配逻辑

---

## 8. 后续增强（2026-07-07）：fallback 2D 反向验证

本 spec 的 prev_frame fallback（方向1）缓解「内容突变」失配——但 **1D 投影 / best-guess 在 prev_frame 也失配时仍可能盲 append**（见 §2 致死双因 ①，本 spec 未根治此路径）。2026-07-07 补一层根治：

- **`verify_alignment_2d`**：1D/best-guess 追加画布前，按候选 dy 算重叠区 `[crop_y-verify_rows, crop_y)`（紧贴 crop 区上方的已见内容）的 2D 抽样 SAD vs 画布底部 strip，超 `FALLBACK_VERIFY_SAD`（默认 15.0）→ 拒绝追加、skip 该帧，靠 Canvas-Anchored 下一帧从画布底部恢复匹配。
- **直接堵住** 1D 行投影对图文混排的假匹配污染（实测 log：图文长页 y≈2520 处错位，1D 给 `conf=0.3574` 弱匹配被 `apply_fallback_match` 无条件采纳所致）。
- **prev_frame 路径 `verify=false`**——其 dy 已过内部 `validate_ncc_match`，且上一帧 skip 时 `prev≠画布底部` 会让本验证误杀这根救命稻草。
- 接入点 `apply_fallback_match`（`stitch.rs`），prev_frame/1D/best-guess 三处统一过验证；`finalize` 已有 NCC validate 不动。阈值 15.0 起步，留 reject 日志便于线上标定。详见 architecture.md `stitch` 行。
