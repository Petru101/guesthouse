@echo off
rem Windows runs this as SYSTEM at the end of setup, before anyone logs on.
mkdir C:\gpupv
for %%i in (D E F G H) do if exist %%i:\setup.ps1 powershell -NoProfile -ExecutionPolicy Bypass -File %%i:\setup.ps1 > C:\gpupv\setup-out.log 2>&1
