#include "check.h"
#include "hlsl_shim.h"
#include "../ZakoVDD/Guesthouse/Shaders/ColourMath.hlsli"
#include "../ZakoVDD/Guesthouse/ColourTransform.h"

using namespace Guesthouse;

static const float NO_SDR_BOOST = 1.0f;

static float3 Through(const Rows& rows, float3 colour)
{
	const auto row = [&](int i) { return float4(rows.value[i][0], rows.value[i][1], rows.value[i][2], rows.value[i][3]); };
	return ApplyRows(row(0), row(1), row(2), colour);
}

// With no transform, SDR through the pass must come out as it went in, for every 8-bit value.
static void Sdr_is_bit_exact_through_the_whole_pipeline()
{
	const Rows rows = ComposeToWire(Primaries::Bt709, WireFormat::Sdr, GammaTransform{});
	for (uint value = 0; value < 256; ++value)
	{
		const float texel = value / 255.0f;
		float3 light = DecodeSource(SOURCE_SRGB, float3(texel, texel, texel), NO_SDR_BOOST);
		light = Through(rows, light);
		const uint packed = PackXrgb8888(EncodeWire(WIRE_SDR, light, NO_SDR_BOOST));
		CHECK(packed == (value << 16 | value << 8 | value));
	}
}

static void Pq_matches_its_published_points()
{
	CHECK_NEAR(NitsToPq1(0.0f), 0.0, 1e-6);
	CHECK_NEAR(NitsToPq1(100.0f), 0.5081, 0.0002);
	CHECK_NEAR(NitsToPq1(1000.0f), 0.7518, 0.0002);
	CHECK_NEAR(NitsToPq1(10000.0f), 1.0, 1e-5);
	CHECK_NEAR(PqToNits1(1.0f), 10000.0, 0.5);
}

static void Pq_round_trips()
{
	for (float nits : {0.1f, 1.0f, 80.0f, 203.0f, 1000.0f, 4000.0f})
	{
		CHECK_NEAR(PqToNits1(NitsToPq1(nits)), nits, nits * 0.001);
	}
}

// scRGB's 1.0 is 80 nits, so white at 12.5 is 1,000 nits on the wire.
static void Scrgb_white_lands_at_the_right_brightness()
{
	const Rows rows = ComposeToWire(Primaries::Bt709, WireFormat::Hdr, GammaTransform{});
	const float3 light = Through(rows, DecodeSource(SOURCE_SCRGB, float3(12.5f, 12.5f, 12.5f), NO_SDR_BOOST));
	const float3 encoded = EncodeWire(WIRE_HDR, light, NO_SDR_BOOST);
	CHECK_NEAR(encoded.x, 0.7518, 0.0005);
	CHECK_NEAR(encoded.y, 0.7518, 0.0005);
	CHECK_NEAR(encoded.z, 0.7518, 0.0005);
}

// On an HDR desktop, SDR white is where the user's slider put it, not 80 nits.
static void Sdr_content_sits_at_the_sdr_white_level()
{
	const float sdrWhite = 240.0f / SCRGB_NITS;
	const Rows rows = ComposeToWire(Primaries::Bt709, WireFormat::Hdr, GammaTransform{});
	const float3 light = Through(rows, DecodeSource(SOURCE_SRGB, float3(1, 1, 1), sdrWhite));
	const float3 encoded = EncodeWire(WIRE_HDR, light, sdrWhite);
	CHECK_NEAR(PqToNits1(encoded.x), 240.0, 0.5);
}

static void Pq_input_passes_through_unchanged()
{
	const Rows rows = ComposeToWire(Primaries::Bt2020, WireFormat::Hdr, GammaTransform{});
	const float3 texel(0.3f, 0.5f, 0.7f);
	const float3 encoded = EncodeWire(WIRE_HDR, Through(rows, DecodeSource(SOURCE_PQ, texel, NO_SDR_BOOST)), NO_SDR_BOOST);
	CHECK_NEAR(encoded.x, 0.3, 0.0005);
	CHECK_NEAR(encoded.y, 0.5, 0.0005);
	CHECK_NEAR(encoded.z, 0.7, 0.0005);
}

static void Bt709_to_bt2020_is_the_published_matrix()
{
	const Rows rows = ComposeToWire(Primaries::Bt709, WireFormat::Hdr, GammaTransform{});
	CHECK_NEAR(rows.value[0][0], 0.6274, 0.0002);
	CHECK_NEAR(rows.value[0][1], 0.3293, 0.0002);
	CHECK_NEAR(rows.value[0][2], 0.0433, 0.0002);
	CHECK_NEAR(rows.value[1][1], 0.9195, 0.0002);
	CHECK_NEAR(rows.value[2][2], 0.8956, 0.0002);
}

static void An_identity_transform_changes_nothing()
{
	GammaTransform transform;
	transform.matrixEnabled = true;
	transform.matrix[0][0] = transform.matrix[1][1] = transform.matrix[2][2] = 1.0f;
	const Rows plain = ComposeToWire(Primaries::Bt709, WireFormat::Hdr, GammaTransform{});
	const Rows same = ComposeToWire(Primaries::Bt709, WireFormat::Hdr, transform);
	for (int row = 0; row < 3; ++row)
	{
		for (int column = 0; column < 4; ++column)
		{
			CHECK_NEAR(same.value[row][column], plain.value[row][column], 1e-6);
		}
	}
}

static void The_scalar_multiplies_the_matrix_and_its_offset()
{
	GammaTransform transform;
	transform.matrixEnabled = true;
	transform.matrix[0][0] = transform.matrix[1][1] = transform.matrix[2][2] = 1.0f;
	transform.scalar = 0.5f;
	const Rows halved = ComposeToWire(Primaries::Bt709, WireFormat::Sdr, transform);
	const float3 white = Through(halved, float3(1, 1, 1));
	CHECK_NEAR(white.x, 0.5, 1e-5);
	CHECK_NEAR(white.y, 0.5, 1e-5);
	CHECK_NEAR(white.z, 0.5, 1e-5);

	// An offset of the white point's XYZ adds one white, in wire RGB, after the scalar has halved it.
	transform.matrix[0][3] = 0.95047f;
	transform.matrix[1][3] = 1.0f;
	transform.matrix[2][3] = 1.08883f;
	const float3 black = Through(ComposeToWire(Primaries::Bt709, WireFormat::Sdr, transform), float3(0, 0, 0));
	CHECK_NEAR(black.x, 0.5, 0.001);
	CHECK_NEAR(black.y, 0.5, 0.001);
	CHECK_NEAR(black.z, 0.5, 0.001);
}

static void Channels_land_where_drm_says()
{
	CHECK(PackXrgb8888(float3(1, 0, 0)) == 0x00FF0000u);
	CHECK(PackXrgb8888(float3(0, 0, 1)) == 0x000000FFu);
	CHECK(PackXbgr2101010(float3(1, 0, 0)) == 0x000003FFu);
	CHECK(PackXbgr2101010(float3(0, 1, 0)) == 0x000FFC00u);
	CHECK(PackXbgr2101010(float3(0, 0, 1)) == 0x3FF00000u);
}

static void Out_of_range_never_wraps()
{
	CHECK(PackXrgb8888(float3(2.0f, -1.0f, 0)) == 0x00FF0000u);
	const float3 encoded = EncodeWire(WIRE_HDR, float3(-1.0f, 1000.0f, 0), NO_SDR_BOOST);
	CHECK_NEAR(encoded.x, 0.0, 1e-6);
	CHECK_NEAR(encoded.y, 1.0, 1e-5);
}

int main()
{
	Sdr_is_bit_exact_through_the_whole_pipeline();
	Pq_matches_its_published_points();
	Pq_round_trips();
	Scrgb_white_lands_at_the_right_brightness();
	Sdr_content_sits_at_the_sdr_white_level();
	Pq_input_passes_through_unchanged();
	Bt709_to_bt2020_is_the_published_matrix();
	An_identity_transform_changes_nothing();
	The_scalar_multiplies_the_matrix_and_its_offset();
	Channels_land_where_drm_says();
	Out_of_range_never_wraps();
	std::printf("colour_math_test: %d failures\n", failures);
	return failures != 0;
}
