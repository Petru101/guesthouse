#pragma once

#include <guesthouse_display_protocol.h>

namespace Guesthouse
{

// IddCx says each frame whether to use new metadata, the monitor's default, or the last frame's. The values below are
// IDDCX_HDR10_FRAME_METADATA_TYPE's, kept apart so that this can be tested without the WDK.
enum class HdrMetadataChange
{
	None = 0,
	UseDefault = 1,
	Unchanged = 2,
	New = 3,
};

class HdrMetadataTracker
{
public:
	void SetDefault(const gh_hdr_metadata& metadata)
	{
		m_Default = metadata;
		m_HasDefault = true;
	}

	void Apply(HdrMetadataChange change, const gh_hdr_metadata& fresh)
	{
		switch (change)
		{
		case HdrMetadataChange::New:
			m_Current = fresh;
			m_HasCurrent = true;
			break;
		case HdrMetadataChange::UseDefault:
			m_Current = m_Default;
			m_HasCurrent = m_HasDefault;
			break;
		default:
			break;
		}
	}

	bool HasCurrent() const { return m_HasCurrent; }
	const gh_hdr_metadata& Current() const { return m_Current; }

private:
	gh_hdr_metadata m_Default = {};
	gh_hdr_metadata m_Current = {};
	bool m_HasDefault = false;
	bool m_HasCurrent = false;
};

}
