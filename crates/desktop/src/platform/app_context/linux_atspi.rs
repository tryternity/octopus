//! Linux AT-SPI2 实现——暂不支持。
//!
//! AT-SPI2 的 DBus 接口与最初实现假设不符：
//! - Accessible.Name 是**属性**（org.freedesktop.DBus.Properties.Get），不是方法调用
//! - AT-SPI2 没有同步获取全局焦点的 API，需要监听 object:state-change:focused 事件流
//!   或从 desktop 根遍历找 STATE_FOCUSED
//! - 对象路径由各应用动态分配，不能硬编码
//!
//! 正确实现需要引入 atspi crate（封装了事件流 + 正确的属性读取），
//! 或完整的 raw DBus 事件监听方案。当前回退到 NullProvider。
//!
//! v2 方向：引入 atspi crate（docs.rs/atspi），订阅 focus 事件，
//! 用 Accessible proxy 正确读 Name 属性 + Text 接口。

#![cfg(target_os = "linux")]

// 当前 Linux 无可用实现，provider() 工厂在 mod.rs 中不会注册 AtspiProvider。
// 此文件保留为设计参考，待 v2 用 atspi crate 实现。
