// The colour maths of the frame pass. It's plain enough to compile as C++ too, so tests/colour_math_test.cpp runs this very
// file on Linux. Keep it to what both languages share: no vector swizzles on the left, no ?: on vectors.
//
// Linear light here is in scRGB's units, where 1.0 is 80 nits, as that's what Windows' FP16 surfaces hold.

#ifndef GUESTHOUSE_COLOUR_MATH
#define GUESTHOUSE_COLOUR_MATH

static const uint SOURCE_SRGB = 0;
static const uint SOURCE_SCRGB = 1;
static const uint SOURCE_PQ = 2;

static const uint WIRE_SDR = 0;
static const uint WIRE_HDR = 1;

static const float SCRGB_NITS = 80.0f;

// SMPTE ST 2084.
static const float PQ_PEAK_NITS = 10000.0f;
static const float PQ_M1 = 0.1593017578125f;
static const float PQ_M2 = 78.84375f;
static const float PQ_C1 = 0.8359375f;
static const float PQ_C2 = 18.8515625f;
static const float PQ_C3 = 18.6875f;

static const float SRGB_LINEAR_LIMIT = 0.04045f;
static const float SRGB_ENCODED_LIMIT = 0.0031308f;

static const float UNORM8_MAX = 255.0f;
static const float UNORM10_MAX = 1023.0f;

float SrgbToLinear1(float encoded)
{
	if (encoded <= SRGB_LINEAR_LIMIT)
	{
		return encoded / 12.92f;
	}
	return pow((encoded + 0.055f) / 1.055f, 2.4f);
}

float LinearToSrgb1(float linearLight)
{
	if (linearLight <= SRGB_ENCODED_LIMIT)
	{
		return linearLight * 12.92f;
	}
	return 1.055f * pow(linearLight, 1.0f / 2.4f) - 0.055f;
}

float PqToNits1(float encoded)
{
	float e = pow(max(encoded, 0.0f), 1.0f / PQ_M2);
	float above = max(e - PQ_C1, 0.0f);
	return PQ_PEAK_NITS * pow(above / (PQ_C2 - PQ_C3 * e), 1.0f / PQ_M1);
}

float NitsToPq1(float nits)
{
	float y = pow(clamp(nits / PQ_PEAK_NITS, 0.0f, 1.0f), PQ_M1);
	return pow((PQ_C1 + PQ_C2 * y) / (1.0f + PQ_C3 * y), PQ_M2);
}

// A surface's pixel as linear light. SDR content sits at the SDR white level, which is how it ends up the same brightness as
// the rest of an HDR desktop. sdrWhite is that level over 80 nits.
float3 DecodeSource(uint source, float3 texel, float sdrWhite)
{
	if (source == SOURCE_SCRGB)
	{
		return texel;
	}
	if (source == SOURCE_PQ)
	{
		return float3(PqToNits1(texel.x), PqToNits1(texel.y), PqToNits1(texel.z)) / SCRGB_NITS;
	}
	return float3(SrgbToLinear1(texel.x), SrgbToLinear1(texel.y), SrgbToLinear1(texel.z)) * sdrWhite;
}

float3 EncodeWire(uint wire, float3 linearLight, float sdrWhite)
{
	if (wire == WIRE_HDR)
	{
		float3 nits = linearLight * SCRGB_NITS;
		return float3(NitsToPq1(nits.x), NitsToPq1(nits.y), NitsToPq1(nits.z));
	}
	float3 relative = saturate(linearLight / sdrWhite);
	return float3(LinearToSrgb1(relative.x), LinearToSrgb1(relative.y), LinearToSrgb1(relative.z));
}

float3 ApplyRows(float4 row0, float4 row1, float4 row2, float3 colour)
{
	return float3(dot(row0.xyz, colour) + row0.w, dot(row1.xyz, colour) + row1.w, dot(row2.xyz, colour) + row2.w);
}

uint Unorm(float value, float top)
{
	return (uint)(saturate(value) * top + 0.5f);
}

// DRM's XRGB8888: B in the low byte.
uint PackXrgb8888(float3 encoded)
{
	return (Unorm(encoded.x, UNORM8_MAX) << 16) | (Unorm(encoded.y, UNORM8_MAX) << 8) | Unorm(encoded.z, UNORM8_MAX);
}

// DRM's XBGR2101010: R in the low bits.
uint PackXbgr2101010(float3 encoded)
{
	return (Unorm(encoded.z, UNORM10_MAX) << 20) | (Unorm(encoded.y, UNORM10_MAX) << 10) | Unorm(encoded.x, UNORM10_MAX);
}

#endif
