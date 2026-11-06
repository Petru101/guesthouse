# GuesthouseDisplay Logging

GuesthouseDisplay uses TraceLogging ETW as the only driver logging backend.

- Provider name: `Guesthouse.Display`
- Provider GUID: `{796F9ADF-3077-5F37-8589-777C9F74F647}`
- Event name: `Log`
- Event fields: `Level`, `Category`, `Message`, `SourceFile`, `Function`, `Line`

Capture a trace:

```powershell
$traceDir = Join-Path $env:TEMP "GuesthouseDisplay"
New-Item -ItemType Directory -Force -Path $traceDir | Out-Null
$etlPath = Join-Path $traceDir "GuesthouseDisplayTrace.etl"
$csvPath = Join-Path $traceDir "GuesthouseDisplayTrace.csv"

logman start GuesthouseDisplayTrace -ets -p "{796F9ADF-3077-5F37-8589-777C9F74F647}" 0xFFFFFFFF 0x5 -o $etlPath
# Reproduce the issue.
logman stop GuesthouseDisplayTrace -ets
tracerpt $etlPath -of CSV -o $csvPath
```

ETW levels follow the Windows convention: error `2`, warning `3`, info `4`, verbose/debug `5`.
The driver does not select a storage location, write log files, or stream logs through the legacy control pipe.
The `.etl` path belongs to the capture session. Keep traces in a non-shared support directory and delete them after use, because logs can contain device names, configuration values, and timing information.
