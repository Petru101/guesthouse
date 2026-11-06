#pragma once

#include "../ZakoVDD/Guesthouse/EscapeChannel.h"

#include <guesthouse_display_protocol.h>

#include <cstring>
#include <vector>

// Plays the daemon by the rules in the design: it alone says which slot is free.
class FakeDaemon : public Guesthouse::EscapeChannel
{
public:
	bool reachable = true;
	bool speaksProtocol = true;
	std::uint32_t statusToAnswer = GH_STATUS_OK;
	std::vector<std::uint32_t> freeSlots;
	std::vector<gh_frame> frames;
	std::vector<gh_register> rings;
	int closes = 0;

	bool Exchange(void* message, std::uint32_t size) override
	{
		if (!reachable)
		{
			return false;
		}
		if (!speaksProtocol)
		{
			return true; // An escape nobody claims comes back as it went.
		}

		gh_header header;
		std::memcpy(&header, message, sizeof(header));
		switch (header.kind)
		{
		case GH_KIND_REGISTER:
			rings.push_back(*static_cast<gh_register*>(message));
			break;
		case GH_KIND_FRAME:
			frames.push_back(*static_cast<gh_frame*>(message));
			break;
		case GH_KIND_CLOSE:
			++closes;
			break;
		}

		gh_reply reply = {};
		reply.header = header;
		reply.header.magic = GH_REPLY_MAGIC;
		reply.header.size = sizeof(reply);
		reply.status = statusToAnswer;
		reply.next_slot = GH_NO_SLOT;
		if (!freeSlots.empty())
		{
			reply.next_slot = freeSlots.front();
			freeSlots.erase(freeSlots.begin());
		}
		reply.monitor_millihertz = 240000;
		reply.peak_nits = 1000;
		std::memcpy(message, &reply, sizeof(reply));
		(void)size;
		return true;
	}
};
