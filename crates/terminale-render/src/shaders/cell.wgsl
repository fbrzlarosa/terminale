// Cell renderer — instanced quad per terminal cell.
// Stub shader; the real pipeline will replace this in a follow-up commit.

struct Globals {
    viewport: vec2<f32>,
    cell_size: vec2<f32>,
}

@group(0) @binding(0) var<uniform> globals: Globals;

struct VertexInput {
    @location(0) quad_pos: vec2<f32>,
    @location(1) grid_pos: vec2<u32>,
    @location(2) fg_color: u32,
    @location(3) bg_color: u32,
    @location(4) glyph_id: u32,
    @location(5) flags: u32,
}

struct VertexOutput {
    @builtin(position) clip_position: vec4<f32>,
    @location(0) fg_color: vec4<f32>,
    @location(1) bg_color: vec4<f32>,
}

fn unpack_rgba(c: u32) -> vec4<f32> {
    let r = f32((c >> 24u) & 0xffu) / 255.0;
    let g = f32((c >> 16u) & 0xffu) / 255.0;
    let b = f32((c >> 8u) & 0xffu) / 255.0;
    let a = f32(c & 0xffu) / 255.0;
    return vec4<f32>(r, g, b, a);
}

@vertex
fn vs_main(in: VertexInput) -> VertexOutput {
    var out: VertexOutput;
    let cell_origin = vec2<f32>(f32(in.grid_pos.x), f32(in.grid_pos.y)) * globals.cell_size;
    let pos = (cell_origin + in.quad_pos * globals.cell_size) / globals.viewport * 2.0 - 1.0;
    out.clip_position = vec4<f32>(pos.x, -pos.y, 0.0, 1.0);
    out.fg_color = unpack_rgba(in.fg_color);
    out.bg_color = unpack_rgba(in.bg_color);
    return out;
}

@fragment
fn fs_main(in: VertexOutput) -> @location(0) vec4<f32> {
    // Placeholder: just output the background color until the glyph atlas
    // sampling lands.
    return in.bg_color;
}
