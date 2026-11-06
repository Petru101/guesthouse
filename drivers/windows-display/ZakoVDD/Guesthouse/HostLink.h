#pragma once

#include "EscapeChannel.h"
#include "WireFormat.h"

#include <cstdint>

namespace Guesthouse
{

struct RingShape
{
	std::uint32_t slots = 0;
	std::uint32_t width = 0;
	std::uint32_t height = 0;
	std::uint64_t slotSize = 0;
	WireFormat format = WireFormat::Sdr;
	std::uint32_t monitor = 0;
};

struct FrameFacts
{
	std::uint64_t presentQpc = 0;
	std::uint32_t windowsFrame = 0;
	std::uint32_t sdrWhiteNits = 0;
	bool hasHdrMetadata = false;
	gh_hdr_metadata hdr = {};
};

// The driver's half of the conversation about one ring. It holds no GPU objects and takes no decisions about slots: it
// writes where the last reply said, and has nowhere to write if that was nothing.
class HostLink
{
public:
	explicit HostLink(EscapeChannel& channel) : m_Channel(channel) {}

	// The generation the next ring will have, which its slots must be stamped with before OpenRing.
	std::uint32_t NextGeneration() const { return m_Generation + 1; }

	// True if the daemon took the ring. A daemon that doesn't know these messages answers nothing we can read, so that's false.
	bool OpenRing(const RingShape& shape);
	void CloseRing();

	bool RingOpen() const { return m_RingOpen; }
	// Set when the daemon has refused, or doesn't speak this protocol. Stays until the mode changes.
	bool GaveUp() const { return m_GaveUp; }
	void TryAgain() { m_GaveUp = false; }

	// Where the next frame goes, or GH_NO_SLOT, in which case the pass is skipped for that frame.
	std::uint32_t NextSlot() const { return m_NextSlot; }

	// The pass for this slot has been flushed.
	void FrameWritten(std::uint32_t slot, const FrameFacts& facts);
	// A frame went by with nowhere to write it. Asks for a slot for the one after.
	void FrameSkipped();

	std::uint32_t MonitorMillihertz() const { return m_MonitorMillihertz; }
	std::uint32_t PeakNits() const { return m_PeakNits; }

private:
	void Fill(gh_header& header, std::uint16_t kind, std::uint32_t size) const;
	bool Exchange(void* message, std::uint32_t size);
	void SendFrame(std::uint32_t slot, const FrameFacts& facts);

	EscapeChannel& m_Channel;
	std::uint32_t m_Generation = 0;
	std::uint32_t m_Monitor = 0;
	std::uint64_t m_Sequence = 0;
	std::uint32_t m_NextSlot = GH_NO_SLOT;
	bool m_RingOpen = false;
	bool m_GaveUp = false;
	std::uint32_t m_MonitorMillihertz = 0;
	std::uint32_t m_PeakNits = 0;
};

}
