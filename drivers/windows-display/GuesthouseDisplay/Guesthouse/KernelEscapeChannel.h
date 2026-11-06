#pragma once

#include "EscapeChannel.h"

#include <windows.h>

namespace Guesthouse
{

// Talks to the host through the render adapter's D3DKMTEscape. In a GPU-PV guest that call leaves the VM: what would
// have gone to the kernel driver goes to the daemon, and the reply comes back in the same buffer.
class KernelEscapeChannel : public EscapeChannel
{
public:
	explicit KernelEscapeChannel(LUID renderAdapter);
	~KernelEscapeChannel() override;
	KernelEscapeChannel(const KernelEscapeChannel&) = delete;
	KernelEscapeChannel& operator=(const KernelEscapeChannel&) = delete;

	bool Exchange(void* message, std::uint32_t size) override;

private:
	bool Open();

	LUID m_Adapter;
	UINT m_Handle = 0;
	HMODULE m_Gdi = nullptr;
};

}
