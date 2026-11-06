// The one GPU pass of each frame: Windows' surface in, a pitch-linear slot of the ring out, with the transform Windows asked for
// and the hardware cursor, if there is one, applied on the way.
//
// The slot is a raw buffer because the daemon can only export pitch-linear memory, and a buffer is never tiled.

#include "ColourMath.hlsli"

static const uint GROUP_SIZE = 8;
static const uint LUT_ENTRIES = 4096;

cbuffer PassInputs : register(b0)
{
	uint width;
	uint height;
	uint pitch;
	uint source;

	uint wire;
	uint lutEnabled;
	uint cursorEnabled;
	float sdrWhite; // The SDR white level over 80 nits.

	int cursorX;
	int cursorY;
	uint cursorWidth;
	uint cursorHeight;

	float4 sourceToWire[3];
	float4 cursorToWire[3];
};

Texture2D<float4> surface : register(t0);
Texture2D<float4> cursor : register(t1);
Buffer<float4> lut : register(t2);
RWByteAddressBuffer slot : register(u0);

float LutChannel(float encoded, uint channel)
{
	float position = saturate(encoded) * (LUT_ENTRIES - 1);
	uint below = (uint)position;
	uint above = min(below + 1, LUT_ENTRIES - 1);
	return lerp(lut[below][channel], lut[above][channel], position - below);
}

float3 OverCursor(uint2 pixel, float3 colour)
{
	int2 inside = int2(pixel) - int2(cursorX, cursorY);
	if (inside.x < 0 || inside.y < 0 || inside.x >= (int)cursorWidth || inside.y >= (int)cursorHeight)
	{
		return colour;
	}

	// Cursor pixels are always SDR, so they go through the same door as an SDR surface.
	float4 shape = cursor.Load(int3(inside, 0));
	float3 light = DecodeSource(SOURCE_SRGB, shape.rgb, sdrWhite);
	light = ApplyRows(cursorToWire[0], cursorToWire[1], cursorToWire[2], light);
	return lerp(colour, light, shape.a);
}

[numthreads(GROUP_SIZE, GROUP_SIZE, 1)]
void main(uint3 id : SV_DispatchThreadID)
{
	if (id.x >= width || id.y >= height)
	{
		return;
	}

	float3 texel = surface.Load(int3(id.xy, 0)).rgb;
	float3 light = DecodeSource(source, texel, sdrWhite);
	light = ApplyRows(sourceToWire[0], sourceToWire[1], sourceToWire[2], light);
	if (cursorEnabled != 0)
	{
		light = OverCursor(id.xy, light);
	}

	float3 encoded = EncodeWire(wire, light, sdrWhite);
	if (lutEnabled != 0)
	{
		encoded = float3(LutChannel(encoded.x, 0), LutChannel(encoded.y, 1), LutChannel(encoded.z, 2));
	}

	uint packed = wire == WIRE_HDR ? PackXbgr2101010(encoded) : PackXrgb8888(encoded);
	slot.Store(id.y * pitch + id.x * 4, packed);
}
