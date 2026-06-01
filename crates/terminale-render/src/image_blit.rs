//! GPU pipeline that blits inline images (OSC 1337 / Sixel / APC graphics)
//! onto the terminal grid.
//!
//! Each visible [`ImagePlacement`] is drawn as a single axis-aligned quad
//! positioned via the grid geometry (cell size + body origin + scroll offset),
//! composited *above* the cell-background quads and *below* text glyphs.
//!
//! Texture management:
//! - Textures are cached by [`ImageId`] in [`ImageBlitPipeline`].
//! - Call [`ImageBlitPipeline::upload_image`] for every newly-added image.
//! - Call [`ImageBlitPipeline::drop_evicted`] with the set of live ids after
//!   each [`ImageStore::live_image_ids`] call to free textures for evicted
//!   images.
//! - Call [`ImageBlitPipeline::draw`] once per frame with the list of visible
//!   placements.

use bytemuck::{Pod, Zeroable};
use std::borrow::Cow;
use std::collections::HashMap;
use terminale_term::{ImageId, InlineImage, VisiblePlacement};

// ── WGSL shader ───────────────────────────────────────────────────────────────

const IMAGE_BLIT_SHADER: &str = concat!(
    "struct Uniforms {\n",
    "    // Normalised device coordinates of the quad: [x0, y0, x1, y1].\n",
    "    ndc: vec4<f32>,\n",
    "}\n",
    "@group(0) @binding(0) var<uniform> u: Uniforms;\n",
    "@group(0) @binding(1) var t_image: texture_2d<f32>;\n",
    "@group(0) @binding(2) var s_image: sampler;\n",
    "\n",
    "struct VOut { @builtin(position) pos: vec4<f32>, @location(0) uv: vec2<f32> }\n",
    "\n",
    "@vertex fn vs_main(@builtin(vertex_index) idx: u32) -> VOut {\n",
    "    // 2 triangles (6 vertices) covering the quad.\n",
    "    // Vertex layout: 0=TL, 1=TR, 2=BL, 3=TR, 4=BR, 5=BL\n",
    "    var xs = array<f32, 6>(u.ndc.x, u.ndc.z, u.ndc.x, u.ndc.z, u.ndc.z, u.ndc.x);\n",
    "    var ys = array<f32, 6>(u.ndc.y, u.ndc.y, u.ndc.w, u.ndc.y, u.ndc.w, u.ndc.w);\n",
    "    var us = array<f32, 6>(0.0, 1.0, 0.0, 1.0, 1.0, 0.0);\n",
    "    var vs = array<f32, 6>(0.0, 0.0, 1.0, 0.0, 1.0, 1.0);\n",
    "    var out: VOut;\n",
    "    out.pos = vec4<f32>(xs[idx], ys[idx], 0.0, 1.0);\n",
    "    out.uv  = vec2<f32>(us[idx], vs[idx]);\n",
    "    return out;\n",
    "}\n",
    "\n",
    "@fragment fn fs_main(in: VOut) -> @location(0) vec4<f32> {\n",
    "    let col = textureSample(t_image, s_image, in.uv);\n",
    "    // Premultiplied-alpha output matches the window's blend mode.\n",
    "    return vec4<f32>(col.rgb * col.a, col.a);\n",
    "}\n",
);

// ── Uniforms ──────────────────────────────────────────────────────────────────

/// Per-draw uniforms: NDC quad extents.
#[repr(C)]
#[derive(Debug, Clone, Copy, Pod, Zeroable)]
struct ImageBlitUniforms {
    ndc: [f32; 4],
}
const _: () = assert!(std::mem::size_of::<ImageBlitUniforms>() == 16);

// ── Pipeline ──────────────────────────────────────────────────────────────────

/// Cached GPU texture + bind-group for one inline image.
struct ImageTexture {
    #[allow(dead_code)]
    texture: wgpu::Texture,
    /// The view keeps the texture alive for the bind-group's sampler reference.
    #[allow(dead_code)]
    view: wgpu::TextureView,
    bind_group: wgpu::BindGroup,
}

/// GPU pipeline for blitting inline images onto the terminal grid.
///
/// One pipeline instance is shared across all images; per-image state lives
/// in the `textures` cache.
pub struct ImageBlitPipeline {
    pipeline: wgpu::RenderPipeline,
    bind_group_layout: wgpu::BindGroupLayout,
    sampler: wgpu::Sampler,
    uniforms_buffer: wgpu::Buffer,
    textures: HashMap<ImageId, ImageTexture>,
}

impl ImageBlitPipeline {
    /// Build the pipeline against the given surface format.
    #[must_use]
    pub fn new(device: &wgpu::Device, format: wgpu::TextureFormat) -> Self {
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("terminale.imgblit.shader"),
            source: wgpu::ShaderSource::Wgsl(Cow::Borrowed(IMAGE_BLIT_SHADER)),
        });

        let bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("terminale.imgblit.bgl"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::VERTEX,
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
            label: Some("terminale.imgblit.uniforms"),
            size: std::mem::size_of::<ImageBlitUniforms>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("terminale.imgblit.sampler"),
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            mipmap_filter: wgpu::FilterMode::Nearest,
            address_mode_u: wgpu::AddressMode::ClampToEdge,
            address_mode_v: wgpu::AddressMode::ClampToEdge,
            ..Default::default()
        });

        let pl = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("terminale.imgblit.pipeline-layout"),
            bind_group_layouts: &[&bgl],
            push_constant_ranges: &[],
        });

        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("terminale.imgblit.pipeline"),
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
                ..Default::default()
            },
            depth_stencil: None,
            multisample: wgpu::MultisampleState::default(),
            multiview: None,
            cache: None,
        });

        Self {
            pipeline,
            bind_group_layout: bgl,
            sampler,
            uniforms_buffer: ub,
            textures: HashMap::new(),
        }
    }

    /// Returns `true` if a GPU texture is already cached for `id`.
    #[must_use]
    pub fn has_texture(&self, id: ImageId) -> bool {
        self.textures.contains_key(&id)
    }

    /// Upload a decoded image to the GPU. Idempotent: if the texture for
    /// `img.id` is already present it is not re-uploaded.
    pub fn upload_image(&mut self, device: &wgpu::Device, queue: &wgpu::Queue, img: &InlineImage) {
        if self.textures.contains_key(&img.id) {
            return; // already uploaded
        }
        if img.width_px == 0 || img.height_px == 0 || img.rgba.is_empty() {
            return;
        }

        let texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("terminale.imgblit.texture"),
            size: wgpu::Extent3d {
                width: img.width_px,
                height: img.height_px,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8UnormSrgb,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });

        queue.write_texture(
            wgpu::ImageCopyTexture {
                texture: &texture,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            &img.rgba,
            wgpu::ImageDataLayout {
                offset: 0,
                bytes_per_row: Some(img.width_px * 4),
                rows_per_image: None,
            },
            wgpu::Extent3d {
                width: img.width_px,
                height: img.height_px,
                depth_or_array_layers: 1,
            },
        );

        let view = texture.create_view(&wgpu::TextureViewDescriptor::default());

        let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("terminale.imgblit.bind-group"),
            layout: &self.bind_group_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: self.uniforms_buffer.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::TextureView(&view),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: wgpu::BindingResource::Sampler(&self.sampler),
                },
            ],
        });

        self.textures.insert(
            img.id,
            ImageTexture {
                texture,
                view,
                bind_group,
            },
        );
        tracing::debug!(
            id = img.id,
            width_px = img.width_px,
            height_px = img.height_px,
            "image-blit: uploaded texture"
        );
    }

    /// Drop GPU textures for any image id that is no longer in `live_ids`.
    /// Call this every frame after [`terminale_term::ImageStore::live_image_ids`].
    pub fn drop_evicted(&mut self, live_ids: &[ImageId]) {
        let live: std::collections::HashSet<ImageId> = live_ids.iter().copied().collect();
        let evicted: Vec<ImageId> = self
            .textures
            .keys()
            .filter(|id| !live.contains(*id))
            .copied()
            .collect();
        for id in evicted {
            self.textures.remove(&id);
            tracing::debug!(id, "image-blit: dropped evicted texture");
        }
    }

    /// Draw all `placements` in the current render pass.
    ///
    /// `body_x`, `body_y` are the physical-pixel coordinates of the terminal
    /// body's top-left corner. `cell_w`, `cell_h` are the physical cell
    /// dimensions. `viewport_w`, `viewport_h` are the surface dimensions in
    /// physical pixels (used for NDC conversion).
    #[allow(clippy::too_many_arguments)]
    pub fn draw<'rp>(
        &'rp self,
        pass: &mut wgpu::RenderPass<'rp>,
        queue: &wgpu::Queue,
        placements: &[VisiblePlacement],
        body_x: f32,
        body_y: f32,
        cell_w: f32,
        cell_h: f32,
        viewport_w: f32,
        viewport_h: f32,
    ) {
        if placements.is_empty() || viewport_w == 0.0 || viewport_h == 0.0 {
            return;
        }

        for vp in placements {
            let p = &vp.placement;
            let Some(tex) = self.textures.get(&p.image_id) else {
                continue;
            };

            // Compute the quad rect in physical pixels.
            let x0 = body_x + f32::from(p.anchor_col) * cell_w;
            let y0 = body_y + f32::from(vp.viewport_row) * cell_h;
            let x1 = x0 + f32::from(p.cols) * cell_w;
            let y1 = y0 + f32::from(p.rows) * cell_h;

            // Convert to NDC (WebGPU: y increases upward, origin at centre).
            let ndc_x0 = x0 / viewport_w * 2.0 - 1.0;
            let ndc_x1 = x1 / viewport_w * 2.0 - 1.0;
            let ndc_y0 = 1.0 - y0 / viewport_h * 2.0; // top in NDC
            let ndc_y1 = 1.0 - y1 / viewport_h * 2.0; // bottom in NDC

            let uniforms = ImageBlitUniforms {
                ndc: [ndc_x0, ndc_y0, ndc_x1, ndc_y1],
            };
            queue.write_buffer(
                &self.uniforms_buffer,
                0,
                bytemuck::cast_slice(&[uniforms]),
            );

            pass.set_pipeline(&self.pipeline);
            pass.set_bind_group(0, &tex.bind_group, &[]);
            pass.draw(0..6, 0..1);
        }
    }
}

// ── Quad geometry unit tests ──────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    /// Compute NDC coordinates for a quad without touching wgpu. Mirrors the
    /// math in [`ImageBlitPipeline::draw`] so we can unit-test it without a GPU.
    // All parameters are distinct viewport/cell dimensions that mirror the GPU
    // draw call; collapsing them would obscure the relationship to the shader.
    #[allow(clippy::too_many_arguments)]
    fn quad_ndc(
        body_x: f32,
        body_y: f32,
        cell_w: f32,
        cell_h: f32,
        viewport_w: f32,
        viewport_h: f32,
        anchor_col: u16,
        viewport_row: u16,
        cols: u16,
        rows: u16,
    ) -> [f32; 4] {
        let x0 = body_x + f32::from(anchor_col) * cell_w;
        let y0 = body_y + f32::from(viewport_row) * cell_h;
        let x1 = x0 + f32::from(cols) * cell_w;
        let y1 = y0 + f32::from(rows) * cell_h;
        let ndc_x0 = x0 / viewport_w * 2.0 - 1.0;
        let ndc_x1 = x1 / viewport_w * 2.0 - 1.0;
        let ndc_y0 = 1.0 - y0 / viewport_h * 2.0;
        let ndc_y1 = 1.0 - y1 / viewport_h * 2.0;
        [ndc_x0, ndc_y0, ndc_x1, ndc_y1]
    }

    #[test]
    fn ndc_full_viewport_is_minus_one_to_one() {
        // A quad covering the entire viewport [0,0]→[vw,vh] should map to NDC [-1,1]→[1,-1].
        let vw = 800.0_f32;
        let vh = 600.0_f32;
        let ndc = quad_ndc(0.0, 0.0, vw, vh, vw, vh, 0, 0, 1, 1);
        assert!((ndc[0] - (-1.0)).abs() < 1e-5, "x0 should be -1");
        assert!((ndc[1] - 1.0).abs() < 1e-5, "y0 (top) should be +1 in NDC");
        assert!((ndc[2] - 1.0).abs() < 1e-5, "x1 should be +1");
        assert!((ndc[3] - (-1.0)).abs() < 1e-5, "y1 (bottom) should be -1 in NDC");
    }

    #[test]
    fn ndc_single_cell_at_origin() {
        // One cell (10×20 px) in a 100×200 viewport at position (0,0).
        let ndc = quad_ndc(0.0, 0.0, 10.0, 20.0, 100.0, 200.0, 0, 0, 1, 1);
        // x0 = 0/100*2-1 = -1, x1 = 10/100*2-1 = -0.8
        assert!((ndc[0] - (-1.0)).abs() < 1e-5);
        assert!((ndc[2] - (-0.8)).abs() < 1e-5);
        // y0 = 1 - 0/200*2 = 1.0, y1 = 1 - 20/200*2 = 0.8
        assert!((ndc[1] - 1.0).abs() < 1e-5);
        assert!((ndc[3] - 0.8).abs() < 1e-5);
    }

    #[test]
    fn ndc_placement_offset_by_body_and_col() {
        // Cell at col=2 with 8px cell width, body_x=10, viewport_w=200
        // x0 = 10 + 2*8 = 26, x1 = 26 + 3*8 = 50
        // NDC: x0 = 26/200*2-1 = -0.74, x1 = 50/200*2-1 = -0.5
        let ndc = quad_ndc(10.0, 0.0, 8.0, 16.0, 200.0, 400.0, 2, 0, 3, 1);
        let expected_x0 = 26.0_f32 / 200.0 * 2.0 - 1.0;
        let expected_x1 = 50.0_f32 / 200.0 * 2.0 - 1.0;
        assert!((ndc[0] - expected_x0).abs() < 1e-5, "x0 mismatch");
        assert!((ndc[2] - expected_x1).abs() < 1e-5, "x1 mismatch");
    }
}
