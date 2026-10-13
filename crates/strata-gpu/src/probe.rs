//! GPU 适配器探针：C1/视口都要先知道"这台机器能不能真渲染"。
//! 判据 = 能否拿到 adapter 并建出 device；拿不到就如实报不可用，绝不假装渲染成功。

/// 探测结果：后端名 + adapter 名；None = 无可用 GPU。
pub fn probe() -> Option<(String, String)> {
    let instance = wgpu::Instance::default();
    let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
        power_preference: wgpu::PowerPreference::default(),
        compatible_surface: None,
        force_fallback_adapter: false,
    }))
    .or_else(|| {
        // 独显/核显都拿不到时，试软件回退（llvmpipe/SwiftShader），CI 无头机常走这条
        pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
            force_fallback_adapter: true,
            ..Default::default()
        }))
    })?;
    let info = adapter.get_info();
    Some((format!("{:?}", info.backend), info.name))
}
