#pragma once

#include "CursorImage.h"
#include "FramePass.h"
#include "HostLink.h"
#include "KernelEscapeChannel.h"
#include "MonitorColour.h"

#include <d3d11.h>
#include <dxgi1_5.h>

#include <atomic>
#include <cstdint>
#include <memory>

namespace Guesthouse
{

// What IddCx said about one frame, without IddCx's types, so that this builds and tests without the WDK.
struct SurfaceFacts
{
	DXGI_COLOR_SPACE_TYPE colourSpace = DXGI_COLOR_SPACE_RGB_FULL_G22_NONE_P709;
	std::uint32_t sdrWhiteNits = 0;
	std::uint64_t presentQpc = 0;
	std::uint32_t windowsFrame = 0;
	HdrMetadataChange hdrChange = HdrMetadataChange::None;
	gh_hdr_metadata hdr = {};
};

// Follows the design's steps for each frame. The two calls are split so that Windows gets its surface back between them:
//
//   Process                     pass into the slot the daemon named, then flush
//   IddCxSwapChainFinishedProcessingFrame
//   Announce                    FRAME to the daemon, whose reply names the next slot
//
// Nothing here may stall or fail the swap chain's loop. With no daemon or no slot, a frame just isn't exported.
class HostExporter
{
public:
	HostExporter(ID3D11Device* device, ID3D11DeviceContext* context, LUID renderAdapter, std::uint32_t monitor,
	             std::shared_ptr<MonitorColour> colour, std::shared_ptr<CursorImage> cursor);
	~HostExporter();
	HostExporter(const HostExporter&) = delete;
	HostExporter& operator=(const HostExporter&) = delete;

	// From CommitModes, on another thread. HDR is the mode's choice, not the frame's: Windows may send any frame as 8-bit.
	void SetCommittedMode(bool known, bool hdr, std::uint32_t refreshMillihertz);

	void Process(IDXGIResource* acquired, const SurfaceFacts& facts);
	void Announce();

private:
	bool EnsureRing(std::uint32_t width, std::uint32_t height, WireFormat format);
	void ReadColourState();
	PassConstants Constants(std::uint32_t width, std::uint32_t height, const SurfaceFacts& facts);

	KernelEscapeChannel m_Channel;
	HostLink m_Link;
	FramePass m_Pass;
	std::shared_ptr<MonitorColour> m_Colour;
	std::shared_ptr<CursorImage> m_Cursor;
	HdrMetadataTracker m_Hdr;
	std::uint32_t m_Monitor;

	bool m_PassReady = false;
	std::atomic<int> m_CommittedMode{0};
	std::atomic<std::uint32_t> m_CommittedRefresh{0};

	std::uint32_t m_RingWidth = 0;
	std::uint32_t m_RingHeight = 0;
	WireFormat m_RingFormat = WireFormat::Sdr;
	std::uint32_t m_RingRefresh = 0;

	std::shared_ptr<const GammaTransform> m_Transform;
	std::uint64_t m_TransformVersion = 0;
	std::uint32_t m_CursorShapeId = 0;

	// What Announce owes the daemon for the frame Process handled.
	bool m_FramePending = false;
	std::uint32_t m_WrittenSlot = GH_NO_SLOT;
	FrameFacts m_Facts;
};

}
