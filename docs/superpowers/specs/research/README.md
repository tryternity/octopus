# 调研与对比文档

竞品调研、工程对比类文档——描述"为什么这么选/不选"（决策溯源），不属于 features（"当前是什么"），也未落地为独立 feature，故从 `specs/` 主目录移出归档于此。衍生出的已落地 feature 见 [`docs/features/`](../../../features/)。

- [`2026-07-06-scroll-stitch-snow-shot-engineering-comparison.md`](./2026-07-06-scroll-stitch-snow-shot-engineering-comparison.md) — octopus（NCC 模板匹配）vs snow-shot（FAST 角点 + HNSW 投票）拼接引擎工程对比；衍生的队列解耦 / 配置字段化 / 两阶段 refine 已落地，见 [features/screenshot.md](../../../features/screenshot.md)
- [`2026-07-07-launcher-survey.md`](./2026-07-07-launcher-survey.md) — Wox / Raycast / Alfred 启动器功能矩阵调研（AI 命令面板、剪贴板联动等方向的选型参考）
- [`2026-07-08-popclip-survey.md`](./2026-07-08-popclip-survey.md) — PopClip / SnipDo / Click to Do / OnText 选中操作工具调研（action bar 的选型参考，落地见 [features/desktop-app.md](../../../features/desktop-app.md) §12）
