#pragma once

#include "WireFormat.h"

#include <array>
#include <cstdint>
#include <vector>

namespace Guesthouse
{

constexpr std::uint32_t GammaLutEntries = 4096;

enum class Primaries
{
	Bt709,
	Bt2020,
};

// Three rows of a 3x3 matrix with an offset in the fourth column, as the shader wants them.
struct Rows
{
	float value[3][4];
};

// What Windows sends through EVT_IDD_CX_MONITOR_SET_GAMMA_RAMP as a 3x4 colourspace transform. Console drivers have to apply it to
// every pixel and to cursors. Windows sends it for SDR too, and an unset part has its flag off.
struct GammaTransform
{
	bool matrixEnabled = false;
	float matrix[3][4] = {};
	float scalar = 1.0f;
	bool lutEnabled = false;
	// Red, green, blue and a spare float, for each entry. Empty unless lutEnabled.
	std::vector<std::array<float, 4>> lut;
};

// The whole colour step as one matrix for the shader: source primaries to CIEXYZ, Windows' matrix, which is defined in CIEXYZ,
// then CIEXYZ to the wire's primaries. With no transform that's a plain change of primaries.
Rows ComposeToWire(Primaries source, WireFormat wire, const GammaTransform& transform);

}
