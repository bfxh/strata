//! UI 视觉 token 与可访问性（C2 对比度门）。
//!
//! 本 crate 只放**纯数据**：颜色 token、WCAG 对比度数学、玻璃合成判据。不含任何
//! wgpu/egui 依赖（纯 std），因此可无头跑门、可进 no_std 化的 Python 校验路径。
//! 真正的像素级 golden-image 断言属 M3 视口接线；这里先把"合成对比度 ≥ 阈值"这条
//! 硬底线（04 文档）钉成会判红的数据门。

pub mod contrast;
pub mod tokens;
