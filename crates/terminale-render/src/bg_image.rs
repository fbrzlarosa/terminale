//! Background image pipeline (fit, fill, opacity, HSB adjustments).
//!
//! Decodes an image file (PNG / JPEG / WebP / GIF first frame) via the
//! [`image`] crate, uploads it as a wgpu RGBA8 texture, and draws a
//! full-screen quad *behind* every other layer. A WGSL fragment shader
//! applies fit/align sampling, per-image opacity, and HSB adjustment.
//!
//! Draw order (same render pass):
//!   clear -> bg_image -> bg_fx -> cell-bg quads -> text -> overlays

use bytemuck::{Pod, Zeroable};
use std::borrow::Cow;
const FIT_FILL: u32 = 0;
const FIT_FIT: u32 = 1;
const FIT_STRETCH: u32 = 2;
const FIT_CENTER: u32 = 3;
const FIT_TILE: u32 = 4;
/// Render-crate fit mode. Kept here so terminale-render stays config-agnostic.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BgImageFit {
    /// Cover -- uniform scale, may crop.
    Fill,
    /// Contain -- uniform scale, may letterbox.
    Fit,
    /// Non-uniform stretch.
    Stretch,
    /// Natural size, centered.
    Center,
    /// Repeat / tile.
    Tile,
}
impl BgImageFit {
    fn shader_mode(self) -> u32 {
        match self {
            Self::Fill => FIT_FILL,
            Self::Fit => FIT_FIT,
            Self::Stretch => FIT_STRETCH,
            Self::Center => FIT_CENTER,
            Self::Tile => FIT_TILE,
        }
    }
}
/// std140 uniforms for the bg-image shader: 12 x 4-byte fields = 48 bytes.
#[repr(C)]
#[derive(Debug, Clone, Copy, Pod, Zeroable)]
struct BgImageUniforms {
    viewport: [f32; 2],
    image_size: [f32; 2],
    opacity: f32,
    fit: u32,
    brightness: f32,
    saturation: f32,
    hue: f32,
    _pad: [u32; 3],
}
const _: () = assert!(std::mem::size_of::<BgImageUniforms>() == 48);// The WGSL shader is stored as a Rust string literal built from concat!() to
// avoid raw-string quoting issues with the embedded WGSL identifiers.
const BG_IMAGE_SHADER: &str = concat!(
    "struct Uniforms {\n",
    "    viewport:   vec2<f32>,\n",
    "    image_size: vec2<f32>,\n",
    "    opacity:    f32,\n",
    "    fit:        u32,\n",
    "    brightness: f32,\n",
    "    saturation: f32,\n",
    "    hue:        f32,\n",
    "    _pad0:      u32,\n",
    "    _pad1:      u32,\n",
    "    _pad2:      u32,\n",
    "}\n",
    "@group(0) @binding(0) var<uniform> u: Uniforms;\n",
    "@group(0) @binding(1) var t_image: texture_2d<f32>;\n",
    "@group(0) @binding(2) var s_image: sampler;\n",
    "@vertex fn vs_main(@builtin(vertex_index) idx: u32) -> @builtin(position) vec4<f32> {\n",
    "    var pos = array<vec2<f32>, 3>(",
    "vec2<f32>(-1.0, -3.0), vec2<f32>(3.0, 1.0), vec2<f32>(-1.0, 1.0));\n",
    "    return vec4<f32>(pos[idx], 0.0, 1.0);\n",
    "}\n",
    "fn compute_uv(fc: vec2<f32>) -> vec2<f32> {\n",
    "    let vp = u.viewport; let im = u.image_size;\n",
    "    if u.fit == 0u {\n",
    "        let sc = max(vp.x/im.x, vp.y/im.y);\n",
    "        return (fc - (vp - im*sc)*0.5) / (im*sc);\n",
    "    } else if u.fit == 1u {\n",
    "        let sc = min(vp.x/im.x, vp.y/im.y);\n",
    "        return clamp((fc - (vp - im*sc)*0.5) / (im*sc), vec2<f32>(0.0), vec2<f32>(1.0));\n",
    "    } else if u.fit == 2u {\n",
    "        return fc / vp;\n",
    "    } else if u.fit == 3u {\n",
    "        return clamp((fc - (vp - im)*0.5) / im, vec2<f32>(0.0), vec2<f32>(1.0));\n",
    "    } else { return fc / im; }\n",
    "}\n",
    "fn rgb_to_hsv(c: vec3<f32>) -> vec3<f32> {\n",
    "    let k = vec4<f32>(0.0, -1.0/3.0, 2.0/3.0, -1.0);\n",
    "    let p = mix(vec4<f32>(c.bg, k.wz), vec4<f32>(c.gb, k.xy), step(c.b, c.g));\n",
    "    let q = mix(vec4<f32>(p.xyw, c.r), vec4<f32>(c.r, p.yzx), step(p.x, c.r));\n",
    "    let d = q.x - min(q.w, q.y); let e = 1e-10;\n",
    "    return vec3<f32>(abs(q.z+(q.w-q.y)/(6.0*d+e)), d/(q.x+e), q.x);\n",
    "}\n",
    "fn hsv_to_rgb(c: vec3<f32>) -> vec3<f32> {\n",
    "    let k = vec4<f32>(1.0, 2.0/3.0, 1.0/3.0, 3.0);\n",
    "    let p = abs(fract(c.xxx + k.xyz)*6.0 - k.www);\n",
    "    return c.z * mix(k.xxx, clamp(p-k.xxx, vec3<f32>(0.0), vec3<f32>(1.0)), c.y);\n",
    "}\n",
    "fn apply_hsb(col: vec3<f32>) -> vec3<f32> {\n",
    "    var hsv = rgb_to_hsv(col);\n",
    "    hsv.x = fract(hsv.x + u.hue/360.0);\n",
    "    hsv.y = clamp(hsv.y * u.saturation, 0.0, 1.0);\n",
    "    hsv.z = clamp(hsv.z * u.brightness, 0.0, 1.0);\n",
    "    return hsv_to_rgb(hsv);\n",
    "}\n",
    "@fragment fn fs_main(@builtin(position) fc: vec4<f32>) -> @location(0) vec4<f32> {\n",
    "    var col = textureSample(t_image, s_image, compute_uv(fc.xy));\n",
    "    col = vec4<f32>(apply_hsb(col.rgb), col.a * u.opacity);\n",
    "    return vec4<f32>(col.rgb * col.a, col.a);\n",
    "}\n",
);
/// Render-crate parameters for the background image.
/// Built from `terminale_config::BackgroundImageConfig` by the host binary.
#[derive(Debug, Clone)]
pub struct BgImageParams {
    /// Absolute path to the image file. `None` = disabled.
    pub path: Option<String>,
    /// Opacity in `0.0..=1.0`.
    pub opacity: f32,
    /// Fit mode.
    pub fit: BgImageFit,
    /// Brightness multiplier `0.0..=2.0`.
    pub brightness: f32,
    /// Saturation multiplier `0.0..=2.0`.
    pub saturation: f32,
    /// Hue rotation in degrees.
    pub hue: f32,
}
impl Default for BgImageParams {
    fn default() -> Self {
        Self {
            path: None,
            opacity: 1.0,
            fit: BgImageFit::Fill,
            brightness: 1.0,
            saturation: 1.0,
            hue: 0.0,
        }
    }
}
impl BgImageParams {
    /// Whether a frame should draw the background image.
    #[must_use]
    pub fn active(&self) -> bool {
        self.path.is_some() && self.opacity > 0.0
    }
}/// GPU pipeline that draws the background image behind the terminal grid.
pub struct BgImagePipeline {
    pipeline: wgpu::RenderPipeline,
    bind_group_layout: wgpu::BindGroupLayout,
    bind_group: wgpu::BindGroup,
    uniforms_buffer: wgpu::Buffer,
    sampler: wgpu::Sampler,
    loaded_path: Option<String>,
    loaded_width: u32,
    loaded_height: u32,
}
fn dummy_texture_view(device: &wgpu::Device) -> wgpu::TextureView {
    let desc_label = "terminale.bgimage.dummy";
    let tex = device.create_texture(&wgpu::TextureDescriptor {
        label: Some(desc_label),
        size: wgpu::Extent3d { width: 1, height: 1, depth_or_array_layers: 1 },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Rgba8UnormSrgb,
        usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
        view_formats: &[],
    });
    tex.create_view(&wgpu::TextureViewDescriptor::default())
}
fn make_bind_group(
    device: &wgpu::Device,
    layout: &wgpu::BindGroupLayout,
    uniforms: &wgpu::Buffer,
    view: &wgpu::TextureView,
    sampler: &wgpu::Sampler,
) -> wgpu::BindGroup {
    device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("terminale.bgimage.bind-group"),
        layout,
        entries: &[
            wgpu::BindGroupEntry { binding: 0, resource: uniforms.as_entire_binding() },
            wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::TextureView(view) },
            wgpu::BindGroupEntry { binding: 2, resource: wgpu::BindingResource::Sampler(sampler) },
        ],
    })
}

impl BgImagePipeline {
    /// Build the pipeline against the given surface format.
    #[must_use]
    pub fn new(device: &wgpu::Device, format: wgpu::TextureFormat) -> Self {
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("terminale.bgimage.shader"),
            source: wgpu::ShaderSource::Wgsl(Cow::Borrowed(BG_IMAGE_SHADER)),
        });
        let bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("terminale.bgimage.layout"),
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
            ],
        });
        let ub = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("terminale.bgimage.uniforms"),
            size: std::mem::size_of::<BgImageUniforms>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("terminale.bgimage.sampler"),
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            mipmap_filter: wgpu::FilterMode::Linear,
            address_mode_u: wgpu::AddressMode::Repeat,
            address_mode_v: wgpu::AddressMode::Repeat,
            ..Default::default()
        });
        let dv = dummy_texture_view(device);
        let bg = make_bind_group(device, &bgl, &ub, &dv, &sampler);
        let pl = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("terminale.bgimage.pipeline-layout"),
            bind_group_layouts: &[&bgl],
            push_constant_ranges: &[],
        });
        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("terminale.bgimage.pipeline"),
            layout: Some(&pl),
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
                    blend: Some(wgpu::BlendState::PREMULTIPLIED_ALPHA_BLENDING),
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
            bind_group_layout: bgl,
            bind_group: bg,
            uniforms_buffer: ub,
            sampler,
            loaded_path: None,
            loaded_width: 1,
            loaded_height: 1,
        }
    }

    fn load_image(&mut self, device: &wgpu::Device, queue: &wgpu::Queue, path: &str) -> bool {
        let img = match image::open(path) {
            Ok(i) => i,
            Err(e) => {
                tracing::warn!(path, error = ?e, "background_image: open failed");
                return false;
            }
        };
        let rgba = img.into_rgba8();
        let (width, height) = (rgba.width(), rgba.height());
        if width == 0 || height == 0 {
            tracing::warn!(path, "background_image: zero-dimension image");
            return false;
        }
        let tex = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("terminale.bgimage.texture"),
            size: wgpu::Extent3d { width, height, depth_or_array_layers: 1 },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8UnormSrgb,
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
            &rgba,
            wgpu::ImageDataLayout {
                offset: 0,
                bytes_per_row: Some(width * 4),
                rows_per_image: None,
            },
            wgpu::Extent3d { width, height, depth_or_array_layers: 1 },
        );
        let view = tex.create_view(&wgpu::TextureViewDescriptor::default());
        self.bind_group = make_bind_group(
            device,
            &self.bind_group_layout,
            &self.uniforms_buffer,
            &view,
            &self.sampler,
        );
        self.loaded_width = width;
        self.loaded_height = height;
        tracing::debug!(path, width, height, "background_image: loaded");
        true
    }

    /// Upload GPU-side uniforms; reload the texture when the path changes.
    pub fn upload(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        viewport_px: [f32; 2],
        params: &BgImageParams,
    ) {
        let want = params.path.as_deref();
        if want != self.loaded_path.as_deref() {
            if let Some(p) = want {
                let ok = self.load_image(device, queue, p);
                self.loaded_path = if ok { Some(p.to_owned()) } else { None };
                if !ok {
                    self.loaded_width = 1;
                    self.loaded_height = 1;
                }
            } else {
                self.loaded_path = None;
                self.loaded_width = 1;
                self.loaded_height = 1;
            }
        }
        let uniforms = BgImageUniforms {
            viewport: viewport_px,
            image_size: [self.loaded_width as f32, self.loaded_height as f32],
            opacity: params.opacity.clamp(0.0, 1.0),
            fit: params.fit.shader_mode(),
            brightness: params.brightness.clamp(0.0, 2.0),
            saturation: params.saturation.clamp(0.0, 2.0),
            hue: params.hue.clamp(0.0, 360.0),
            _pad: [0; 3],
        };
        queue.write_buffer(&self.uniforms_buffer, 0, bytemuck::bytes_of(&uniforms));
    }

    /// Draw the full-screen background image quad into `pass`.
    pub fn draw<'rp>(&'rp self, pass: &mut wgpu::RenderPass<'rp>) {
        pass.set_pipeline(&self.pipeline);
        pass.set_bind_group(0, &self.bind_group, &[]);
        pass.draw(0..3, 0..1);
    }
}

/// Compute UV for `frag_px` inside `viewport_px` sampling `image_px` with `fit`.
/// Mirrors the WGSL `compute_uv` function for pure-Rust testing without a GPU.
#[must_use]
pub fn compute_uv_cpu(
    frag_px: [f32; 2],
    viewport_px: [f32; 2],
    image_px: [f32; 2],
    fit: BgImageFit,
) -> [f32; 2] {
    let [fx, fy] = frag_px;
    let [vw, vh] = viewport_px;
    let [iw, ih] = image_px;
    match fit {
        BgImageFit::Fill => {
            let sc = f32::max(vw / iw, vh / ih);
            let (sw, sh) = (iw * sc, ih * sc);
            [(fx - (vw - sw) * 0.5) / sw, (fy - (vh - sh) * 0.5) / sh]
        }
        BgImageFit::Fit => {
            let sc = f32::min(vw / iw, vh / ih);
            let (sw, sh) = (iw * sc, ih * sc);
            [
                ((fx - (vw - sw) * 0.5) / sw).clamp(0.0, 1.0),
                ((fy - (vh - sh) * 0.5) / sh).clamp(0.0, 1.0),
            ]
        }
        BgImageFit::Stretch => [fx / vw, fy / vh],
        BgImageFit::Center => [
            ((fx - (vw - iw) * 0.5) / iw).clamp(0.0, 1.0),
            ((fy - (vh - ih) * 0.5) / ih).clamp(0.0, 1.0),
        ],
        BgImageFit::Tile => [fx / iw, fy / ih],
    }
}

/// Apply HSB adjustments to sRGB `[r, g, b]` in `0.0..=1.0`.
/// Mirrors the WGSL `apply_hsb` function.
#[must_use]
pub fn apply_hsb_cpu(rgb: [f32; 3], brightness: f32, saturation: f32, hue_deg: f32) -> [f32; 3] {
    let [r, g, b] = rgb;
    let max = r.max(g).max(b);
    let min = r.min(g).min(b);
    let delta = max - min;
    let h_raw = if delta < 1e-10 {
        0.0_f32
    } else if (max - r).abs() < 1e-10 {
        (g - b) / delta
    } else if (max - g).abs() < 1e-10 {
        (b - r) / delta + 2.0
    } else {
        (r - g) / delta + 4.0
    };
    let h = ((h_raw / 6.0) % 1.0 + 1.0) % 1.0;
    let s = if max < 1e-10 { 0.0 } else { delta / max };
    let h = (h + hue_deg / 360.0) % 1.0;
    let s = (s * saturation).clamp(0.0, 1.0);
    let v = (max * brightness).clamp(0.0, 1.0);
    let i = (h * 6.0).floor() as i32;
    let f = h * 6.0 - i as f32;
    let p = v * (1.0 - s);
    let q = v * (1.0 - f * s);
    let t = v * (1.0 - (1.0 - f) * s);
    match i % 6 {
        0 => [v, t, p],
        1 => [q, v, p],
        2 => [p, v, t],
        3 => [p, q, v],
        4 => [t, p, v],
        _ => [v, p, q],
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn uniforms_size_is_48() {
        assert_eq!(std::mem::size_of::<BgImageUniforms>(), 48);
    }

    #[test]
    fn stretch_centre_uv_is_half() {
        let uv = compute_uv_cpu([400.0, 300.0], [800.0, 600.0], [256.0, 256.0], BgImageFit::Stretch);
        assert!((uv[0] - 0.5).abs() < 1e-5);
        assert!((uv[1] - 0.5).abs() < 1e-5);
    }

    #[test]
    fn tile_uv_exceeds_one() {
        let uv = compute_uv_cpu([300.0, 300.0], [800.0, 600.0], [100.0, 100.0], BgImageFit::Tile);
        assert!((uv[0] - 3.0).abs() < 1e-4);
        assert!((uv[1] - 3.0).abs() < 1e-4);
    }

    #[test]
    fn fill_corner_uv() {
        // scale = max(1920/100, 1080/100) = 19.2
        // offset_y = (1080 - 1920) / 2 = -420 => v = 420/1920
        let uv = compute_uv_cpu([0.0, 0.0], [1920.0, 1080.0], [100.0, 100.0], BgImageFit::Fill);
        assert!(uv[0].abs() < 1e-5);
        assert!((uv[1] - 420.0 / 1920.0).abs() < 1e-4);
    }

    #[test]
    fn fit_letterbox_clamped() {
        // scale=0.5; scaled=(100,50); offset_y=(600-50)/2=275
        let uv = compute_uv_cpu([0.0, 275.0], [100.0, 600.0], [200.0, 100.0], BgImageFit::Fit);
        assert!(uv[0].abs() < 1e-5);
        assert!(uv[1].abs() < 1e-5);
    }

    #[test]
    fn hsb_identity() {
        let c = [0.3_f32, 0.5, 0.7];
        let out = apply_hsb_cpu(c, 1.0, 1.0, 0.0);
        for (i, (&a, &b)) in c.iter().zip(out.iter()).enumerate() {
            assert!((a - b).abs() < 1e-4, "channel {i}: {a} vs {b}");
        }
    }

    #[test]
    fn hsb_brightness_zero_is_black() {
        for v in apply_hsb_cpu([0.5, 0.3, 0.8], 0.0, 1.0, 0.0) {
            assert!(v.abs() < 1e-6, "expected 0, got {v}");
        }
    }

    #[test]
    fn hsb_saturation_zero_is_grey() {
        let out = apply_hsb_cpu([0.6, 0.2, 0.8], 1.0, 0.0, 0.0);
        assert!((out[0] - out[1]).abs() < 1e-4);
        assert!((out[1] - out[2]).abs() < 1e-4);
    }

    #[test]
    fn params_active_requires_path_and_opacity() {
        let mut p = BgImageParams::default();
        assert!(!p.active(), "no path should be inactive");
        p.path = Some("/img.png".into());
        assert!(p.active(), "path set should be active");
        p.opacity = 0.0;
        assert!(!p.active(), "opacity=0 should be inactive");
    }
}