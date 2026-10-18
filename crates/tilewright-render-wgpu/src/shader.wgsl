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
    @location(6) mode: f32,
};

@group(0) @binding(0) var sprite_texture: texture_2d<f32>;
@group(0) @binding(1) var sprite_sampler: sampler;

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
    @location(7) mode: f32,
) -> VertexOutput {
    var output: VertexOutput;
    output.position = vec4<f32>(position, 1.0);
    output.color = color;
    output.uv = uv;
    output.effect = effect;
    output.grayscale = grayscale;
    output.glow_color = glow_color;
    output.glow_radius = glow_radius;
    output.mode = mode;
    return output;
}

fn sample_inside(uv: vec2<f32>) -> vec4<f32> {
    if (any(uv < vec2<f32>(0.0)) || any(uv > vec2<f32>(1.0))) {
        return vec4<f32>(0.0);
    }
    return textureSampleLevel(sprite_texture, sprite_sampler, uv, 0.0);
}

@fragment
fn fragment_main(input: VertexOutput) -> @location(0) vec4<f32> {
    if (input.mode > 0.5) {
        var alpha = sample_inside(input.uv).a;
        // A zero radius samples only the centre, as in the reference.
        if (any(input.glow_radius > vec2<f32>(0.0))) {
            for (var index = 0u; index < 16u; index = index + 1u) {
                let direction = GLOW_DIRECTIONS[index] * input.glow_radius;
                alpha = max(alpha, sample_inside(input.uv + direction).a);
                alpha = max(alpha, sample_inside(input.uv + direction * 0.5).a);
            }
        }
        return vec4<f32>(input.glow_color.rgb, input.glow_color.a * alpha);
    }
    var output = sample_inside(input.uv) * input.color;
    let gray = dot(output.rgb, GRAYSCALE_WEIGHTS);
    output = vec4<f32>(mix(output.rgb, vec3<f32>(gray), input.grayscale), output.a);
    output = vec4<f32>(mix(output.rgb, output.bgr, input.effect), output.a);
    return output;
}
