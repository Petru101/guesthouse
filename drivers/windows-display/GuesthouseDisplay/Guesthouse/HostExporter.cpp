#include "HostExporter.h"

#include "..\Logging\Logger.h"

#include <cstring>

using Microsoft::WRL::ComPtr;

namespace Guesthouse
{

namespace
{

// The design's six slots: 200 MB at 4K.
constexpr std::uint32_t RingSlots = 6;
static_assert(RingSlots <= GH_MAX_SLOTS, "The protocol caps how many slots a ring has");

constexpr float ScrgbNits = 80.0f;
constexpr std::uint32_t DefaultSdrWhiteNits = 80;

// Values for the shader's SOURCE_ constants.
constexpr std::uint32_t SourceSrgb = 0;
constexpr std::uint32_t SourceScrgb = 1;
constexpr std::uint32_t SourcePq = 2;

// m_CommittedMode's values.
constexpr int ModeUnknown = 0;
constexpr int ModeSdr = 1;
constexpr int ModeHdr = 2;

std::uint32_t SourceOf(DXGI_COLOR_SPACE_TYPE colourSpace)
{
	switch (colourSpace)
	{
	case DXGI_COLOR_SPACE_RGB_FULL_G10_NONE_P709:
		return SourceScrgb;
	case DXGI_COLOR_SPACE_RGB_FULL_G2084_NONE_P2020:
		return SourcePq;
	default:
		return SourceSrgb;
	}
}

Primaries PrimariesOf(std::uint32_t source)
{
	return source == SourcePq ? Primaries::Bt2020 : Primaries::Bt709;
}

void CopyRows(float (&to)[3][4], const Rows& from)
{
	std::memcpy(to, from.value, sizeof(to));
}

}

HostExporter::HostExporter(ID3D11Device* device, ID3D11DeviceContext* context, LUID renderAdapter, std::uint32_t monitor,
                           std::shared_ptr<MonitorColour> colour, std::shared_ptr<CursorImage> cursor)
	: m_Channel(renderAdapter), m_Link(m_Channel), m_Pass(device, context), m_Colour(std::move(colour)),
	  m_Cursor(std::move(cursor)), m_Monitor(monitor)
{
	const HRESULT hr = m_Pass.Init();
	m_PassReady = SUCCEEDED(hr);
	if (!m_PassReady)
	{
		VDD_LOG_ERROR_STREAM("[Guesthouse] The frame pass failed to start: 0x" << std::hex << hr);
	}
}

HostExporter::~HostExporter()
{
	m_Link.CloseRing();
}

void HostExporter::SetCommittedMode(bool known, bool hdr, std::uint32_t refreshMillihertz)
{
	m_CommittedMode = known ? (hdr ? ModeHdr : ModeSdr) : ModeUnknown;
	m_CommittedRefresh = refreshMillihertz;
}

bool HostExporter::EnsureRing(std::uint32_t width, std::uint32_t height, WireFormat format)
{
	const std::uint32_t refresh = m_CommittedRefresh;
	const bool sameShape = width == m_RingWidth && height == m_RingHeight && format == m_RingFormat && refresh == m_RingRefresh;
	if (sameShape && (m_Link.RingOpen() || m_Link.GaveUp()))
	{
		return m_Link.RingOpen();
	}

	// A new shape is a new mode, which earns a daemon that refused another chance.
	if (!sameShape)
	{
		m_Link.TryAgain();
	}
	m_Link.CloseRing();

	const HRESULT hr = m_Pass.CreateRing(RingSlots, width, height, m_Link.NextGeneration());
	if (FAILED(hr))
	{
		VDD_LOG_ERROR_STREAM("[Guesthouse] Making the ring failed: 0x" << std::hex << hr);
		return false;
	}
	m_RingWidth = width;
	m_RingHeight = height;
	m_RingFormat = format;
	m_RingRefresh = refresh;

	RingShape shape;
	shape.slots = RingSlots;
	shape.width = width;
	shape.height = height;
	shape.slotSize = m_Pass.SlotSize();
	shape.format = format;
	shape.refreshMillihertz = refresh;
	shape.monitor = m_Monitor;

	const bool open = m_Link.OpenRing(shape);
	VDD_LOG_INFO_STREAM("[Guesthouse] Ring " << width << "x" << height << (format == WireFormat::Hdr ? " HDR" : " SDR")
	                    << (open ? " registered" : " not taken by the host"));
	return open;
}

void HostExporter::ReadColourState()
{
	std::uint64_t version = 0;
	auto transform = m_Colour->Transform(version);
	if (version != m_TransformVersion)
	{
		m_Transform = std::move(transform);
		m_TransformVersion = version;
		m_Pass.SetLut(*m_Transform);
	}

	gh_hdr_metadata fallback = {};
	if (m_Colour->DefaultHdrMetadata(fallback))
	{
		m_Hdr.SetDefault(fallback);
	}
}

PassConstants HostExporter::Constants(std::uint32_t width, std::uint32_t height, const SurfaceFacts& facts)
{
	const std::uint32_t source = SourceOf(facts.colourSpace);
	const std::uint32_t whiteNits = facts.sdrWhiteNits ? facts.sdrWhiteNits : DefaultSdrWhiteNits;

	PassConstants constants = {};
	constants.width = width;
	constants.height = height;
	constants.pitch = PitchFor(width);
	constants.source = source;
	constants.wire = m_RingFormat == WireFormat::Hdr ? 1 : 0;
	constants.lutEnabled = m_Transform->lutEnabled ? 1 : 0;
	constants.sdrWhite = static_cast<float>(whiteNits) / ScrgbNits;
	CopyRows(constants.sourceToWire, ComposeToWire(PrimariesOf(source), m_RingFormat, *m_Transform));
	CopyRows(constants.cursorToWire, ComposeToWire(Primaries::Bt709, m_RingFormat, *m_Transform));

	const CursorImage::Snapshot cursor = m_Cursor->Read(m_CursorShapeId);
	if (!cursor.pixels.empty() && SUCCEEDED(m_Pass.SetCursorShape(cursor)))
	{
		m_CursorShapeId = cursor.shapeId;
	}
	if (cursor.visible && m_CursorShapeId != 0 && !m_Cursor->DrawnByViewer())
	{
		constants.cursorEnabled = 1;
		constants.cursorX = cursor.x;
		constants.cursorY = cursor.y;
		constants.cursorWidth = cursor.width;
		constants.cursorHeight = cursor.height;
	}
	return constants;
}

void HostExporter::Process(IDXGIResource* acquired, const SurfaceFacts& facts)
{
	m_FramePending = false;
	m_Hdr.Apply(facts.hdrChange, facts.hdr);
	if (!m_PassReady || !acquired)
	{
		return;
	}

	ComPtr<ID3D11Texture2D> surface;
	if (FAILED(acquired->QueryInterface(__uuidof(ID3D11Texture2D), reinterpret_cast<void**>(surface.GetAddressOf()))))
	{
		return;
	}
	D3D11_TEXTURE2D_DESC desc = {};
	surface->GetDesc(&desc);

	const int mode = m_CommittedMode;
	const bool hdr = mode == ModeUnknown ? SourceOf(facts.colourSpace) != SourceSrgb : mode == ModeHdr;
	if (!EnsureRing(desc.Width, desc.Height, hdr ? WireFormat::Hdr : WireFormat::Sdr))
	{
		return;
	}

	m_FramePending = true;
	m_WrittenSlot = GH_NO_SLOT;
	const std::uint32_t slot = m_Link.NextSlot();
	if (slot == GH_NO_SLOT)
	{
		return;
	}

	ReadColourState();
	const HRESULT hr = m_Pass.Run(surface.Get(), slot, Constants(desc.Width, desc.Height, facts));
	if (FAILED(hr))
	{
		VDD_LOG_WARNING_STREAM("[Guesthouse] The frame pass failed: 0x" << std::hex << hr);
		return;
	}

	m_WrittenSlot = slot;
	m_Facts.presentQpc = facts.presentQpc;
	m_Facts.windowsFrame = facts.windowsFrame;
	m_Facts.sdrWhiteNits = facts.sdrWhiteNits ? facts.sdrWhiteNits : DefaultSdrWhiteNits;
	m_Facts.hasHdrMetadata = m_RingFormat == WireFormat::Hdr && m_Hdr.HasCurrent();
	m_Facts.hdr = m_Hdr.Current();
}

void HostExporter::Announce()
{
	if (!m_FramePending)
	{
		return;
	}
	m_FramePending = false;

	if (m_WrittenSlot == GH_NO_SLOT)
	{
		m_Link.FrameSkipped();
	}
	else
	{
		m_Link.FrameWritten(m_WrittenSlot, m_Facts);
	}
}

}
