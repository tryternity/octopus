mod overlay_impl;
mod capture;
mod helpers;
pub mod preview_window;

use std::sync::Mutex;
use objc2::rc::Retained;
use objc2_app_kit::NSWindow;

pub fn run(on_complete: Box<dyn FnOnce(Vec<u8>) + Send + 'static>) {
    overlay_impl::start_overlay(on_complete);
}
