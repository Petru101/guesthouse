#include "Logger.h"

#include <cstring>
#include <windows.h>
#include <winmeta.h>
#include <TraceLoggingProvider.h>

TRACELOGGING_DEFINE_PROVIDER(
	g_VddLogProvider,
	"Guesthouse.Display",
	(0x796f9adf, 0x3077, 0x5f37, 0x85, 0x89, 0x77, 0x7c, 0x9f, 0x74, 0xf6, 0x47));

namespace
{
UCHAR ToTraceLoggingLevel(VddLogLevel level)
{
	switch (level)
	{
	case VddLogLevel::Critical:
		return WINEVENT_LEVEL_CRITICAL;
	case VddLogLevel::Error:
		return WINEVENT_LEVEL_ERROR;
	case VddLogLevel::Warning:
		return WINEVENT_LEVEL_WARNING;
	case VddLogLevel::Debug:
	case VddLogLevel::Verbose:
		return WINEVENT_LEVEL_VERBOSE;
	case VddLogLevel::Info:
	default:
		return WINEVENT_LEVEL_INFO;
	}
}

const char *LevelName(VddLogLevel level)
{
	switch (level)
	{
	case VddLogLevel::Critical:
		return "critical";
	case VddLogLevel::Error:
		return "error";
	case VddLogLevel::Warning:
		return "warning";
	case VddLogLevel::Debug:
		return "debug";
	case VddLogLevel::Verbose:
		return "verbose";
	case VddLogLevel::Info:
	default:
		return "info";
	}
}

const char *SourceFileName(const char *sourceFile)
{
	if (sourceFile == nullptr || sourceFile[0] == '\0')
	{
		return "";
	}

	const char *fileName = sourceFile;
	for (const char *p = sourceFile; *p != '\0'; ++p)
	{
		if (*p == '\\' || *p == '/')
		{
			fileName = p + 1;
		}
	}
	return fileName;
}

const char *CategoryFromSource(const char *sourceFile)
{
	if (sourceFile == nullptr)
	{
		return "driver";
	}

	if (strstr(sourceFile, "\\Adapter\\") || strstr(sourceFile, "/Adapter/")) return "Adapter";
	if (strstr(sourceFile, "\\Callbacks\\") || strstr(sourceFile, "/Callbacks/")) return "Callbacks";
	if (strstr(sourceFile, "\\Config\\") || strstr(sourceFile, "/Config/")) return "Config";
	if (strstr(sourceFile, "\\Control\\") || strstr(sourceFile, "/Control/")) return "Control";
	if (strstr(sourceFile, "\\Core\\") || strstr(sourceFile, "/Core/")) return "Core";
	if (strstr(sourceFile, "\\Device\\") || strstr(sourceFile, "/Device/")) return "Device";
	if (strstr(sourceFile, "\\Diagnostics\\") || strstr(sourceFile, "/Diagnostics/")) return "Diagnostics";
	if (strstr(sourceFile, "\\Edid\\") || strstr(sourceFile, "/Edid/")) return "Edid";
	if (strstr(sourceFile, "\\Logging\\") || strstr(sourceFile, "/Logging/")) return "Logging";
	if (strstr(sourceFile, "\\Rendering\\") || strstr(sourceFile, "/Rendering/")) return "Rendering";
	if (strstr(sourceFile, "\\Util\\") || strstr(sourceFile, "/Util/")) return "Util";
	return "Driver";
}
}

bool VddLogIsEnabled(VddLogLevel level)
{
	return TraceLoggingProviderEnabled(g_VddLogProvider, ToTraceLoggingLevel(level), 0) != FALSE;
}

void VddLogWrite(VddLogLevel level, const char *sourceFile, const char *functionName, unsigned int line, const char *message)
{
	const char *safeMessage = message != nullptr ? message : "";
	const char *safeFunction = functionName != nullptr ? functionName : "";
	const char *sourceName = SourceFileName(sourceFile);
	const char *category = CategoryFromSource(sourceFile);

#define VDD_TRACELOG_WRITE(constantLevel) \
	TraceLoggingWrite( \
		g_VddLogProvider, \
		"Log", \
		TraceLoggingLevel(constantLevel), \
		TraceLoggingString(LevelName(level), "Level"), \
		TraceLoggingString(category, "Category"), \
		TraceLoggingString(safeMessage, "Message"), \
		TraceLoggingString(sourceName, "SourceFile"), \
		TraceLoggingString(safeFunction, "Function"), \
		TraceLoggingUInt32(line, "Line"))

	switch (level)
	{
	case VddLogLevel::Critical:
		VDD_TRACELOG_WRITE(WINEVENT_LEVEL_CRITICAL);
		break;
	case VddLogLevel::Error:
		VDD_TRACELOG_WRITE(WINEVENT_LEVEL_ERROR);
		break;
	case VddLogLevel::Warning:
		VDD_TRACELOG_WRITE(WINEVENT_LEVEL_WARNING);
		break;
	case VddLogLevel::Debug:
	case VddLogLevel::Verbose:
		VDD_TRACELOG_WRITE(WINEVENT_LEVEL_VERBOSE);
		break;
	case VddLogLevel::Info:
	default:
		VDD_TRACELOG_WRITE(WINEVENT_LEVEL_INFO);
		break;
	}

#undef VDD_TRACELOG_WRITE
}

extern "C" BOOL WINAPI DllMain(
	_In_ HINSTANCE hInstance,
	_In_ UINT reason,
	_In_opt_ LPVOID reserved)
{
	UNREFERENCED_PARAMETER(reserved);

	switch (reason)
	{
	case DLL_PROCESS_ATTACH:
		DisableThreadLibraryCalls(hInstance);
		TraceLoggingRegister(g_VddLogProvider);
		break;
	case DLL_PROCESS_DETACH:
		TraceLoggingUnregister(g_VddLogProvider);
		break;
	default:
		break;
	}

	return TRUE;
}
