//! Background quad pipeline.
//!
//! Draws axis-aligned coloured rectangles in screen space. Used for
//! per-cell ANSI backgrounds, the cursor block, mouse selection highlight,
//! and the right-click context menu overlay.

use bytemuck::{Pod, Zeroable};
use std::borrow::Cow;

/// One coloured rectangle in pixel space, optionally rotated around its centre.
#[repr(C)]
#[derive(Debug, Clone, Copy, Pod, Zeroable)]
pub struct Quad {
    /// Top-left position (before rotation) in **physical pixels**.
    pub pos: [f32; 2],
    /// Size in **physical pixels**.
    pub size: [f32; 2],
    /// Linear-sRGB premultiplied colour, components in `[0,1]`.
    pub color: [f32; 4],
    /// `cos(theta)` of the rotation around the quad's centre.
    pub rot_cos: f32,
    /// `sin(theta)` of the rotation around the quad's centre.
    pub rot_sin: f32,
}

fn srgb_to_linear(c: u8) -> f32 {
    let v = f32::from(c) / 255.0;
    v.powf(2.2)
}

impl Quad {
    /// Build an axis-aligned quad in physical pixels.
    #[must_use]
    pub fn new(pos: [f32; 2], size: [f32; 2], color_srgb: [u8; 3], alpha: f32) -> Self {
        Self {
            pos,
            size,
            color: [
                srgb_to_linear(color_srgb[0]) * alpha,
                srgb_to_linear(color_srgb[1]) * alpha,
                srgb_to_linear(color_srgb[2]) * alpha,
                alpha,
            ],
            rot_cos: 1.0,
            rot_sin: 0.0,
        }
    }

    /// Build a quad representing a line segment between two points, with
    /// the given thickness (in physical pixels). The segment is rendered
    /// as a thin rotated rectangle aligned along (p1 → p2).
    #[must_use]
    pub fn line(
        p1: [f32; 2],
        p2: [f32; 2],
        thickness: f32,
        color_srgb: [u8; 3],
        alpha: f32,
    ) -> Self {
        let dx = p2[0] - p1[0];
        let dy = p2[1] - p1[1];
        let length = (dx * dx + dy * dy).sqrt().max(0.0001);
        let cos = dx / length;
        let sin = dy / length;
        let cx = (p1[0] + p2[0]) * 0.5;
        let cy = (p1[1] + p2[1]) * 0.5;
        // Store as an axis-aligned `length × thickness` quad whose centre
        // is (cx, cy), rotated by (cos, sin). `pos` stays the pre-rotation
        // top-left; the shader rotates around the quad's geometric centre.
        let pos = [cx - length * 0.5, cy - thickness * 0.5];
        Self {
            pos,
            size: [length, thickness],
            color: [
                srgb_to_linear(color_srgb[0]) * alpha,
                srgb_to_linear(color_srgb[1]) * alpha,
                srgb_to_linear(color_srgb[2]) * alpha,
                alpha,
            ],
            rot_cos: cos,
            rot_sin: sin,
        }
    }
}

#[repr(C)]
#[derive(Debug, Clone, Copy, Pod, Zeroable)]
struct Uniforms {
    viewport: [f32; 2],
    _pad: [f32; 2],
}

/// GPU pipeline that draws a list of [`Quad`]s.
pub struct BgPipeline {
    pipeline: wgpu::RenderPipeline,
    bind_group_layout: wgpu::BindGroupLayout,
    bind_group: wgpu::BindGroup,
    uniforms_buffer: wgpu::Buffer,
    instance_buffer: wgpu::Buffer,
    instance_capacity: usize,
}

impl BgPipeline {
    /// Build the pipeline against the given surface format.
    pub fn new(device: &wgpu::Device, format: wgpu::TextureFormat) -> Self {
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("terminale.bg.shader"),
            source: wgpu::ShaderSource::Wgsl(Cow::Borrowed(BG_SHADER)),
        });

        let bind_group_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("terminale.bg.bg-layout"),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::VERTEX,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: false,
                    min_binding_size: None,
                },
                count: None,
            }],
        });

        let uniforms_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("terminale.bg.uniforms"),
            size: std::mem::size_of::<Uniforms>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("terminale.bg.bind-group"),
            layout: &bind_group_layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: uniforms_buffer.as_entire_binding(),
            }],
        });

        let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("terminale.bg.pipeline-layout"),
            bind_group_layouts: &[&bind_group_layout],
            push_constant_ranges: &[],
        });

        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("terminale.bg.pipeline"),
            layout: Some(&layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: "vs_main",
                compilation_options: wgpu::PipelineCompilationOptions::default(),
                buffers: &[wgpu::VertexBufferLayout {
                    array_stride: std::mem::size_of::<Quad>() as u64,
                    step_mode: wgpu::VertexStepMode::Instance,
                    attributes: &[
                        wgpu::VertexAttribute {
                            offset: 0,
                            shader_location: 0,
                            format: wgpu::VertexFormat::Float32x2,
                        },
                        wgpu::VertexAttribute {
                            offset: 8,
                            shader_location: 1,
                            format: wgpu::VertexFormat::Float32x2,
                        },
                        wgpu::VertexAttribute {
                            offset: 16,
                            shader_location: 2,
                            format: wgpu::VertexFormat::Float32x4,
                        },
                        wgpu::VertexAttribute {
                            offset: 32,
                            shader_location: 3,
                            format: wgpu::VertexFormat::Float32x2,
                        },
                    ],
                }],
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
                topology: wgpu::PrimitiveTopology::TriangleStrip,
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

        let initial_capacity: usize = 256;
        let instance_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("terminale.bg.instances"),
            size: (initial_capacity * std::mem::size_of::<Quad>()) as u64,
            usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        Self {
            pipeline,
            bind_group_layout,
            bind_group,
            uniforms_buffer,
            instance_buffer,
            instance_capacity: initial_capacity,
        }
    }

    /// Upload the latest quad list and viewport size.
    pub fn upload(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        viewport_px: [f32; 2],
        quads: &[Quad],
    ) {
        queue.write_buffer(
            &self.uniforms_buffer,
            0,
            bytemuck::bytes_of(&Uniforms {
                viewport: viewport_px,
                _pad: [0.0, 0.0],
            }),
        );

        if quads.is_empty() {
            return;
        }

        if quads.len() > self.instance_capacity {
            // Grow geometrically so amortised cost stays low.
            let new_capacity = quads.len().next_power_of_two().max(256);
            self.instance_buffer = device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("terminale.bg.instances"),
                size: (new_capacity * std::mem::size_of::<Quad>()) as u64,
                usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            });
            self.instance_capacity = new_capacity;
        }

        queue.write_buffer(&self.instance_buffer, 0, bytemuck::cast_slice(quads));
    }

    /// Issue the draw call for `instance_count` quads.
    pub fn draw<'pass>(&'pass self, pass: &mut wgpu::RenderPass<'pass>, instance_count: u32) {
        self.draw_range(pass, 0..instance_count);
    }

    /// Issue a draw call for a sub-range of the uploaded quad buffer. Used
    /// to split a frame into layers — e.g. terminal background quads drawn
    /// *under* the text pass, and a modal overlay's quads drawn *over* it —
    /// while keeping everything in a single uploaded instance buffer.
    pub fn draw_range<'pass>(
        &'pass self,
        pass: &mut wgpu::RenderPass<'pass>,
        range: std::ops::Range<u32>,
    ) {
        if range.start >= range.end {
            return;
        }
        pass.set_pipeline(&self.pipeline);
        pass.set_bind_group(0, &self.bind_group, &[]);
        pass.set_vertex_buffer(0, self.instance_buffer.slice(..));
        pass.draw(0..4, range);
    }

    /// Bind-group layout, exposed in case higher layers want to extend it.
    #[must_use]
    pub fn bind_group_layout(&self) -> &wgpu::BindGroupLayout {
        &self.bind_group_layout
    }
}

const BG_SHADER: &str = r#"
struct Uniforms {
    viewport: vec2<f32>,
    _pad: vec2<f32>,
};

@group(0) @binding(0) var<uniform> u: Uniforms;

struct VsIn {
    @builtin(vertex_index) vid: u32,
    @location(0) pos: vec2<f32>,
    @location(1) size: vec2<f32>,
    @location(2) color: vec4<f32>,
    @location(3) cossin: vec2<f32>,
};

struct VsOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) color: vec4<f32>,
};

@vertex
fn vs_main(in: VsIn) -> VsOut {
    // 0,1,2,3 -> TL, TR, BL, BR (triangle strip).
    // naga 0.19 forbids dynamic indexing of const arrays in vertex shaders,
    // so derive the corner from bit ops on the vertex index.
    let cx = f32(in.vid & 1u);
    let cy = f32((in.vid >> 1u) & 1u);
    let corner = vec2<f32>(cx, cy);

    // Position the corner relative to the quad's geometric centre, rotate
    // it by (cos, sin), then translate back to pixel-space.
    let centre = in.pos + in.size * 0.5;
    let local = (corner - vec2<f32>(0.5, 0.5)) * in.size;
    let rotated = vec2<f32>(
        local.x * in.cossin.x - local.y * in.cossin.y,
        local.x * in.cossin.y + local.y * in.cossin.x,
    );
    let px = centre + rotated;

    // pixel -> NDC: (px/viewport)*2 - 1, flip y
    let ndc_x = (px.x / u.viewport.x) * 2.0 - 1.0;
    let ndc_y = 1.0 - (px.y / u.viewport.y) * 2.0;

    var out: VsOut;
    out.clip = vec4<f32>(ndc_x, ndc_y, 0.0, 1.0);
    out.color = in.color;
    return out;
}

@fragment
fn fs_main(in: VsOut) -> @location(0) vec4<f32> {
    return in.color;
}
"#;
