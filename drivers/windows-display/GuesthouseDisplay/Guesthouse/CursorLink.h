#pragma once

#include "EscapeChannel.h"

#include <guesthouse_display_protocol.h>

#include <cstdint>

namespace Guesthouse
{

struct CursorPicture
{
	std::uint32_t shape;
	std::uint32_t width;
	std::uint32_t height;
	std::uint32_t pitch;
	std::uint32_t hotX;
	std::uint32_t hotY;
};

// Tells the host about the hardware cursor, for a viewer to draw over the frames. That way the cursor moves without a new frame.
// It runs on the cursor thread with a channel of its own, so a move never waits behind a frame's escape.
class CursorLink
{
public:
	CursorLink(EscapeChannel& channel, std::uint32_t monitor) : m_Channel(channel), m_Monitor(monitor) {}

	void SendShape(const CursorPicture& picture, const std::uint8_t* pixels);
	void SendMove(std::int32_t x, std::int32_t y, bool visible, std::uint32_t sdrWhiteNits);

	// What the last reply said. Without a viewer that draws it, the cursor has to go in the frames.
	bool ViewerDrawsIt() const { return m_ViewerDrawsIt; }

private:
	void FillHeader(gh_header& header, std::uint16_t kind, std::uint32_t size) const;
	void Exchange(void* message, std::uint32_t size);

	EscapeChannel& m_Channel;
	std::uint32_t m_Monitor;
	bool m_ViewerDrawsIt = false;
};

}
