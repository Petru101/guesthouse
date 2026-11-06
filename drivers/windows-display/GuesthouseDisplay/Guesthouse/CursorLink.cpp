#include "CursorLink.h"

#include <algorithm>
#include <cstring>
#include <vector>

namespace Guesthouse
{

void CursorLink::FillHeader(gh_header& header, std::uint16_t kind, std::uint32_t size) const
{
	header.magic = GH_REQUEST_MAGIC;
	header.version = GH_PROTOCOL_VERSION;
	header.kind = kind;
	header.size = size;
	header.monitor = m_Monitor;
}

void CursorLink::Exchange(void* message, std::uint32_t size)
{
	m_ViewerDrawsIt = false;
	if (!m_Channel.Exchange(message, size))
	{
		return;
	}
	gh_reply reply;
	std::memcpy(&reply, message, sizeof(reply));
	m_ViewerDrawsIt = reply.header.magic == GH_REPLY_MAGIC && reply.status == GH_STATUS_OK;
}

void CursorLink::SendShape(const CursorPicture& picture, const std::uint8_t* pixels)
{
	const std::uint32_t total = picture.pitch * picture.height;
	std::vector<std::uint8_t> buffer(sizeof(gh_cursor_shape) + GH_CURSOR_PIECE_BYTES);

	for (std::uint32_t offset = 0; offset < total; offset += GH_CURSOR_PIECE_BYTES)
	{
		const std::uint32_t bytes = std::min(GH_CURSOR_PIECE_BYTES, total - offset);
		gh_cursor_shape message = {};
		FillHeader(message.header, GH_KIND_CURSOR_SHAPE, static_cast<std::uint32_t>(sizeof(message)) + bytes);
		message.shape = picture.shape;
		message.width = picture.width;
		message.height = picture.height;
		message.pitch = picture.pitch;
		message.hot_x = picture.hotX;
		message.hot_y = picture.hotY;
		message.total_bytes = total;
		message.piece_offset = offset;
		message.piece_bytes = bytes;
		std::memcpy(buffer.data(), &message, sizeof(message));
		std::memcpy(buffer.data() + sizeof(message), pixels + offset, bytes);
		Exchange(buffer.data(), static_cast<std::uint32_t>(sizeof(message)) + bytes);
	}
}

void CursorLink::SendMove(std::int32_t x, std::int32_t y, bool visible, std::uint32_t sdrWhiteNits)
{
	gh_cursor_move message = {};
	FillHeader(message.header, GH_KIND_CURSOR_MOVE, sizeof(message));
	message.x = x;
	message.y = y;
	message.visible = visible ? 1 : 0;
	message.sdr_white_nits = sdrWhiteNits;
	Exchange(&message, sizeof(message));
}

}
