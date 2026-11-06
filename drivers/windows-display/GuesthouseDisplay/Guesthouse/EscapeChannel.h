#pragma once

#include <cstdint>

namespace Guesthouse
{

// One request and its reply, through the same buffer. It's an interface so that the ring logic can be tested off Windows.
class EscapeChannel
{
public:
	virtual ~EscapeChannel() = default;

	// False if the message never got to the host. The caller works out from the buffer whether the host understood it.
	virtual bool Exchange(void* message, std::uint32_t size) = 0;
};

}
