#pragma once

// Just enough HLSL for ColourMath.hlsli to compile as C++, so that the test runs the shader's own lines.
#include <algorithm>
#include <cmath>

using uint = unsigned int;

struct float3
{
	float x, y, z;
	float3(float a = 0, float b = 0, float c = 0) : x(a), y(b), z(c) {}
	float3 operator*(float s) const { return {x * s, y * s, z * s}; }
	float3 operator/(float s) const { return {x / s, y / s, z / s}; }
};

struct float4
{
	float x, y, z, w;
	float3 xyz;
	float4(float a, float b, float c, float d) : x(a), y(b), z(c), w(d), xyz(a, b, c) {}
};

inline float pow(float a, float b) { return std::pow(a, b); }
inline float max(float a, float b) { return std::max(a, b); }
inline float clamp(float v, float low, float high) { return std::min(std::max(v, low), high); }
inline float saturate(float v) { return clamp(v, 0.0f, 1.0f); }
inline float3 saturate(float3 v) { return {saturate(v.x), saturate(v.y), saturate(v.z)}; }
inline float dot(float3 a, float3 b) { return a.x * b.x + a.y * b.y + a.z * b.z; }
