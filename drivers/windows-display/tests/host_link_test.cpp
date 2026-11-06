#include "check.h"
#include "fake_daemon.h"
#include "../ZakoVDD/Guesthouse/CursorLink.h"
#include "../ZakoVDD/Guesthouse/HdrMetadataTracker.h"
#include "../ZakoVDD/Guesthouse/HostLink.h"

#include <cstring>
#include <vector>

using namespace Guesthouse;

static RingShape Shape(WireFormat format)
{
	RingShape shape;
	shape.slots = 6;
	shape.width = 3840;
	shape.height = 2160;
	shape.slotSize = static_cast<std::uint64_t>(PitchFor(3840)) * 2160;
	shape.format = format;
	shape.monitor = 2;
	return shape;
}

static void Registers_the_ring_and_gets_its_first_slot()
{
	FakeDaemon daemon;
	daemon.freeSlots = {4};
	HostLink link(daemon);

	CHECK(link.OpenRing(Shape(WireFormat::Hdr)));
	CHECK(link.NextSlot() == 4);
	CHECK(daemon.rings.size() == 1);
	const gh_register& ring = daemon.rings[0];
	CHECK(ring.header.generation == 1);
	CHECK(ring.header.monitor == 2);
	CHECK(ring.fourcc == GH_FOURCC_XBGR2101010);
	CHECK(ring.cicp_primaries == GH_CICP_PRIMARIES_BT2020);
	CHECK(ring.cicp_transfer == GH_CICP_TRANSFER_PQ);
	CHECK(ring.pitch == 15360);
	CHECK(ring.slot_size == 15360ull * 2160);
	CHECK(link.PeakNits() == 1000);
}

static void Sdr_rings_say_so()
{
	FakeDaemon daemon;
	HostLink link(daemon);
	link.OpenRing(Shape(WireFormat::Sdr));
	CHECK(daemon.rings[0].fourcc == GH_FOURCC_XRGB8888);
	CHECK(daemon.rings[0].cicp_transfer == GH_CICP_TRANSFER_SRGB);
}

static void Writes_only_where_the_last_reply_said()
{
	FakeDaemon daemon;
	daemon.freeSlots = {0, 2};
	HostLink link(daemon);
	link.OpenRing(Shape(WireFormat::Sdr));

	link.FrameWritten(link.NextSlot(), FrameFacts{});
	CHECK(daemon.frames.back().slot == 0);
	CHECK(daemon.frames.back().sequence == 1);
	CHECK(daemon.frames.back().header.monitor == 2);
	CHECK(link.NextSlot() == 2);

	link.FrameWritten(link.NextSlot(), FrameFacts{});
	CHECK(daemon.frames.back().slot == 2);
	CHECK(daemon.frames.back().sequence == 2);
	CHECK(link.NextSlot() == GH_NO_SLOT);
}

static void A_skipped_frame_asks_for_a_slot_without_claiming_one()
{
	FakeDaemon daemon;
	HostLink link(daemon);
	link.OpenRing(Shape(WireFormat::Sdr));
	CHECK(link.NextSlot() == GH_NO_SLOT);

	daemon.freeSlots = {3};
	link.FrameSkipped();
	CHECK(daemon.frames.back().slot == GH_NO_SLOT);
	CHECK(daemon.frames.back().sequence == 0);
	CHECK(link.NextSlot() == 3);
}

static void Hdr_metadata_goes_with_the_frame_only_when_there_is_some()
{
	FakeDaemon daemon;
	daemon.freeSlots = {0, 1, 2};
	HostLink link(daemon);
	link.OpenRing(Shape(WireFormat::Hdr));

	FrameFacts facts;
	facts.sdrWhiteNits = 240;
	facts.presentQpc = 1234;
	facts.windowsFrame = 7;
	link.FrameWritten(link.NextSlot(), facts);
	CHECK((daemon.frames.back().flags & GH_FRAME_HAS_HDR_METADATA) == 0);
	CHECK(daemon.frames.back().sdr_white_nits == 240);
	CHECK(daemon.frames.back().present_qpc == 1234);
	CHECK(daemon.frames.back().windows_frame == 7);

	facts.hasHdrMetadata = true;
	facts.hdr.max_content_nits = 900;
	link.FrameWritten(link.NextSlot(), facts);
	CHECK((daemon.frames.back().flags & GH_FRAME_HAS_HDR_METADATA) != 0);
	CHECK(daemon.frames.back().hdr.max_content_nits == 900);
}

static void A_daemon_without_the_protocol_means_giving_up_quietly()
{
	FakeDaemon daemon;
	daemon.speaksProtocol = false;
	HostLink link(daemon);

	CHECK(!link.OpenRing(Shape(WireFormat::Sdr)));
	CHECK(link.GaveUp());
	CHECK(!link.RingOpen());
	CHECK(link.NextSlot() == GH_NO_SLOT);
}

// An echoed request can look like a good reply, as status lands on a field that may be zero. Only the magic tells them apart.
static void An_echo_is_never_taken_for_a_reply()
{
	FakeDaemon daemon;
	daemon.speaksProtocol = false;
	HostLink link(daemon);
	RingShape shape = Shape(WireFormat::Sdr);
	shape.slots = 0;

	CHECK(!link.OpenRing(shape));
	CHECK(link.GaveUp());
}

static void A_refusal_stops_until_the_mode_changes()
{
	FakeDaemon daemon;
	daemon.statusToAnswer = GH_STATUS_REFUSED;
	HostLink link(daemon);

	CHECK(!link.OpenRing(Shape(WireFormat::Sdr)));
	CHECK(link.GaveUp());
	link.TryAgain();
	CHECK(!link.GaveUp());
}

static void A_stale_generation_closes_the_ring_but_does_not_give_up()
{
	FakeDaemon daemon;
	daemon.freeSlots = {1};
	HostLink link(daemon);
	link.OpenRing(Shape(WireFormat::Sdr));

	daemon.statusToAnswer = GH_STATUS_STALE_GENERATION;
	link.FrameWritten(link.NextSlot(), FrameFacts{});
	CHECK(!link.RingOpen());
	CHECK(!link.GaveUp());
	CHECK(link.NextSlot() == GH_NO_SLOT);
	CHECK(link.NextGeneration() == 2);
}

static void A_failed_escape_loses_the_slot()
{
	FakeDaemon daemon;
	daemon.freeSlots = {1};
	HostLink link(daemon);
	link.OpenRing(Shape(WireFormat::Sdr));

	daemon.reachable = false;
	link.FrameWritten(link.NextSlot(), FrameFacts{});
	CHECK(link.NextSlot() == GH_NO_SLOT);
	CHECK(!link.RingOpen());
}

static void Closing_tells_the_daemon_once()
{
	FakeDaemon daemon;
	HostLink link(daemon);
	link.OpenRing(Shape(WireFormat::Sdr));
	link.CloseRing();
	link.CloseRing();
	CHECK(daemon.closes == 1);
}

static void A_new_ring_has_a_new_generation_and_restarts_the_count()
{
	FakeDaemon daemon;
	daemon.freeSlots = {0, 1, 2};
	HostLink link(daemon);
	link.OpenRing(Shape(WireFormat::Sdr));
	link.FrameWritten(link.NextSlot(), FrameFacts{});
	link.CloseRing();

	CHECK(link.NextGeneration() == 2);
	link.OpenRing(Shape(WireFormat::Hdr));
	CHECK(daemon.rings.back().header.generation == 2);
	link.FrameWritten(link.NextSlot(), FrameFacts{});
	CHECK(daemon.frames.back().header.generation == 2);
	CHECK(daemon.frames.back().sequence == 1);
}

static void Pitches_are_aligned()
{
	CHECK(PitchFor(1920) == 7680);
	CHECK(PitchFor(1731) == 7168);
	CHECK(PitchFor(1) == PitchAlignment);
}

// Plays a daemon for cursor messages: it keeps each one, and answers with what it was told to.
class CursorDaemon : public EscapeChannel
{
public:
	std::uint32_t statusToAnswer = GH_STATUS_OK;
	std::vector<std::vector<std::uint8_t>> messages;

	bool Exchange(void* message, std::uint32_t size) override
	{
		const auto* bytes = static_cast<const std::uint8_t*>(message);
		messages.emplace_back(bytes, bytes + size);
		gh_reply reply = {};
		reply.header.magic = GH_REPLY_MAGIC;
		reply.status = statusToAnswer;
		std::memcpy(message, &reply, sizeof(reply));
		return true;
	}
};

static void A_big_cursor_goes_in_pieces_that_add_up()
{
	CursorDaemon daemon;
	CursorLink link(daemon, 1);
	const CursorPicture picture = {7, 128, 128, 512, 3, 4};
	std::vector<std::uint8_t> pixels(512 * 128);
	for (std::size_t i = 0; i < pixels.size(); ++i)
	{
		pixels[i] = static_cast<std::uint8_t>(i * 31);
	}

	link.SendShape(picture, pixels.data());

	CHECK(daemon.messages.size() == 2);
	std::vector<std::uint8_t> joined;
	for (const auto& bytes : daemon.messages)
	{
		gh_cursor_shape piece;
		std::memcpy(&piece, bytes.data(), sizeof(piece));
		CHECK(piece.header.kind == GH_KIND_CURSOR_SHAPE);
		CHECK(piece.header.monitor == 1);
		CHECK(piece.header.size == bytes.size());
		CHECK(piece.total_bytes == 512 * 128);
		CHECK(piece.piece_offset == joined.size());
		CHECK(piece.piece_bytes == GH_CURSOR_PIECE_BYTES);
		joined.insert(joined.end(), bytes.begin() + sizeof(piece), bytes.end());
	}
	CHECK(joined == pixels);
}

static void A_small_cursor_is_one_short_piece()
{
	CursorDaemon daemon;
	CursorLink link(daemon, 0);
	const CursorPicture picture = {1, 32, 32, 128, 0, 0};
	std::vector<std::uint8_t> pixels(128 * 32);
	link.SendShape(picture, pixels.data());
	CHECK(daemon.messages.size() == 1);
	CHECK(daemon.messages[0].size() == sizeof(gh_cursor_shape) + pixels.size());
}

static void The_reply_says_who_draws_the_cursor()
{
	CursorDaemon daemon;
	CursorLink link(daemon, 0);
	CHECK(!link.ViewerDrawsIt());
	link.SendMove(-3, 40, true, 240);
	CHECK(link.ViewerDrawsIt());

	gh_cursor_move move;
	std::memcpy(&move, daemon.messages[0].data(), sizeof(move));
	CHECK(move.x == -3 && move.y == 40 && move.visible == 1 && move.sdr_white_nits == 240);

	daemon.statusToAnswer = GH_STATUS_CURSOR_NOT_WANTED;
	link.SendMove(0, 0, true, 80);
	CHECK(!link.ViewerDrawsIt());
}

static void Hdr_metadata_follows_what_each_frame_says()
{
	HdrMetadataTracker tracker;
	gh_hdr_metadata fresh = {};
	fresh.max_content_nits = 500;
	gh_hdr_metadata fallback = {};
	fallback.max_content_nits = 1000;

	tracker.Apply(HdrMetadataChange::UseDefault, fresh);
	CHECK(!tracker.HasCurrent());

	tracker.SetDefault(fallback);
	tracker.Apply(HdrMetadataChange::UseDefault, fresh);
	CHECK(tracker.Current().max_content_nits == 1000);

	tracker.Apply(HdrMetadataChange::New, fresh);
	CHECK(tracker.Current().max_content_nits == 500);

	gh_hdr_metadata ignored = {};
	tracker.Apply(HdrMetadataChange::Unchanged, ignored);
	CHECK(tracker.Current().max_content_nits == 500);

	// New metadata must not replace the default.
	tracker.Apply(HdrMetadataChange::UseDefault, ignored);
	CHECK(tracker.Current().max_content_nits == 1000);
}

int main()
{
	Registers_the_ring_and_gets_its_first_slot();
	Sdr_rings_say_so();
	Writes_only_where_the_last_reply_said();
	A_skipped_frame_asks_for_a_slot_without_claiming_one();
	Hdr_metadata_goes_with_the_frame_only_when_there_is_some();
	A_daemon_without_the_protocol_means_giving_up_quietly();
	An_echo_is_never_taken_for_a_reply();
	A_refusal_stops_until_the_mode_changes();
	A_stale_generation_closes_the_ring_but_does_not_give_up();
	A_failed_escape_loses_the_slot();
	Closing_tells_the_daemon_once();
	A_new_ring_has_a_new_generation_and_restarts_the_count();
	Pitches_are_aligned();
	Hdr_metadata_follows_what_each_frame_says();
	A_big_cursor_goes_in_pieces_that_add_up();
	A_small_cursor_is_one_short_piece();
	The_reply_says_who_draws_the_cursor();
	std::printf("host_link_test: %d failures\n", failures);
	return failures != 0;
}
