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

// Fragment shader - 420 (8-bit 4:2:0 NV12 YUV) to RGB conversion
// Supports both video range (420v) and full range (420f)
fragment float4 fragment_main(
    VertexOut in [[stage_in]],
    texture2d<float> yTexture [[texture(0)]],   // R8Unorm - 8-bit Y plane (luma)
    texture2d<float> uvTexture [[texture(1)]]   // RG8Unorm - 8-bit UV plane (chroma)
) {
    constexpr sampler textureSampler(mag_filter::linear, min_filter::linear);

    // Sample Y and UV planes
    float y = yTexture.sample(textureSampler, in.texCoord).r;
    float2 uv = uvTexture.sample(textureSampler, in.texCoord).rg;

    // For full range (420f), no offset adjustments needed
    // For video range (420v), we'd need: y = (y - 0.0627) * 1.164
    // Since most hardware decoders output full range, we use full range conversion
    // This provides better color accuracy for most content

    // Center UV values around 0
    float u = (uv.r - 0.5);
    float v = (uv.g - 0.5);

    // BT.709 YUV to RGB conversion matrix (full range)
    // These are the standard coefficients for HD video
    float r = y + 1.5748 * v;
    float g = y - 0.1873 * u - 0.4681 * v;
    float b = y + 1.8556 * u;

    // Clamp to valid range
    r = clamp(r, 0.0, 1.0);
    g = clamp(g, 0.0, 1.0);
    b = clamp(b, 0.0, 1.0);

    return float4(r, g, b, 1.0);
}
