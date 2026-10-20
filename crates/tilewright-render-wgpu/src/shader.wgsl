// Copyright 2026 Seraphina Studios
// SPDX-License-Identifier: MIT

struct VertexOutput {
    @builtin(position) position: vec4<f32>,
    @location(0) color: vec4<f32>,
    // The frame-relative texture coordinate: 0-1 spans the frame.
    @location(1) uv: vec2<f32>,
    @location(2) effect: f32,
    @location(3) grayscale: f32,
    @location(4) glow_color: vec4<f32>,
    @location(5) @interpolate(flat) glow_radius: vec2<f32>,
    @location(6) @interpolate(flat) flags: u32,
    // The frame's origin and size in whole-texture coordinates.
    @location(7) @interpolate(flat) frame: vec4<f32>,
    // Where a sample point may lie: half a texel inside the frame's edges.
    @location(8) @interpolate(flat) bounds: vec4<f32>,
};

// Each binding pairs a texture with a nearest or a linear sampler, so the
// shader never branches between samplers.
@group(0) @binding(0) var sprite_texture: texture_2d<f32>;
@group(0) @binding(1) var sprite_sampler: sampler;

// Vertex flags.
const FLAG_GLOW: u32 = 1u;

// Whether coordinates are mapped onto a frame and clamped inside it. The
// renderer builds one pipeline with it for frames smaller than their texture
// and one without for whole textures, where frame-relative coordinates are
// texture coordinates, the sampler's clamp-to-edge keeps samples on the
// texture, and plain interpolated coordinates sample fastest.
override CLAMP_TO_FRAME: bool = true;

// Must match tilewright_render_resources::GLOW_DIRECTIONS.
const GLOW_DIRECTIONS = array<vec2<f32>, 16>(
    vec2<f32>(1.0, 0.0),
    vec2<f32>(0.9238795, 0.38268343),
    vec2<f32>(0.70710677, 0.70710677),
    vec2<f32>(0.38268343, 0.9238795),
    vec2<f32>(0.0, 1.0),
    vec2<f32>(-0.38268343, 0.9238795),
    vec2<f32>(-0.70710677, 0.70710677),
    vec2<f32>(-0.9238795, 0.38268343),
    vec2<f32>(-1.0, 0.0),
    vec2<f32>(-0.9238795, -0.38268343),
    vec2<f32>(-0.70710677, -0.70710677),
    vec2<f32>(-0.38268343, -0.9238795),
    vec2<f32>(0.0, -1.0),
    vec2<f32>(0.38268343, -0.9238795),
    vec2<f32>(0.70710677, -0.70710677),
    vec2<f32>(0.9238795, -0.38268343),
);

// Must match tilewright_render_resources::GRAYSCALE_WEIGHTS.
const GRAYSCALE_WEIGHTS = vec3<f32>(0.299, 0.587, 0.114);

@vertex
fn vertex_main(
    @location(0) position: vec3<f32>,
    @location(1) color: vec4<f32>,
    @location(2) uv: vec2<f32>,
    @location(3) effect: f32,
    @location(4) grayscale: f32,
    @location(5) glow_color: vec4<f32>,
    @location(6) glow_radius: vec2<f32>,
    @location(7) flags: u32,
    @location(8) frame: vec4<u32>,
    @location(9) texture_size: vec2<u32>,
) -> VertexOutput {
    let size = vec2<f32>(texture_size);
    let origin = vec2<f32>(frame.xy) / size;
    let extent = vec2<f32>(frame.zw) / size;
    let half_texel = 0.5 / size;
    var output: VertexOutput;
    output.position = vec4<f32>(position, 1.0);
    output.color = color;
    output.uv = uv;
    output.effect = effect;
    output.grayscale = grayscale;
    output.glow_color = glow_color;
    output.glow_radius = glow_radius;
    output.flags = flags;
    output.frame = vec4<f32>(origin, extent);
    output.bounds = vec4<f32>(origin + half_texel, origin + extent - half_texel);
    return output;
}

// Samples the frame at a frame-relative coordinate. Coordinates outside 0-1
// are transparent; the test runs on the frame-relative coordinate, as the
// reference's does, before any rounding from mapping it onto the texture.
// Clamping the mapped point half a texel inside the frame's edges keeps both
// filters on the frame's own texels exactly as the reference clamps to the
// frame's edge: nearest sampling still picks the edge texel there, and
// linear filtering takes the edge texel alone.
fn sample_inside(uv: vec2<f32>, frame: vec4<f32>, bounds: vec4<f32>) -> vec4<f32> {
    if (any(uv < vec2<f32>(0.0)) || any(uv > vec2<f32>(1.0))) {
        return vec4<f32>(0.0);
    }
    if (CLAMP_TO_FRAME) {
        let point = clamp(fma(uv, frame.zw, frame.xy), bounds.xy, bounds.zw);
        return textureSampleLevel(sprite_texture, sprite_sampler, point, 0.0);
    }
    return textureSampleLevel(sprite_texture, sprite_sampler, uv, 0.0);
}

@fragment
fn fragment_main(input: VertexOutput) -> @location(0) vec4<f32> {
    if ((input.flags & FLAG_GLOW) != 0u) {
        var alpha = sample_inside(input.uv, input.frame, input.bounds).a;
        // A zero radius samples only the centre, as in the reference.
        if (any(input.glow_radius > vec2<f32>(0.0))) {
            for (var index = 0u; index < 16u; index = index + 1u) {
                let direction = GLOW_DIRECTIONS[index] * input.glow_radius;
                alpha = max(alpha, sample_inside(input.uv + direction, input.frame, input.bounds).a);
                alpha = max(
                    alpha,
                    sample_inside(input.uv + direction * 0.5, input.frame, input.bounds).a,
                );
            }
        }
        return vec4<f32>(input.glow_color.rgb, input.glow_color.a * alpha);
    }
    var output = sample_inside(input.uv, input.frame, input.bounds) * input.color;
    let gray = dot(output.rgb, GRAYSCALE_WEIGHTS);
    output = vec4<f32>(mix(output.rgb, vec3<f32>(gray), input.grayscale), output.a);
    output = vec4<f32>(mix(output.rgb, output.bgr, input.effect), output.a);
    return output;
}
