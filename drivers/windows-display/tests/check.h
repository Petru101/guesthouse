#pragma once

#include <cmath>
#include <cstdio>

inline int failures = 0;

#define CHECK(condition) \
	do { if (!(condition)) { std::printf("%s:%d: %s\n", __FILE__, __LINE__, #condition); ++failures; } } while (0)

#define CHECK_NEAR(actual, expected, margin) \
	do { const double a_ = (actual), e_ = (expected); if (std::fabs(a_ - e_) > (margin)) { \
		std::printf("%s:%d: %s is %f, not %f\n", __FILE__, __LINE__, #actual, a_, e_); ++failures; } } while (0)
