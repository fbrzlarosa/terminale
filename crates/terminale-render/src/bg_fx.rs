//! Animated background-effect pipeline.
//!
//! Draws a single full-screen triangle with a procedural fragment shader,
//! selected by a `mode` uniform. Rendered *after* the framebuffer clear but
//! *before* the per-cell background quads and the text, so it shows through
//! every cell that uses the default window background (those quads are
//! skipped) — i.e. it behaves like an animated wallpaper behind the terminal.
//!
//! ## Emitter model
//!
//! Each keystroke spawns a `CpuEmitter` on the CPU. Up to `MAX_EMITTERS`
//! entries are packed into a `BgFxEmitters` uniform (binding 3) and uploaded
//! every frame alongside the existing `BgFxUniforms` (binding 0). The shader
//! loops over `em.count` live emitters (bounded loop, GL/llvmpipe safe) and
//! accumulates their contribution — so multiple concurrent bands are fully
//! supported.

use bytemuck::{Pod, Zeroable};
use std::borrow::Cow;

/// Maximum number of concurrent emitter bands the GPU shader supports.
/// Must match `MAX_EMITTERS` in the WGSL shader exactly.
pub const MAX_EMITTERS: usize = 48;

/// CPU-side parameters for one frame of the background effect. Mirrors the
/// `BgFxUniforms` std140 layout consumed by the shader.
#[repr(C)]
#[derive(Debug, Clone, Copy, Pod, Zeroable)]
struct BgFxUniforms {
    /// Framebuffer size in physical pixels.
    resolution: [f32; 2],
    /// Seconds since start, already multiplied by the speed factor.
    time: f32,
    /// Effect strength / opacity in `0..=1`.
    intensity: f32,
    /// Style selector (see `BackgroundFxStyle::shader_mode`).
    mode: u32,
    /// Global pulse `0..=1` — kept for Aurora/Starfield/PixelCRT global flare.
    /// For Matrix the emitter buffer drives per-band rendering instead.
    pulse: f32,
    /// Katakana glyph-atlas grid columns / rows.
    glyph_cols: u32,
    glyph_rows: u32,
    /// Number of glyphs actually present in the atlas.
    glyph_count: u32,
    _pad: [u32; 3],
    /// Primary tint (linear RGB) in `.xyz`.
    color1: [f32; 4],
    /// Secondary tint (linear RGB) in `.xyz`.
    color2: [f32; 4],
}

// std140: three 16-byte scalar blocks + two vec4s = 80 bytes. Keep the Rust
// mirror in lockstep or wgpu rejects the bind group at draw time.
const _: () = assert!(std::mem::size_of::<BgFxUniforms>() == 80);

/// One per-keystroke emitter on the GPU side. std140: 4 × f32 = 16 bytes.
#[repr(C)]
#[derive(Debug, Clone, Copy, Pod, Zeroable)]
pub struct GpuEmitter {
    /// Scaled time at birth (`bg_fx_start.elapsed() * speed`).
    pub birth: f32,
    /// Normalised horizontal position in `0..=1` (emitter column).
    pub col: f32,
    /// Pseudo-random seed `0..=1` for per-band variation.
    pub seed: f32,
    /// Style kind passed through from the current `BgFxParams::mode` at spawn
    /// time — reserved for future multi-mode mixing; currently matches `mode`.
    pub kind: f32,
}

// 16-byte emitter body — std140-clean.
const _: () = assert!(std::mem::size_of::<GpuEmitter>() == 16);

/// Per-frame emitter array uploaded to binding 3.
///
/// Layout (std140):
/// - header: `count` (u32) + 3 × u32 padding = 16 bytes
/// - body:   `MAX_EMITTERS` × `GpuEmitter` (16 bytes each)
///
/// Total: 16 + 48 × 16 = 784 bytes.
#[repr(C)]
#[derive(Debug, Clone, Copy, Pod, Zeroable)]
struct BgFxEmitters {
    /// Number of live emitters (`0..=MAX_EMITTERS`).
    count: u32,
    _pad: [u32; 3],
    /// Emitter array; only the first `count` entries are meaningful.
    items: [GpuEmitter; MAX_EMITTERS],
}

const _: () = assert!(std::mem::size_of::<BgFxEmitters>() == 16 + MAX_EMITTERS * 16);

/// Resolved, render-ready background-FX settings. Built by the host from the
/// user's `BackgroundFxConfig`.
#[derive(Debug, Clone, Copy)]
pub struct BgFxParams {
    /// Master enable switch.
    pub enabled: bool,
    /// Shader mode index; `0` = off.
    pub mode: u32,
    /// Effect strength / opacity, `0..=1`.
    pub intensity: f32,
    /// Animation speed multiplier.
    pub speed: f32,
    /// Linear-RGB primary tint.
    pub color1: [f32; 3],
    /// Linear-RGB secondary tint.
    pub color2: [f32; 3],
    /// Per-emitter lifetime in seconds.
    pub band_lifetime_secs: f32,
    /// Matrix band width in character columns (`1..=8`).
    pub matrix_band_width: u32,
    /// Matrix base fall speed in character rows per second.
    pub matrix_fall_speed: f32,
    /// Maximum number of concurrent emitter bands (`1..=MAX_EMITTERS`).
    pub max_emitters: u32,
}

impl Default for BgFxParams {
    fn default() -> Self {
        Self {
            enabled: false,
            mode: 0,
            intensity: 0.35,
            speed: 1.0,
            color1: [0.30, 0.10, 0.65],
            color2: [0.05, 0.55, 0.65],
            band_lifetime_secs: 2.5,
            matrix_band_width: 3,
            matrix_fall_speed: 14.0,
            max_emitters: MAX_EMITTERS as u32,
        }
    }
}

impl BgFxParams {
    /// Whether a frame should actually draw the effect.
    #[must_use]
    pub fn active(&self) -> bool {
        self.enabled && self.mode != 0 && self.intensity > 0.0
    }
}

/// GPU pipeline that draws the animated background.
pub struct BgFxPipeline {
    pipeline: wgpu::RenderPipeline,
    bind_group_layout: wgpu::BindGroupLayout,
    bind_group: wgpu::BindGroup,
    uniforms_buffer: wgpu::Buffer,
    /// Emitter array buffer (binding 3); re-uploaded every active frame.
    emitters_buffer: wgpu::Buffer,
    sampler: wgpu::Sampler,
    /// Katakana glyph-atlas grid (set by `set_glyph_atlas`; 0 until then).
    glyph_cols: u32,
    glyph_rows: u32,
    glyph_count: u32,
}

/// Build a 1×1 transparent R8 texture so the bind group is valid before the
/// real glyph atlas is uploaded.
fn dummy_atlas(device: &wgpu::Device) -> wgpu::TextureView {
    let tex = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("terminale.bgfx.atlas.dummy"),
        size: wgpu::Extent3d {
            width: 1,
            height: 1,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::R8Unorm,
        usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
        view_formats: &[],
    });
    tex.create_view(&wgpu::TextureViewDescriptor::default())
}

fn atlas_bind_group(
    device: &wgpu::Device,
    layout: &wgpu::BindGroupLayout,
    uniforms: &wgpu::Buffer,
    emitters: &wgpu::Buffer,
    view: &wgpu::TextureView,
    sampler: &wgpu::Sampler,
) -> wgpu::BindGroup {
    device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("terminale.bgfx.bind-group"),
        layout,
        entries: &[
            wgpu::BindGroupEntry {
                binding: 0,
                resource: uniforms.as_entire_binding(),
            },
            wgpu::BindGroupEntry {
                binding: 1,
                resource: wgpu::BindingResource::TextureView(view),
            },
            wgpu::BindGroupEntry {
                binding: 2,
                resource: wgpu::BindingResource::Sampler(sampler),
            },
            wgpu::BindGroupEntry {
                binding: 3,
                resource: emitters.as_entire_binding(),
            },
        ],
    })
}

impl BgFxPipeline {
    /// Build the pipeline against the given surface format.
    #[must_use]
    pub fn new(device: &wgpu::Device, format: wgpu::TextureFormat) -> Self {
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("terminale.bgfx.shader"),
            source: wgpu::ShaderSource::Wgsl(Cow::Borrowed(BG_FX_SHADER)),
        });

        let bind_group_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("terminale.bgfx.bg-layout"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 2,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 3,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
            ],
        });

        let uniforms_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("terminale.bgfx.uniforms"),
            size: std::mem::size_of::<BgFxUniforms>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        let emitters_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("terminale.bgfx.emitters"),
            size: std::mem::size_of::<BgFxEmitters>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("terminale.bgfx.atlas.sampler"),
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        });

        let bind_group = atlas_bind_group(
            device,
            &bind_group_layout,
            &uniforms_buffer,
            &emitters_buffer,
            &dummy_atlas(device),
            &sampler,
        );

        let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("terminale.bgfx.pipeline-layout"),
            bind_group_layouts: &[&bind_group_layout],
            push_constant_ranges: &[],
        });

        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("terminale.bgfx.pipeline"),
            layout: Some(&layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: "vs_main",
                compilation_options: wgpu::PipelineCompilationOptions::default(),
                buffers: &[],
            },
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: "fs_main",
                compilation_options: wgpu::PipelineCompilationOptions::default(),
                targets: &[Some(wgpu::ColorTargetState {
                    format,
                    blend: Some(wgpu::BlendState::ALPHA_BLENDING),
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            primitive: wgpu::PrimitiveState {
                topology: wgpu::PrimitiveTopology::TriangleList,
                strip_index_format: None,
                front_face: wgpu::FrontFace::Ccw,
                cull_mode: None,
                unclipped_depth: false,
                polygon_mode: wgpu::PolygonMode::Fill,
                conservative: false,
            },
            depth_stencil: None,
            multisample: wgpu::MultisampleState::default(),
            multiview: None,
            cache: None,
        });

        Self {
            pipeline,
            bind_group_layout,
            bind_group,
            uniforms_buffer,
            emitters_buffer,
            sampler,
            glyph_cols: 0,
            glyph_rows: 0,
            glyph_count: 0,
        }
    }

    /// Upload an R8 katakana glyph atlas (`cols × rows` cells, `count` glyphs
    /// filled) and rebuild the bind group to sample it. Call once at startup.
    // All parameters are distinct GPU atlas dimensions; collapsing them into a
    // struct would add indirection for an internal helper method.
    #[allow(clippy::too_many_arguments)]
    pub fn set_glyph_atlas(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        data: &[u8],
        width: u32,
        height: u32,
        cols: u32,
        rows: u32,
        count: u32,
    ) {
        let tex = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("terminale.bgfx.atlas"),
            size: wgpu::Extent3d {
                width,
                height,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::R8Unorm,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        queue.write_texture(
            wgpu::ImageCopyTexture {
                texture: &tex,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            data,
            wgpu::ImageDataLayout {
                offset: 0,
                bytes_per_row: Some(width),
                rows_per_image: Some(height),
            },
            wgpu::Extent3d {
                width,
                height,
                depth_or_array_layers: 1,
            },
        );
        let view = tex.create_view(&wgpu::TextureViewDescriptor::default());
        self.bind_group = atlas_bind_group(
            device,
            &self.bind_group_layout,
            &self.uniforms_buffer,
            &self.emitters_buffer,
            &view,
            &self.sampler,
        );
        self.glyph_cols = cols;
        self.glyph_rows = rows;
        self.glyph_count = count;
    }

    /// Upload this frame's parameters and the live emitter list.
    ///
    /// `gpu_emitters` is the slice of live emitter entries (at most
    /// `MAX_EMITTERS`); `pulse` is the global decaying energy (used by
    /// Aurora / Starfield / PixelCRT for a global flare).
    pub fn upload(
        &self,
        queue: &wgpu::Queue,
        viewport_px: [f32; 2],
        time: f32,
        pulse: f32,
        params: &BgFxParams,
        gpu_emitters: &[GpuEmitter],
    ) {
        // Pack matrix tunables into the unused .w components of color1/color2.
        // The shader reads u.color1.w as the band width (float columns) and
        // u.color2.w as the base fall speed (rows/second).
        #[allow(clippy::cast_precision_loss)]
        let band_w_f = params.matrix_band_width as f32;
        let u = BgFxUniforms {
            resolution: viewport_px,
            time,
            intensity: params.intensity.clamp(0.0, 1.0),
            mode: params.mode,
            pulse: pulse.clamp(0.0, 1.0),
            glyph_cols: self.glyph_cols,
            glyph_rows: self.glyph_rows,
            glyph_count: self.glyph_count,
            _pad: [0; 3],
            color1: [
                params.color1[0],
                params.color1[1],
                params.color1[2],
                band_w_f,
            ],
            color2: [
                params.color2[0],
                params.color2[1],
                params.color2[2],
                params.matrix_fall_speed,
            ],
        };
        queue.write_buffer(&self.uniforms_buffer, 0, bytemuck::bytes_of(&u));

        // Pack live emitters into the fixed-size GPU struct.
        let count = gpu_emitters.len().min(MAX_EMITTERS) as u32;
        let mut em = BgFxEmitters {
            count,
            _pad: [0; 3],
            items: [GpuEmitter {
                birth: 0.0,
                col: 0.0,
                seed: 0.0,
                kind: 0.0,
            }; MAX_EMITTERS],
        };
        for (i, e) in gpu_emitters.iter().take(MAX_EMITTERS).enumerate() {
            em.items[i] = *e;
        }
        queue.write_buffer(&self.emitters_buffer, 0, bytemuck::bytes_of(&em));
    }

    /// Draw the full-screen effect into the current pass.
    pub fn draw<'pass>(&'pass self, pass: &mut wgpu::RenderPass<'pass>) {
        pass.set_pipeline(&self.pipeline);
        pass.set_bind_group(0, &self.bind_group, &[]);
        pass.draw(0..3, 0..1);
    }
}

const BG_FX_SHADER: &str = r#"
// ── Uniform layouts ────────────────────────────────────────────────────────────

struct BgFxUniforms {
    resolution: vec2<f32>,   // physical px
    time:       f32,         // elapsed * speed
    intensity:  f32,         // 0..1
    mode:       u32,         // 1=aurora 2=starfield 3=matrix 4=pixelcrt
    pulse:      f32,         // global decaying keystroke energy 0..1
    glyph_cols: u32,
    glyph_rows: u32,
    glyph_count: u32,
    _pad0:      u32,
    _pad1:      u32,
    _pad2:      u32,
    color1:     vec4<f32>,
    color2:     vec4<f32>,
};

struct GpuEmitter {
    birth: f32,   // scaled time at spawn
    col:   f32,   // normalised column 0..1
    seed:  f32,   // per-band random 0..1
    kind:  f32,   // reserved / mode mirror
};

// MAX_EMITTERS must match the Rust constant MAX_EMITTERS = 48.
const MAX_EMITTERS: u32 = 48u;

struct BgFxEmitters {
    count: u32,
    _pad0: u32,
    _pad1: u32,
    _pad2: u32,
    items: array<GpuEmitter, 48>,
};

@group(0) @binding(0) var<uniform> u:  BgFxUniforms;
@group(0) @binding(1) var atlas_tex:   texture_2d<f32>;
@group(0) @binding(2) var atlas_samp:  sampler;
@group(0) @binding(3) var<uniform> em: BgFxEmitters;

struct VsOut {
    @builtin(position) clip: vec4<f32>,
    @location(0)       uv:   vec2<f32>,
};

// ── Fullscreen triangle (no vertex buffer) ────────────────────────────────────
@vertex
fn vs_main(@builtin(vertex_index) vid: u32) -> VsOut {
    let x = f32((vid << 1u) & 2u);
    let y = f32(vid & 2u);
    var out: VsOut;
    out.clip = vec4<f32>(x * 2.0 - 1.0, 1.0 - y * 2.0, 0.0, 1.0);
    out.uv   = vec2<f32>(x, y);
    return out;
}

// ── Hash utilities ────────────────────────────────────────────────────────────
fn hash21(p: vec2<f32>) -> f32 {
    var p3 = fract(vec3<f32>(p.xyx) * 0.1031);
    p3 = p3 + dot(p3, p3.yzx + 33.33);
    return fract((p3.x + p3.y) * p3.z);
}
fn hash11(n: f32) -> f32 { return fract(sin(n * 12.9898) * 43758.5453); }

// ── Atlas glyph sampler (Matrix shared helper) ────────────────────────────────
fn matrix_glyph(which: u32, sub: vec2<f32>) -> f32 {
    if (u.glyph_count == 0u || u.glyph_cols == 0u) { return 0.0; }
    let idx = which % u.glyph_count;
    let gc  = idx % u.glyph_cols;
    let gr  = idx / u.glyph_cols;
    let sz  = vec2<f32>(1.0 / f32(u.glyph_cols), 1.0 / f32(u.glyph_rows));
    let s   = clamp(sub, vec2<f32>(0.02), vec2<f32>(0.98));
    let uv  = (vec2<f32>(f32(gc), f32(gr)) + s) * sz;
    return textureSampleLevel(atlas_tex, atlas_samp, uv, 0.0).r;
}

// ── Retro posterise: snap brightness to N bands ───────────────────────────────
fn posterize(v: f32, bands: f32) -> f32 { return floor(v * bands) / bands; }

// ── Aurora / plasma ────────────────────────────────────────────────────────────
// Continuous slow-drifting curtain; keystroke emitters inject a vertical light
// column at em.items[i].col that fades with age.
fn fx_aurora(uv: vec2<f32>, t: f32) -> vec4<f32> {
    let p = uv * vec2<f32>(u.resolution.x / u.resolution.y, 1.0);
    var v = sin(p.x * 6.0 + t);
    v += sin(p.y * 7.0 - t * 0.8);
    v += sin(p.x * 4.0 + p.y * 5.0 + t * 0.6);
    v += sin(length(p - vec2<f32>(0.5, 0.5)) * 12.0 - t * 1.2);
    v = v / 4.0;
    let m  = 0.5 + 0.5 * v;
    let curtain = 0.6 + 0.4 * sin(p.x * 14.0 + sin(p.y * 3.0 + t) * 2.0);
    var col = mix(u.color1.xyz, u.color2.xyz, m) * curtain;
    // Posterize for retro banding (~6 bands).
    let lum = dot(col, vec3<f32>(0.299, 0.587, 0.114));
    let plum = posterize(lum, 6.0);
    col = col * (plum / max(lum, 0.001));

    // Keystroke emitter: soft vertical curtain at emitter column.
    var extra = 0.0;
    for (var i = 0u; i < MAX_EMITTERS; i++) {
        if (i >= em.count) { break; }
        let e   = em.items[i];
        let age = t - e.birth;
        if (age < 0.0) { continue; }
        // Horizontal falloff from emitter column (~15% wide Gaussian).
        let dx   = uv.x - e.col;
        let hw   = 0.12 + e.seed * 0.08;
        let xfal = exp(-(dx * dx) / (hw * hw));
        // Vertical shimmer rises from bottom.
        let yosc = 0.5 + 0.5 * sin(uv.y * 8.0 + t * 2.0 + e.seed * 6.28);
        let fade = (1.0 - smoothstep(0.7, 1.0, age / 2.5)) * xfal * yosc;
        extra += fade * 0.55;
    }
    let a = clamp(u.intensity * 0.4 * (1.0 + u.pulse * 0.8) + extra * u.intensity, 0.0, 1.0);
    return vec4<f32>(col * (1.0 + u.pulse * 1.3 + extra * 0.8), a);
}

// ── Starfield ─────────────────────────────────────────────────────────────────
// Pixel-art: stars are hard 2×2 quads on a low-res grid; no soft blobs.
// Keystroke emitters spawn a brief expanding ring at emitter column.
fn px_star(uv: vec2<f32>, t: f32, density: f32, drift: f32) -> f32 {
    let grid = vec2<f32>(density, density * u.resolution.y / u.resolution.x);
    let suv  = uv * grid + vec2<f32>(0.0, t * drift);
    let cell = floor(suv);
    let f    = fract(suv);
    let rnd  = hash21(cell);
    if (rnd < 0.965) { return 0.0; }
    // Pixel-art: 2×2 hard block at cell center (no smoothstep).
    let cx = hash21(cell + 1.7);
    let cy = hash21(cell + 4.3);
    let bx = step(abs(f.x - cx), 1.5 / density);
    let by = step(abs(f.y - cy), 1.5 * u.resolution.x / (u.resolution.y * density));
    let twinkle = 0.55 + 0.45 * step(0.5, sin(t * 4.0 + rnd * 31.4) * 0.5 + 0.5);
    return bx * by * twinkle;
}

fn fx_starfield(uv: vec2<f32>, t: f32) -> vec4<f32> {
    let aspect = vec2<f32>(u.resolution.x / u.resolution.y, 1.0);
    let p = uv * aspect;
    var b = px_star(uv, t, 18.0, 0.02);
    b += 0.7 * px_star(uv + 0.37, t, 30.0, 0.05);
    b += 0.5 * px_star(uv + 0.71, t, 48.0, 0.09);

    // Emitter rings: brief expanding concentric circles.
    for (var i = 0u; i < MAX_EMITTERS; i++) {
        if (i >= em.count) { break; }
        let e   = em.items[i];
        let age = t - e.birth;
        if (age < 0.0) { continue; }
        // Ring expands outward; aspect-correct distance.
        let dx   = (uv.x - e.col) * (u.resolution.x / u.resolution.y);
        let dy   = uv.y - 0.5;
        let dist = sqrt(dx * dx + dy * dy);
        let r    = age * 0.35; // ring radius in UV space
        let ring = exp(-((dist - r) * (dist - r)) * 120.0);
        let fade = (1.0 - smoothstep(0.5, 1.0, age / 1.8));
        b += ring * fade * 1.2;
    }
    b = clamp(b, 0.0, 1.0);
    let col = mix(u.color1.xyz, u.color2.xyz, b);
    let boost = 1.0 + u.pulse * 2.5;
    return vec4<f32>(col * boost, b * u.intensity * boost);
}

// ── Matrix rain ───────────────────────────────────────────────────────────────
// Each emitter drives an independent band of falling katakana. The fall snaps
// to integer rows for a crisp pixel-art cadence. Multiple bands accumulate.
fn fx_matrix_band(
    uv:        vec2<f32>,
    t:         f32,
    band_col:  f32,   // normalised x centre of the band
    band_w:    f32,   // band width in columns
    fall_spd:  f32,   // rows per second
    seed:      f32,
    age:       f32,
    ttl:       f32,
) -> vec4<f32> {
    let aspect = u.resolution.x / u.resolution.y;
    let cols   = 55.0 * aspect;
    let rows   = 30.0;

    // Band x-range in grid columns.
    let bx_ctr  = band_col * cols;
    let bx_lo   = bx_ctr - band_w * 0.5;
    let bx_hi   = bx_lo + band_w;

    let gx = uv.x * cols;
    let gy = uv.y * rows;
    if (gx < bx_lo || gx >= bx_hi) { return vec4<f32>(0.0); }

    let colf = floor(gx);
    // Integer row for crisp pixel-art cadence.
    let rowf = floor(gy);
    let sub  = vec2<f32>(fract(gx), fract(gy));

    // Per-column variation seeded by the band seed so concurrent bands differ.
    let spd_var   = 0.75 + hash11(colf + seed * 37.0) * 0.5;
    let phase_var = hash11(colf + seed * 13.0 + 7.3);
    let eff_spd   = fall_spd * spd_var / rows;   // fractional rows per second

    // Head position: starts at 0, falls downward.
    let head = age * eff_spd * rows;      // in row units
    let dist = head - rowf;               // rows above the head

    var bright = 0.0;
    if (dist >= 0.0 && dist < 20.0) {
        bright = exp(-dist * 0.22);
    }
    if (bright < 0.01) { return vec4<f32>(0.0); }

    // Glyph selection: flickers a few times per second with per-band seed.
    let flick = floor(t * (3.0 + hash11(colf + seed) * 5.0));
    let which = u32(hash21(vec2<f32>(colf + seed * 100.0, rowf) + flick) * 997.0);
    let g = matrix_glyph(which, sub);

    // Head: bright near-white; trail: color1 → color2 gradient.
    let headness = smoothstep(3.0, 0.0, dist);
    let col_rgb  = mix(u.color1.xyz, u.color2.xyz, headness)
                 + vec3<f32>(1.0) * headness * 0.7;

    // Fade out near end of lifetime.
    let fade = 1.0 - smoothstep(ttl * 0.7, ttl, age);

    let a = clamp(bright * g * u.intensity * fade, 0.0, 1.0);
    return vec4<f32>(col_rgb, a);
}

fn fx_matrix(uv: vec2<f32>, t: f32) -> vec4<f32> {
    // These tunables match BgFxParams fields forwarded via the uniforms.
    // band_width is packed into color1.w, fall_speed into color2.w.
    let band_w   = u.color1.w;   // character columns (float)
    let fall_spd = u.color2.w;   // rows/sec

    // Default TTL from the birth/age calc; we use a fixed 2.5 s here
    // because the CPU prunes emitters; the shader fades using the same const.
    let ttl = 2.5;

    var acc = vec4<f32>(0.0);
    for (var i = 0u; i < MAX_EMITTERS; i++) {
        if (i >= em.count) { break; }
        let e   = em.items[i];
        let age = t - e.birth;
        if (age < 0.0 || age > ttl * 1.1) { continue; }
        let band = fx_matrix_band(uv, t, e.col, band_w, fall_spd, e.seed, age, ttl);
        // Additive blend, then clamp.
        acc = vec4<f32>(max(acc.rgb, band.rgb), clamp(acc.a + band.a, 0.0, 1.0));
    }
    return acc;
}

// ── Pixel CRT ─────────────────────────────────────────────────────────────────
// Chunky low-res plasma + hard CRT scanlines. Keystroke emitters spawn a
// bright block-burst at the emitter column.
fn fx_pixelcrt(uv: vec2<f32>, t: f32) -> vec4<f32> {
    let aspect = u.resolution.x / u.resolution.y;
    // Low-res palette-quantized plasma.
    let grid_w  = 80.0 * aspect;
    let grid_h  = 80.0;
    let pixuv   = floor(uv * vec2<f32>(grid_w, grid_h)) / vec2<f32>(grid_w, grid_h);
    // Plasma field.
    var pv = sin(pixuv.x * 8.0 + t * 1.1);
    pv += sin(pixuv.y * 9.0 - t * 0.9);
    pv += sin((pixuv.x + pixuv.y) * 6.0 + t * 0.7);
    pv  = posterize(pv * 0.25 + 0.5, 5.0);   // 5 palette bands
    let col = mix(u.color1.xyz, u.color2.xyz, pv);

    // Hard CRT scanlines (every other low-res row).
    let scan = 0.4 + 0.6 * step(0.5, fract(uv.y * grid_h * 0.5));
    // Occasional row-glitch: shift a row horizontally based on time.
    let glitch_row = floor(t * 3.7 + 0.5);
    let gr_y       = hash11(glitch_row) * grid_h;
    let is_glitch  = step(0.93, hash11(glitch_row + 1.0));
    let shift      = (hash11(glitch_row + 2.0) - 0.5) * 0.04;
    let glyph_uv   = select(uv, uv + vec2<f32>(shift, 0.0),
                           abs(floor(uv.y * grid_h) - gr_y) < 1.0 && is_glitch > 0.5);
    let glitch_lum = hash21(floor(glyph_uv * vec2<f32>(grid_w, grid_h))) * is_glitch * 0.3;

    // Refresh bar sweeping down.
    let yb  = fract(uv.y - t * 0.22);
    let bar = smoothstep(0.06, 0.0, yb) * 0.8;

    var rgb_out = col * scan + glitch_lum + u.color2.xyz * bar;

    // Keystroke emitters: bright block-burst around the emitter column.
    for (var i = 0u; i < MAX_EMITTERS; i++) {
        if (i >= em.count) { break; }
        let e    = em.items[i];
        let age  = t - e.birth;
        if (age < 0.0) { continue; }
        // Block burst — pixel-quantised columns centred on emitter.
        let dx   = abs(uv.x - e.col);
        let bw   = 0.06 + e.seed * 0.04;
        let fade = (1.0 - smoothstep(0.4, 1.0, age / 1.5))
                 * step(dx, bw)
                 * (0.7 + 0.3 * hash21(floor(uv * vec2<f32>(grid_w, grid_h)) + age));
        rgb_out += u.color2.xyz * fade * 0.9;
    }

    let burst  = u.pulse * 1.4;
    rgb_out    = rgb_out * (1.0 + burst);
    let a = u.intensity * clamp(pv * 0.6 + bar * 0.5 + u.pulse * 0.4, 0.0, 1.0);
    return vec4<f32>(clamp(rgb_out, vec3<f32>(0.0), vec3<f32>(1.0)), a);
}

// ── Fragment entry point ──────────────────────────────────────────────────────
@fragment
fn fs_main(in: VsOut) -> @location(0) vec4<f32> {
    let uv = in.uv;
    let t  = u.time;
    var c  = vec4<f32>(0.0);
    if (u.mode == 1u) {
        c = fx_aurora(uv, t);
    } else if (u.mode == 2u) {
        c = fx_starfield(uv, t);
    } else if (u.mode == 3u) {
        // Matrix: emitter-driven, no global pulse boost (it's per-band).
        c = fx_matrix(uv, t);
    } else if (u.mode == 4u) {
        c = fx_pixelcrt(uv, t);
    }
    return c;
}
"#;
