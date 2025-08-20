use futures::executor::block_on;

#[test]
fn seq_wgsl_compiles() {
    // Try to grab any adapter; prefer CPU (Lavapipe/SwiftShader) if present.
    let instance = wgpu::Instance::default();
    let adapter = block_on(async {
        // First, force fallback adapter which often picks CPU ICDs
        if let Some(a) = instance.request_adapter(&wgpu::RequestAdapterOptions {
            power_preference: wgpu::PowerPreference::LowPower,
            compatible_surface: None,
            force_fallback_adapter: true,
        }).await { Some(a) } else {
            instance.request_adapter(&wgpu::RequestAdapterOptions::default()).await
        }
    }).expect("No wgpu adapter (install mesa-vulkan-drivers and set VK_ICD_FILENAMES)");

    let info = adapter.get_info();
    eprintln!("Using adapter: {:?} / {:?}", info.name, info.device_type);

    // Create device/queue
    let (device, _queue) = block_on(adapter.request_device(&wgpu::DeviceDescriptor {
        label: None,
        required_features: wgpu::Features::empty(),
        required_limits: wgpu::Limits::downlevel_defaults(),
    }, None)).expect("device");

    // Compile shader (works without a real GPU; naga runs on CPU)
    let shader_src = include_str!("../shaders/seq.wgsl");
    device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some("seq-test"),
        source: wgpu::ShaderSource::Wgsl(shader_src.into()),
    });
}