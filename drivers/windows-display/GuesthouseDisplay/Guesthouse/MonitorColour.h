#pragma once

#include "ColourTransform.h"
#include "HdrMetadataTracker.h"

#include <cstdint>
#include <memory>
#include <mutex>
#include <utility>

namespace Guesthouse
{

// What Windows has told us about one monitor's colour. It outlives swap chains, as a mode change makes a new swap chain and
// Windows doesn't repeat itself. IddCx callbacks write it from their own threads and the frame thread reads it.
class MonitorColour
{
public:
	void SetTransform(GammaTransform transform)
	{
		std::lock_guard<std::mutex> lock(m_Mutex);
		m_Transform = std::make_shared<const GammaTransform>(std::move(transform));
		++m_Version;
	}

	void SetDefaultHdrMetadata(const gh_hdr_metadata& metadata)
	{
		std::lock_guard<std::mutex> lock(m_Mutex);
		m_DefaultHdr = metadata;
		m_HasDefaultHdr = true;
	}

	// The transform and its version, so that the frame thread only uploads what changed.
	std::shared_ptr<const GammaTransform> Transform(std::uint64_t& version) const
	{
		std::lock_guard<std::mutex> lock(m_Mutex);
		version = m_Version;
		return m_Transform;
	}

	bool DefaultHdrMetadata(gh_hdr_metadata& metadata) const
	{
		std::lock_guard<std::mutex> lock(m_Mutex);
		metadata = m_DefaultHdr;
		return m_HasDefaultHdr;
	}

private:
	mutable std::mutex m_Mutex;
	std::shared_ptr<const GammaTransform> m_Transform = std::make_shared<const GammaTransform>();
	std::uint64_t m_Version = 1;
	gh_hdr_metadata m_DefaultHdr = {};
	bool m_HasDefaultHdr = false;
};

}
