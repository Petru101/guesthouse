#pragma once

#include <guesthouse_display_protocol.h>

#include <cstdint>

namespace Guesthouse
{

// What leaves the guest. Both are four bytes a pixel, so the pass stores one word for each.
enum class WireFormat
{
	Sdr,
	Hdr,
};

// Vulkan and the scanout hardware both want rows on a generous boundary. 3840 and 1920 wide are already multiples.
constexpr std::uint32_t PitchAlignment = 256;
constexpr std::uint32_t BytesPerPixel = 4;

constexpr std::uint32_t PitchFor(std::uint32_t width)
{
	return (width * BytesPerPixel + PitchAlignment - 1) / PitchAlignment * PitchAlignment;
}

constexpr std::uint32_t FourccOf(WireFormat format)
{
	return format == WireFormat::Hdr ? GH_FOURCC_XBGR2101010 : GH_FOURCC_XRGB8888;
}

// Fills in the colour fields of a REGISTER. The pixels are always full range RGB.
inline void DescribeColour(WireFormat format, gh_register& message)
{
	const bool hdr = format == WireFormat::Hdr;
	message.cicp_primaries = hdr ? GH_CICP_PRIMARIES_BT2020 : GH_CICP_PRIMARIES_BT709;
	message.cicp_transfer = hdr ? GH_CICP_TRANSFER_PQ : GH_CICP_TRANSFER_SRGB;
	message.cicp_matrix = GH_CICP_MATRIX_IDENTITY;
	message.full_range = 1;
}

}
