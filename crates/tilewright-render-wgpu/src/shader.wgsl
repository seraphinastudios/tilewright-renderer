// Copyright 2026 Seraphina Studios
// SPDX-License-Identifier: MIT

struct VertexOutput {
    @builtin(position) position: vec4<f32>,
    @location(0) color: vec4<f32>,
    @location(1) uv: vec2<f32>,
    @location(2) effect: f32,
    @location(3) grayscale: f32,
    @location(4) glow_color: vec4<f32>,
    @location(5) glow_radius: vec2<f32>,
    @location(6) @interpolate(flat) flags: u32,
    @location(7) @interpolate(flat) frame: vec4<u32>,
};

// Texels are read exactly with textureLoad and filtered here, so sampling
// matches tilewright_render_resources::sample_frame_rgba8 and never reads
// outside the quad's frame.
@group(0) @binding(0) var sprite_texture: texture_2d<f32>;

// Vertex flags.
const FLAG_GLOW: u32 = 1u;
const FLAG_LINEAR: u32 = 2u;
// The largest f32 below 1: 1 - f32::EPSILON.
const BELOW_ONE: f32 = 0.99999988;

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
) -> VertexOutput {
    var output: VertexOutput;
    output.position = vec4<f32>(position, 1.0);
    output.color = color;
    output.uv = uv;
    output.effect = effect;
    output.grayscale = grayscale;
    output.glow_color = glow_color;
    output.glow_radius = glow_radius;
    output.flags = flags;
    output.frame = frame;
    return output;
}

// One texel of the frame, in 0-255 units.
fn frame_texel(frame: vec4<u32>, texel: vec2<u32>) -> vec4<f32> {
    return textureLoad(sprite_texture, vec2<i32>(frame.xy + texel), 0) * 255.0;
}

// An 8-bit blend rounded half up, as the reference rounds.
fn blend(left: vec4<f32>, right: vec4<f32>, amount: f32) -> vec4<f32> {
    return floor(fma(left, vec4<f32>(1.0 - amount), right * amount) + 0.5);
}

// The frame sampled at a frame-relative coordinate, clamped to the frame.
fn sample_frame(uv: vec2<f32>, frame: vec4<u32>, linear: bool) -> vec4<f32> {
    let size = vec2<f32>(frame.zw);
    let last = frame.zw - vec2<u32>(1u);
    if (!linear) {
        let texel = vec2<u32>(floor(clamp(uv, vec2<f32>(0.0), vec2<f32>(BELOW_ONE)) * size));
        return frame_texel(frame, min(texel, last)) / 255.0;
    }
    let position = fma(uv, size, vec2<f32>(-0.5));
    let first = min(vec2<u32>(max(floor(position), vec2<f32>(0.0))), last);
    let second = min(first + vec2<u32>(1u), last);
    // The fraction past truncation, clamped at zero, as the reference takes it.
    let amount = max(position - trunc(position), vec2<f32>(0.0));
    let top = blend(
        frame_texel(frame, first),
        frame_texel(frame, vec2<u32>(second.x, first.y)),
        amount.x,
    );
    let bottom = blend(
        frame_texel(frame, vec2<u32>(first.x, second.y)),
        frame_texel(frame, second),
        amount.x,
    );
    return blend(top, bottom, amount.y) / 255.0;
}

fn sample_inside(uv: vec2<f32>, frame: vec4<u32>, linear: bool) -> vec4<f32> {
    if (any(uv < vec2<f32>(0.0)) || any(uv > vec2<f32>(1.0))) {
        return vec4<f32>(0.0);
    }
    return sample_frame(uv, frame, linear);
}

@fragment
fn fragment_main(input: VertexOutput) -> @location(0) vec4<f32> {
    let linear = (input.flags & FLAG_LINEAR) != 0u;
    if ((input.flags & FLAG_GLOW) != 0u) {
        var alpha = sample_inside(input.uv, input.frame, linear).a;
        // A zero radius samples only the centre, as in the reference.
        if (any(input.glow_radius > vec2<f32>(0.0))) {
            for (var index = 0u; index < 16u; index = index + 1u) {
                let direction = GLOW_DIRECTIONS[index] * input.glow_radius;
                alpha = max(alpha, sample_inside(input.uv + direction, input.frame, linear).a);
                alpha = max(alpha, sample_inside(input.uv + direction * 0.5, input.frame, linear).a);
            }
        }
        return vec4<f32>(input.glow_color.rgb, input.glow_color.a * alpha);
    }
    var output = sample_inside(input.uv, input.frame, linear) * input.color;
    let gray = dot(output.rgb, GRAYSCALE_WEIGHTS);
    output = vec4<f32>(mix(output.rgb, vec3<f32>(gray), input.grayscale), output.a);
    output = vec4<f32>(mix(output.rgb, output.bgr, input.effect), output.a);
    return output;
}
