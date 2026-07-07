@echo off
rem ===================================================================
rem  moterm Windows build (double-click or command prompt).
rem  Delegates to the PowerShell script (windows-build.ps1).
rem
rem  Usage:
rem    windows-build.bat                     release build only
rem    windows-build.bat <DEST>              build, then copy moterm.exe to <DEST>
rem    windows-build.bat deploy              build, then copy to %USERPROFILE%\moterm
rem    windows-build.bat deploy <DEST>       build, then copy to <DEST>
rem    windows-build.bat run                 build, then launch
rem    windows-build.bat debug               debug build
rem
rem  Examples:
rem    windows-build.bat D:\apps\moterm
rem    windows-build.bat deploy "D:\my apps\moterm"
rem
rem  Requires: Rust (stable, MSVC) and VS Build Tools (C++).
rem  Only native dependency is the MSVC C compiler for the vendored Lua
rem  (OpenSSL / libssh2 / CMake / NASM / Perl are NOT needed).
rem ===================================================================
setlocal

rem 最新のソースを取得してからビルドする（git pull が失敗しても続行）。
echo [git] pulling latest...
git -C "%~dp0.." pull
if errorlevel 1 echo [WARN] git pull skipped/failed - continuing with local files.

set "PS1=%~dp0windows-build.ps1"
set "MODE=%~1"

if /I "%MODE%"=="run"    goto :run
if /I "%MODE%"=="debug"  goto :debug
if /I "%MODE%"=="deploy" goto :deploy
if not "%MODE%"==""      goto :deploy_dest
rem no argument -> plain release build
call :ps
goto :finish

:run
call :ps -Run
goto :finish

:debug
call :ps -DebugBuild
goto :finish

:deploy
rem "deploy" with an optional destination folder as the 2nd argument
if "%~2"=="" (
    call :ps -Deploy
) else (
    call :ps -Deploy -Dest "%~2"
)
goto :finish

:deploy_dest
rem first argument is a destination folder -> build and copy the exe there
call :ps -Deploy -Dest "%MODE%"
goto :finish

:ps
powershell -NoProfile -ExecutionPolicy Bypass -File "%PS1%" %*
exit /b %ERRORLEVEL%

:finish
set "RC=%ERRORLEVEL%"
if not "%RC%"=="0" (
    echo.
    echo [ERROR] build failed with exit code %RC%
    pause
    exit /b %RC%
)
echo.
echo [OK] build finished.
endlocal
