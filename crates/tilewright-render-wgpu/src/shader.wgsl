// Copyright 2026 Seraphina Studios
// SPDX-License-Identifier: MIT

struct VertexOutput {
    @builtin(position) position: vec4<f32>,
    @location(0) color: vec4<f32>,
    @location(1) uv: vec2<f32>,
    @location(2) effect: f32,
};

@group(0) @binding(0) var sprite_texture: texture_2d<f32>;
@group(0) @binding(1) var sprite_sampler: sampler;

@vertex
fn vertex_main(
    @location(0) position: vec3<f32>,
    @location(1) color: vec4<f32>,
    @location(2) uv: vec2<f32>,
    @location(3) effect: f32,
) -> VertexOutput {
    var output: VertexOutput;
    output.position = vec4<f32>(position, 1.0);
    output.color = color;
    output.uv = uv;
    output.effect = effect;
    return output;
}

@fragment
fn fragment_main(input: VertexOutput) -> @location(0) vec4<f32> {
    var output = textureSample(sprite_texture, sprite_sampler, input.uv) * input.color;
    output = vec4<f32>(mix(output.rgb, output.bgr, input.effect), output.a);
    return output;
}
