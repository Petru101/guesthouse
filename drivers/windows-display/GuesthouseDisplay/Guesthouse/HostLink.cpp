#include "HostLink.h"

#include <algorithm>
#include <cstring>

namespace Guesthouse
{

namespace
{

// The request is written into this and the reply read out of it. The daemon replies in the same bytes.
union MessageBuffer
{
	gh_register registerRing;
	gh_frame frame;
	gh_close close;
	gh_reply reply;
};

constexpr std::uint32_t BufferSize = sizeof(MessageBuffer);

}

void HostLink::Fill(gh_header& header, std::uint16_t kind, std::uint32_t size) const
{
	header.magic = GH_REQUEST_MAGIC;
	header.version = GH_PROTOCOL_VERSION;
	header.kind = kind;
	header.size = size;
	header.generation = m_Generation;
	header.monitor = m_Monitor;
}

bool HostLink::Exchange(void* message, std::uint32_t size)
{
	if (!m_Channel.Exchange(message, size))
	{
		m_RingOpen = false;
		m_NextSlot = GH_NO_SLOT;
		return false;
	}

	const auto* reply = static_cast<const gh_reply*>(message);
	if (reply->header.magic != GH_REPLY_MAGIC || reply->header.version != GH_PROTOCOL_VERSION)
	{
		m_GaveUp = true;
		m_RingOpen = false;
		m_NextSlot = GH_NO_SLOT;
		return false;
	}

	m_MonitorMillihertz = reply->monitor_millihertz;
	m_PeakNits = reply->peak_nits;

	switch (reply->status)
	{
	case GH_STATUS_OK:
		m_NextSlot = reply->next_slot;
		return true;
	case GH_STATUS_STALE_GENERATION:
		// The daemon restarted or moved on. Whoever owns the ring makes a new one on the next frame.
		m_RingOpen = false;
		m_NextSlot = GH_NO_SLOT;
		return false;
	default:
		m_GaveUp = true;
		m_RingOpen = false;
		m_NextSlot = GH_NO_SLOT;
		return false;
	}
}

bool HostLink::OpenRing(const RingShape& shape)
{
	m_Generation = NextGeneration();
	m_Monitor = shape.monitor;
	m_Sequence = 0;

	MessageBuffer buffer = {};
	gh_register& message = buffer.registerRing;
	Fill(message.header, GH_KIND_REGISTER, sizeof(message));
	message.slot_count = shape.slots;
	message.width = shape.width;
	message.height = shape.height;
	message.pitch = PitchFor(shape.width);
	message.slot_size = shape.slotSize;
	message.fourcc = FourccOf(shape.format);
	DescribeColour(shape.format, message);

	m_RingOpen = Exchange(&buffer, BufferSize);
	return m_RingOpen;
}

void HostLink::CloseRing()
{
	if (!m_RingOpen)
	{
		return;
	}

	MessageBuffer buffer = {};
	Fill(buffer.close.header, GH_KIND_CLOSE, sizeof(buffer.close));
	Exchange(&buffer, BufferSize);
	m_RingOpen = false;
	m_NextSlot = GH_NO_SLOT;
}

void HostLink::SendFrame(std::uint32_t slot, const FrameFacts& facts)
{
	MessageBuffer buffer = {};
	gh_frame& message = buffer.frame;
	Fill(message.header, GH_KIND_FRAME, sizeof(message));
	message.slot = slot;
	message.sequence = m_Sequence;
	message.present_qpc = facts.presentQpc;
	message.windows_frame = facts.windowsFrame;
	message.sdr_white_nits = facts.sdrWhiteNits;
	if (facts.hasHdrMetadata)
	{
		message.flags |= GH_FRAME_HAS_HDR_METADATA;
		message.hdr = facts.hdr;
	}
	Exchange(&buffer, BufferSize);
}

void HostLink::FrameWritten(std::uint32_t slot, const FrameFacts& facts)
{
	++m_Sequence;
	SendFrame(slot, facts);
}

void HostLink::FrameSkipped()
{
	SendFrame(GH_NO_SLOT, FrameFacts{});
}

}
