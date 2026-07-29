//! screenshot Tauri 命令层（区域截图 + 滚动截图）。
//!
//! 2026-07-30 起拆分为子模块。mod.rs 仅声明子模块 + glob re-export
//! （`pub(crate) use submodule::*`）保持 `crate::screenshot_commands::xxx` 路径不变。
//! 子模块：`shared`（跨域平台 helper）/ `scroll`（滚动截图）/ `area`（区域截图）。

mod shared;
pub(crate) use shared::*;

mod scroll;
pub(crate) use scroll::*;

mod area;
pub(crate) use area::*;

