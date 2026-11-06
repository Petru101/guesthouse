#include "ColourTransform.h"

namespace Guesthouse
{

namespace
{

using Matrix = std::array<std::array<double, 3>, 3>;

// RGB to CIEXYZ with a D65 white, from each standard's primaries.
constexpr Matrix Bt709ToXyz = {{
	{0.4123907993, 0.3575843394, 0.1804807884},
	{0.2126390059, 0.7151686788, 0.0721923154},
	{0.0193308187, 0.1191947798, 0.9505321522},
}};

constexpr Matrix Bt2020ToXyz = {{
	{0.6369580483, 0.1446169036, 0.1688809752},
	{0.2627002120, 0.6779980715, 0.0593017165},
	{0.0000000000, 0.0280726930, 1.0609850577},
}};

Matrix Multiply(const Matrix& a, const Matrix& b)
{
	Matrix result = {};
	for (int row = 0; row < 3; ++row)
	{
		for (int column = 0; column < 3; ++column)
		{
			for (int i = 0; i < 3; ++i)
			{
				result[row][column] += a[row][i] * b[i][column];
			}
		}
	}
	return result;
}

Matrix Inverse(const Matrix& m)
{
	const double c00 = m[1][1] * m[2][2] - m[1][2] * m[2][1];
	const double c01 = m[1][2] * m[2][0] - m[1][0] * m[2][2];
	const double c02 = m[1][0] * m[2][1] - m[1][1] * m[2][0];
	const double scale = 1.0 / (m[0][0] * c00 + m[0][1] * c01 + m[0][2] * c02);

	return {{
		{c00 * scale, (m[0][2] * m[2][1] - m[0][1] * m[2][2]) * scale, (m[0][1] * m[1][2] - m[0][2] * m[1][1]) * scale},
		{c01 * scale, (m[0][0] * m[2][2] - m[0][2] * m[2][0]) * scale, (m[0][2] * m[1][0] - m[0][0] * m[1][2]) * scale},
		{c02 * scale, (m[0][1] * m[2][0] - m[0][0] * m[2][1]) * scale, (m[0][0] * m[1][1] - m[0][1] * m[1][0]) * scale},
	}};
}

const Matrix& ToXyz(Primaries primaries)
{
	return primaries == Primaries::Bt2020 ? Bt2020ToXyz : Bt709ToXyz;
}

}

Rows ComposeToWire(Primaries source, WireFormat wire, const GammaTransform& transform)
{
	const Matrix fromXyz = Inverse(ToXyz(wire == WireFormat::Hdr ? Primaries::Bt2020 : Primaries::Bt709));
	Matrix inXyz = ToXyz(source);
	double offset[3] = {};

	if (transform.matrixEnabled)
	{
		// The scalar multiplies every element, the offsets too.
		Matrix windows = {};
		for (int row = 0; row < 3; ++row)
		{
			for (int column = 0; column < 3; ++column)
			{
				windows[row][column] = static_cast<double>(transform.matrix[row][column]) * transform.scalar;
			}
			offset[row] = static_cast<double>(transform.matrix[row][3]) * transform.scalar;
		}
		inXyz = Multiply(windows, inXyz);
	}

	const Matrix whole = Multiply(fromXyz, inXyz);
	Rows rows = {};
	for (int row = 0; row < 3; ++row)
	{
		double wireOffset = 0.0;
		for (int column = 0; column < 3; ++column)
		{
			rows.value[row][column] = static_cast<float>(whole[row][column]);
			wireOffset += fromXyz[row][column] * offset[column];
		}
		rows.value[row][3] = static_cast<float>(wireOffset);
	}
	return rows;
}

}
