//! GPU compute pipelines that close the Vello CMYK-overprint parity
//! gap. The Vello backend renders the scene as today, then the work in
//! this module composes per-channel max overprint (process and spot
//! ink) against parallel plane state held in storage buffers.
//!
//! See `crates/paged-gpu/src/cpu.rs` (`compose_cmyk_overprint_via_planes`,
//! `compose_spot_overprint_via_plane`, and `naive_cmyk_to_rgb_8bit`) for
//! the bit-stable CPU reference; the two shaders here (`splat_or_overprint`
//! and `recomposite`) mirror that arithmetic in u32-lane packed form to
//! avoid the non-portable `read_write` Rgba8Unorm storage extension.
//!
//! Pipelines are created once per `GpuState` and reused; per-render
//! resources (planes, scratch buffers, bind groups) are allocated on
//! every `rasterize()` call and dropped after readback. This matches
//! the existing Vello `init_gpu` lifetime contract.
//!
//! Pipeline-creation failure is *not* fatal — the Vello backend logs
//! the error and falls back to the pre-parity knockout behaviour. The
//! `vello_compute_pipeline_creation_failure_falls_back_to_knockout`
//! test in `vello_rs.rs` pins this contract.

use std::num::NonZeroU64;

/// Source for `splat_or_overprint.wgsl`. Inlined at compile time so
/// the binary doesn't carry a runtime file-load path.
pub const SPLAT_OR_OVERPRINT_WGSL: &str =
    include_str!("shaders/splat_or_overprint.wgsl");

/// Source for `recomposite.wgsl`. See the module-level `recomposite`
/// docstring for the pixel-by-pixel composite rule.
pub const RECOMPOSITE_WGSL: &str = include_str!("shaders/recomposite.wgsl");

/// Sentinel `spot_id` value indicating a process-ink dispatch (writes
/// into the four CMYK planes rather than a spot plane). Matches the
/// shader's `0xFFFFFFFFu` test.
pub const NO_SPOT_SENTINEL: u32 = 0xFFFF_FFFFu32;

/// Parameters pushed to `splat_or_overprint` per dispatch. The shader
/// uses pure integer math on the packed `ink_mask` so all values are
/// pre-quantised to 0..=255 on the Rust side.
#[repr(C)]
#[derive(Debug, Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
pub struct SplatParams {
    pub ink_mask_packed: u32,
    pub spot_id: u32,
    pub spot_channel: u32,
    pub spot_tint: u32,
    pub width: u32,
    pub height: u32,
    pub _pad0: u32,
    pub _pad1: u32,
}

/// Parameters for `recomposite`. `num_spot_groups` is the per-pixel
/// stride of `spot_planes` (ceil(`num_spots` / 4)). `num_spots` bounds
/// the spot-loop iteration count in the shader.
#[repr(C)]
#[derive(Debug, Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
pub struct RecompositeParams {
    pub width: u32,
    pub height: u32,
    pub num_spot_groups: u32,
    pub num_spots: u32,
}

/// Pack the C, M, Y, K bytes into the same u32 the shader expects
/// (byte 0=C, byte 1=M, byte 2=Y, byte 3=K). Used for the splat
/// dispatch's `ink_mask_packed` and for seeding spot alternate tables.
pub fn pack_cmyk_bytes(c: u8, m: u8, y: u8, k: u8) -> u32 {
    (c as u32) | ((m as u32) << 8) | ((y as u32) << 16) | ((k as u32) << 24)
}

/// Convert a unit-CMYK `[c, m, y, k]` (0..=1 floats) into the same
/// packed-byte u32 the shader expects.
pub fn pack_cmyk_unit(cmyk: [f32; 4]) -> u32 {
    let to_u8 = |v: f32| -> u8 { (v.clamp(0.0, 1.0) * 255.0).round() as u8 };
    pack_cmyk_bytes(to_u8(cmyk[0]), to_u8(cmyk[1]), to_u8(cmyk[2]), to_u8(cmyk[3]))
}

/// Plumbing handle for the two compute pipelines + their bind-group
/// layouts. Cached on `GpuState`; per-render resources allocate
/// against these layouts but the pipelines themselves are reused.
///
/// `Pipelines::new` returns `Err` if either shader fails to compile or
/// the pipeline-creation call fails on the device. The caller (the
/// Vello backend) catches the error and falls back to the knockout
/// path — `Pipelines` is never stored as `Some` if construction
/// errored, so a missing field is treated as "no compute path".
pub struct Pipelines {
    pub splat: wgpu::ComputePipeline,
    pub splat_group0_layout: wgpu::BindGroupLayout,
    pub splat_group1_layout: wgpu::BindGroupLayout,
    pub recomposite: wgpu::ComputePipeline,
    pub recomposite_group0_layout: wgpu::BindGroupLayout,
}

impl Pipelines {
    /// Build both compute pipelines + their bind-group layouts on the
    /// supplied device. The shader sources are validated at build time
    /// (see `crates/paged-gpu/build.rs`); a failure here would have to
    /// be device-specific (limits, missing feature, driver bug).
    ///
    /// Catches panics from `create_shader_module` etc. via
    /// `wgpu::Device::on_uncaptured_error` is out of scope; we let any
    /// pipeline-creation error bubble through `Result<Self, String>`
    /// so the caller can fall back cleanly. A successful return means
    /// both pipelines + every layout are live.
    pub fn new(device: &wgpu::Device) -> Result<Self, String> {
        let splat_shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("paged-gpu splat_or_overprint"),
            source: wgpu::ShaderSource::Wgsl(SPLAT_OR_OVERPRINT_WGSL.into()),
        });
        let recomposite_shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("paged-gpu recomposite"),
            source: wgpu::ShaderSource::Wgsl(RECOMPOSITE_WGSL.into()),
        });

        let splat_group0_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("paged-gpu splat bg0"),
            entries: &[
                // plane_cmyk (read_write)
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::COMPUTE,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Storage { read_only: false },
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
                // coverage (read_write)
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::COMPUTE,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Storage { read_only: false },
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
                // scratch_rgba (read)
                wgpu::BindGroupLayoutEntry {
                    binding: 2,
                    visibility: wgpu::ShaderStages::COMPUTE,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Storage { read_only: true },
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
                // params uniform
                wgpu::BindGroupLayoutEntry {
                    binding: 3,
                    visibility: wgpu::ShaderStages::COMPUTE,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: NonZeroU64::new(
                            std::mem::size_of::<SplatParams>() as u64,
                        ),
                    },
                    count: None,
                },
                // vello_target (read). Needed for the
                // `rgb_to_naive_cmyk_8bit` fallback on virgin pixels —
                // the shader recovers the bottom-side CMYK from any
                // prior non-overprint colour so we don't drop it.
                wgpu::BindGroupLayoutEntry {
                    binding: 4,
                    visibility: wgpu::ShaderStages::COMPUTE,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Storage { read_only: true },
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
            ],
        });

        let splat_group1_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("paged-gpu splat bg1 (spot)"),
            entries: &[
                // spot_plane (read_write). Sentinel-bound on process
                // dispatches; the shader branch on `spot_id` keeps the
                // sentinel untouched.
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::COMPUTE,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Storage { read_only: false },
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
            ],
        });

        let splat_pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("paged-gpu splat pipeline layout"),
            bind_group_layouts: &[Some(&splat_group0_layout), Some(&splat_group1_layout)],
            immediate_size: 0,
        });
        let splat = device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
            label: Some("paged-gpu splat_or_overprint pipeline"),
            layout: Some(&splat_pipeline_layout),
            module: &splat_shader,
            entry_point: Some("main"),
            compilation_options: wgpu::PipelineCompilationOptions::default(),
            cache: None,
        });

        let recomposite_group0_layout =
            device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                label: Some("paged-gpu recomposite bg0"),
                entries: &[
                    wgpu::BindGroupLayoutEntry {
                        binding: 0,
                        visibility: wgpu::ShaderStages::COMPUTE,
                        ty: wgpu::BindingType::Buffer {
                            ty: wgpu::BufferBindingType::Storage { read_only: true },
                            has_dynamic_offset: false,
                            min_binding_size: None,
                        },
                        count: None,
                    },
                    wgpu::BindGroupLayoutEntry {
                        binding: 1,
                        visibility: wgpu::ShaderStages::COMPUTE,
                        ty: wgpu::BindingType::Buffer {
                            ty: wgpu::BufferBindingType::Storage { read_only: true },
                            has_dynamic_offset: false,
                            min_binding_size: None,
                        },
                        count: None,
                    },
                    wgpu::BindGroupLayoutEntry {
                        binding: 2,
                        visibility: wgpu::ShaderStages::COMPUTE,
                        ty: wgpu::BindingType::Buffer {
                            ty: wgpu::BufferBindingType::Storage { read_only: true },
                            has_dynamic_offset: false,
                            min_binding_size: None,
                        },
                        count: None,
                    },
                    wgpu::BindGroupLayoutEntry {
                        binding: 3,
                        visibility: wgpu::ShaderStages::COMPUTE,
                        ty: wgpu::BindingType::Buffer {
                            ty: wgpu::BufferBindingType::Storage { read_only: true },
                            has_dynamic_offset: false,
                            min_binding_size: None,
                        },
                        count: None,
                    },
                    wgpu::BindGroupLayoutEntry {
                        binding: 4,
                        visibility: wgpu::ShaderStages::COMPUTE,
                        ty: wgpu::BindingType::Buffer {
                            ty: wgpu::BufferBindingType::Storage { read_only: true },
                            has_dynamic_offset: false,
                            min_binding_size: None,
                        },
                        count: None,
                    },
                    wgpu::BindGroupLayoutEntry {
                        binding: 5,
                        visibility: wgpu::ShaderStages::COMPUTE,
                        ty: wgpu::BindingType::Buffer {
                            ty: wgpu::BufferBindingType::Uniform,
                            has_dynamic_offset: false,
                            min_binding_size: NonZeroU64::new(
                                std::mem::size_of::<RecompositeParams>() as u64,
                            ),
                        },
                        count: None,
                    },
                    wgpu::BindGroupLayoutEntry {
                        binding: 6,
                        visibility: wgpu::ShaderStages::COMPUTE,
                        ty: wgpu::BindingType::StorageTexture {
                            access: wgpu::StorageTextureAccess::WriteOnly,
                            format: wgpu::TextureFormat::Rgba8Unorm,
                            view_dimension: wgpu::TextureViewDimension::D2,
                        },
                        count: None,
                    },
                ],
            });
        let recomposite_pipeline_layout =
            device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                label: Some("paged-gpu recomposite pipeline layout"),
                bind_group_layouts: &[Some(&recomposite_group0_layout)],
                immediate_size: 0,
            });
        let recomposite = device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
            label: Some("paged-gpu recomposite pipeline"),
            layout: Some(&recomposite_pipeline_layout),
            module: &recomposite_shader,
            entry_point: Some("main"),
            compilation_options: wgpu::PipelineCompilationOptions::default(),
            cache: None,
        });

        Ok(Self {
            splat,
            splat_group0_layout,
            splat_group1_layout,
            recomposite,
            recomposite_group0_layout,
        })
    }
}

/// Test-only knob to force `Pipelines::new` to fail, exercising the
/// knockout-fallback path even on hosts where pipeline creation would
/// otherwise succeed. The `vello_compute_pipeline_creation_failure_falls_back_to_knockout`
/// test toggles this around its render. Outside tests, the value stays
/// `false` and the flag is invisible to callers.
#[cfg(test)]
pub(crate) static FAIL_PIPELINE_CREATION: std::sync::atomic::AtomicBool =
    std::sync::atomic::AtomicBool::new(false);

/// Test-only knob to force the GPU compute path even when the
/// overprint count is below the CPU-finisher threshold. Without this,
/// the focused CMYK overprint tests with small synthetic scenes would
/// route through the CPU rasterizer and the compute shaders would go
/// unexercised. Outside tests, the flag is invisible.
#[cfg(test)]
pub(crate) static FORCE_COMPUTE_PATH: std::sync::atomic::AtomicBool =
    std::sync::atomic::AtomicBool::new(false);

/// Build pipelines, honouring the test-only failure hook. Production
/// callers (the Vello backend) go through this entry-point exclusively
/// so the failure path stays exercised even when `Pipelines::new`
/// itself is robust on real adapters.
pub fn create_pipelines(device: &wgpu::Device) -> Result<Pipelines, String> {
    #[cfg(test)]
    if FAIL_PIPELINE_CREATION.load(std::sync::atomic::Ordering::SeqCst) {
        return Err("test-injected pipeline-creation failure".to_string());
    }
    Pipelines::new(device)
}

/// Whether the CPU fast-path should defer to the GPU compute path
/// regardless of overprint count. Always false outside tests.
pub fn should_force_compute_path() -> bool {
    #[cfg(test)]
    {
        FORCE_COMPUTE_PATH.load(std::sync::atomic::Ordering::SeqCst)
    }
    #[cfg(not(test))]
    {
        false
    }
}
