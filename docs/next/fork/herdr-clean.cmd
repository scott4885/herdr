@echo off
setlocal
set ANTHROPIC_API_KEY=
set GIT_ASKPASS=
set CLAUDE_CODE_SSE_PORT=
set ELECTRON_RUN_AS_NODE=
for /f "delims==" %%v in ('set VSCODE_ 2^>nul') do set "%%v="
"%LOCALAPPDATA%\Programs\Herdr\bin\herdr.exe" %*
