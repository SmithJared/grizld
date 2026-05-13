#include <metal_stdlib>
using namespace metal;

// Vertex shader input
struct VertexIn {
    float2 position [[attribute(0)]];
    float2 texCoord [[attribute(1)]];
};

// Vertex shader output / Fragment shader input
struct VertexOut {
    float4 position [[position]];
    float2 texCoord;
};

// Vertex shader - pass through for full-screen quad
vertex VertexOut vertex_main(VertexIn in [[stage_in]]) {
    VertexOut out;
    out.position = float4(in.position, 0.0, 1.0);
    out.texCoord = in.texCoord;
    return out;
}

// Fragment shader - P210 (10-bit 4:2:2 YUV) to RGB conversion
fragment float4 fragment_main(
    VertexOut in [[stage_in]],
    texture2d<float> yTexture [[texture(0)]],   // R16Unorm - 10-bit Y plane
    texture2d<float> uvTexture [[texture(1)]]   // RG16Unorm - 10-bit UV plane
) {
    constexpr sampler textureSampler(mag_filter::linear, min_filter::linear);

    // Sample Y and UV planes (R16/RG16 textures are normalized to [0.0, 1.0])
    float y = yTexture.sample(textureSampler, in.texCoord).r;
    float2 uv = uvTexture.sample(textureSampler, in.texCoord).rg;

    // Convert from 10-bit video range to full range
    // 10-bit video range: Y [64, 940] / 1023, UV [64, 960] / 1023
    // Note: R16Unorm normalization handles the 16-bit to float conversion
    y = (y - 0.0625) * 1.164;
    float u = (uv.r - 0.5);
    float v = (uv.g - 0.5);
    
    // BT.709 YUV to RGB conversion matrix
    float r = y + 1.793 * v;
    float g = y - 0.213 * u - 0.533 * v;
    float b = y + 2.112 * u;
    
    // Clamp to valid range
    r = clamp(r, 0.0, 1.0);
    g = clamp(g, 0.0, 1.0);
    b = clamp(b, 0.0, 1.0);
    
    return float4(r, g, b, 1.0);
}
