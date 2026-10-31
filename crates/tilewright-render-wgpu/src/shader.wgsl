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
    // The frame's origin and size in texels.
    @location(7) @interpolate(flat) frame: vec4<f32>,
    // Original snapped physical bounds, before conversion to NDC.
    @location(8) @interpolate(flat) screen_rect: vec4<f32>,
    @location(9) @interpolate(flat) uv_rect: vec4<f32>,
};

// The sampler binding remains in the shared resource layout. Texel loads
// implement the reference's staged RGBA8 interpolation without depending on
// a backend's filter precision or atlas-coordinate normalization.
@group(0) @binding(0) var sprite_texture: texture_2d<f32>;
const FLAG_GLOW: u32 = 1u;
const FLAG_ALIGNED: u32 = 2u;
override NEAREST: bool = true;
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
    @location(9) screen_rect: vec4<f32>,
    @location(10) uv_rect: vec4<f32>,
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
    output.frame = vec4<f32>(frame);
    output.screen_rect = screen_rect;
    output.uv_rect = uv_rect;
    return output;
}

// Round nonnegative byte-domain values as Rust f32::round does. WGSL round
// uses ties-to-even, which is not the renderer's RGBA8 contract.
fn bytes(value: vec4<f32>) -> vec4<f32> {
    // Adding 0.5 first can round a value just below a tie upward. Compare
    // the fractional part instead, including at powers of two.
    let rounded = floor(value) + select(vec4<f32>(0.0), vec4<f32>(1.0), fract(value) >= vec4<f32>(0.5));
    return clamp(rounded, vec4<f32>(0.0), vec4<f32>(255.0));
}

fn texel(frame: vec4<f32>, point: vec2<f32>) -> vec4<f32> {
    return bytes(textureLoad(sprite_texture, vec2<i32>(frame.xy + point), 0) * 255.0);
}

// Matches sample_frame_rgba8: clamp to the frame, round horizontal blends,
// then round their vertical blend. No neighboring atlas texel is loaded.
fn sample_inside(uv: vec2<f32>, frame: vec4<f32>) -> vec4<f32> {
    if (any(uv < vec2<f32>(0.0)) || any(uv > vec2<f32>(1.0))) {
        return vec4<f32>(0.0);
    }
    let last = frame.zw - vec2<f32>(1.0);
    if (NEAREST) {
        let point = min(floor(clamp(uv, vec2<f32>(0.0), vec2<f32>(BELOW_ONE)) * frame.zw), last);
        return texel(frame, point);
    }
    // At negative coordinates the reference's fract().max(0) is zero.
    let point = max(fma(uv, frame.zw, vec2<f32>(-0.5)), vec2<f32>(0.0));
    let low = min(floor(point), last);
    let high = min(low + vec2<f32>(1.0), last);
    let amount = fract(point);
    let top = bytes(fma(texel(frame, low), vec4<f32>(1.0 - amount.x),
                        texel(frame, vec2<f32>(high.x, low.y)) * amount.x));
    let bottom = bytes(fma(texel(frame, vec2<f32>(low.x, high.y)), vec4<f32>(1.0 - amount.x),
                           texel(frame, high) * amount.x));
    return bytes(fma(top, vec4<f32>(1.0 - amount.y), bottom * amount.y));
}

fn pixel_uv(input: VertexOutput) -> vec2<f32> {
    if ((input.flags & FLAG_ALIGNED) != 0u) {
        // Hardware coverage can include a snapped edge while interpolated UV
        // is slightly negative after the NDC round trip. Use the same physical
        // pixel center and operation order as for_each_quad_pixel instead.
        let fraction = (input.position.xy - input.screen_rect.xy)
                     / (input.screen_rect.zw - input.screen_rect.xy);
        return input.uv_rect.xy + fraction * (input.uv_rect.zw - input.uv_rect.xy);
    }
    // Rotated/skewed geometry or non-aligned UVs retain triangle interpolation.
    return input.uv;
}

@fragment
fn fragment_main(input: VertexOutput) -> @location(0) vec4<f32> {
    let uv = pixel_uv(input);
    if ((input.flags & FLAG_GLOW) != 0u) {
        var alpha = sample_inside(uv, input.frame).a;
        if (any(input.glow_radius > vec2<f32>(0.0))) {
            for (var index = 0u; index < 16u; index = index + 1u) {
                let direction = GLOW_DIRECTIONS[index] * input.glow_radius;
                alpha = max(alpha, sample_inside(uv + direction, input.frame).a);
                alpha = max(alpha, sample_inside(uv + direction * 0.5, input.frame).a);
            }
        }
        return bytes(vec4<f32>(input.glow_color.rgb * 255.0, input.glow_color.a * alpha)) / 255.0;
    }
    var output = sample_inside(uv, input.frame) * input.color;
    if (input.grayscale > 0.0) {
        let gray = output.r * GRAYSCALE_WEIGHTS.r + output.g * GRAYSCALE_WEIGHTS.g
                 + output.b * GRAYSCALE_WEIGHTS.b;
        output = vec4<f32>(output.rgb + (vec3<f32>(gray) - output.rgb) * input.grayscale, output.a);
    }
    output = bytes(output);
    output = bytes(vec4<f32>(fma(output.rgb, vec3<f32>(1.0 - input.effect), output.bgr * input.effect), output.a));
    return output / 255.0;
}
