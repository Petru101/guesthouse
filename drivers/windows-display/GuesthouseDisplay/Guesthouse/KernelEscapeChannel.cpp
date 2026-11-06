#include "KernelEscapeChannel.h"

#include "..\Logging\Logger.h"

namespace Guesthouse
{

namespace
{

// These three are copied from d3dkmthk.h's public layout. That header sits in the kernel half of the WDK, and gdi32 isn't in
// the OneCore set a universal driver links against, so the entry points are looked up at run time anyway.
struct OpenAdapterFromLuid
{
	LUID adapterLuid;
	UINT adapter;
};

struct CloseAdapter
{
	UINT adapter;
};

struct Escape
{
	UINT adapter;
	UINT device;
	UINT type;
	UINT flags;
	void* privateData;
	UINT privateDataSize;
	UINT context;
};

// D3DKMT_ESCAPE_DRIVERPRIVATE. Flags stay zero: Microsoft's docs say HardwareAccess must be off under virtualization.
constexpr UINT DriverPrivateEscape = 0;

using OpenFunction = LONG(APIENTRY*)(OpenAdapterFromLuid*);
using CloseFunction = LONG(APIENTRY*)(const CloseAdapter*);
using EscapeFunction = LONG(APIENTRY*)(const Escape*);

template <typename Function>
Function Find(HMODULE module, const char* name)
{
	return reinterpret_cast<Function>(reinterpret_cast<void*>(GetProcAddress(module, name)));
}

}

KernelEscapeChannel::KernelEscapeChannel(LUID renderAdapter) : m_Adapter(renderAdapter)
{
}

KernelEscapeChannel::~KernelEscapeChannel()
{
	if (m_Handle != 0)
	{
		if (auto close = Find<CloseFunction>(m_Gdi, "D3DKMTCloseAdapter"))
		{
			CloseAdapter arguments = {m_Handle};
			close(&arguments);
		}
	}
	if (m_Gdi)
	{
		FreeLibrary(m_Gdi);
	}
}

bool KernelEscapeChannel::Open()
{
	if (m_Handle != 0)
	{
		return true;
	}

	if (!m_Gdi)
	{
		m_Gdi = LoadLibraryW(L"gdi32.dll");
	}
	auto open = m_Gdi ? Find<OpenFunction>(m_Gdi, "D3DKMTOpenAdapterFromLuid") : nullptr;
	if (!open)
	{
		VDD_LOG_ERROR("[Guesthouse] D3DKMTOpenAdapterFromLuid is not there");
		return false;
	}

	OpenAdapterFromLuid arguments = {m_Adapter, 0};
	const LONG status = open(&arguments);
	if (status < 0)
	{
		VDD_LOG_ERROR_STREAM("[Guesthouse] Opening the render adapter failed: 0x" << std::hex << status);
		return false;
	}
	m_Handle = arguments.adapter;
	return true;
}

bool KernelEscapeChannel::Exchange(void* message, std::uint32_t size)
{
	if (!Open())
	{
		return false;
	}

	auto escape = Find<EscapeFunction>(m_Gdi, "D3DKMTEscape");
	if (!escape)
	{
		VDD_LOG_ERROR("[Guesthouse] D3DKMTEscape is not there");
		return false;
	}

	// No device or context: this process's D3D11 device keeps its kernel handles to itself. The daemon knows the
	// sender by the process in the message's header, and that's what it finds the queues by.
	Escape arguments = {};
	arguments.adapter = m_Handle;
	arguments.type = DriverPrivateEscape;
	arguments.privateData = message;
	arguments.privateDataSize = size;

	const LONG status = escape(&arguments);
	if (status < 0)
	{
		VDD_LOG_WARNING_STREAM("[Guesthouse] D3DKMTEscape failed: 0x" << std::hex << status);
		return false;
	}
	return true;
}

}
