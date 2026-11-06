#pragma once

#include <cstdint>
#include <mutex>
#include <vector>

namespace Guesthouse
{

// The hardware cursor as the frame pass wants it. CursorExporter's thread writes it and the frame thread reads it.
//
// This only moves when a frame arrives, because a hardware cursor moving doesn't make Windows send a frame. That's why
// our vdd_settings.xml turns the hardware cursor off: DWM then draws it, at the right white level, and every move is a frame.
class CursorImage
{
public:
	struct Snapshot
	{
		bool visible = false;
		std::int32_t x = 0;
		std::int32_t y = 0;
		std::uint32_t width = 0;
		std::uint32_t height = 0;
		std::uint32_t pitch = 0;
		std::uint32_t shapeId = 0;
		std::vector<std::uint8_t> pixels; // Only filled in when asked for a shape newer than the caller's.
	};

	void SetPosition(bool visible, std::int32_t x, std::int32_t y)
	{
		std::lock_guard<std::mutex> lock(m_Mutex);
		m_Now.visible = visible;
		m_Now.x = x;
		m_Now.y = y;
	}

	// Only alpha cursors are drawn. A masked one needs the pixels under it, and Windows draws those itself when
	// XorCursorSupportLevel says we can't.
	void SetShape(std::uint32_t shapeId, std::uint32_t width, std::uint32_t height, std::uint32_t pitch, const std::uint8_t* pixels)
	{
		std::lock_guard<std::mutex> lock(m_Mutex);
		m_Now.shapeId = shapeId;
		m_Now.width = width;
		m_Now.height = height;
		m_Now.pitch = pitch;
		m_Now.pixels.assign(pixels, pixels + static_cast<std::size_t>(pitch) * height);
	}

	Snapshot Read(std::uint32_t knownShapeId) const
	{
		std::lock_guard<std::mutex> lock(m_Mutex);
		Snapshot snapshot;
		snapshot.visible = m_Now.visible;
		snapshot.x = m_Now.x;
		snapshot.y = m_Now.y;
		snapshot.width = m_Now.width;
		snapshot.height = m_Now.height;
		snapshot.pitch = m_Now.pitch;
		snapshot.shapeId = m_Now.shapeId;
		if (m_Now.shapeId != knownShapeId)
		{
			snapshot.pixels = m_Now.pixels;
		}
		return snapshot;
	}

private:
	mutable std::mutex m_Mutex;
	Snapshot m_Now;
};

}
