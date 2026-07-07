<#
.SYNOPSIS
    Build moterm(.exe) for Windows, optionally deploy it.

.DESCRIPTION
    Requires Rust (MSVC toolchain) and VS Build Tools (C++).
    russh uses pure-Rust crypto, so OpenSSL / libssh2 / CMake / NASM / Perl are
    NOT needed. The only native dependency is the MSVC C compiler used to build
    the vendored Lua (mlua).

.PARAMETER Deploy
    Copy the built moterm.exe and a default config into <Dest>.

.PARAMETER Dest
    Deploy target folder (only with -Deploy). Default "$env:USERPROFILE\moterm".

.PARAMETER DebugBuild
    Build the debug profile instead of release (default is release).
    (Named DebugBuild, not Debug, to avoid clashing with the built-in -Debug
    common parameter added by CmdletBinding.)

.PARAMETER Run
    Launch moterm after building.

.EXAMPLE
    powershell -ExecutionPolicy Bypass -File scripts\windows-build.ps1
    powershell -ExecutionPolicy Bypass -File scripts\windows-build.ps1 -Deploy -Dest D:\apps\moterm
#>
[CmdletBinding()]
param(
    [switch]$Deploy,
    [string]$Dest = "$env:USERPROFILE\moterm",
    [switch]$DebugBuild,
    [switch]$Run
)

$ErrorActionPreference = 'Stop'

# Move to the repository root (one level above this script).
$RepoRoot = Split-Path -Parent $PSScriptRoot
Set-Location $RepoRoot
Write-Host "== moterm Windows build ==" -ForegroundColor Cyan
Write-Host "repo: $RepoRoot"

# --- Prerequisite checks --------------------------------------------
function Test-Tool {
    param([string]$Name, [string]$Hint)
    if (-not (Get-Command $Name -ErrorAction SilentlyContinue)) {
        Write-Error ("{0} not found. {1}" -f $Name, $Hint)
    }
}
Test-Tool -Name 'cargo' -Hint 'Install Rust (stable, MSVC) from https://rustup.rs'

# Confirm the MSVC toolchain (vendored Lua needs the MSVC C compiler).
$hostLine = (rustc -vV | Select-String '^host:')
if ($hostLine) {
    $hostTriple = ($hostLine.ToString() -split '\s+')[1]
    if ($hostTriple -notlike '*windows-msvc*') {
        Write-Warning ("Default toolchain is not MSVC ({0})." -f $hostTriple)
        Write-Warning '  Recommended: rustup default stable-x86_64-pc-windows-msvc'
    }
}

# Check for the C compiler (cl.exe). If missing, point at VS Build Tools.
if (-not (Get-Command cl.exe -ErrorAction SilentlyContinue)) {
    Write-Warning 'cl.exe not on PATH. Building vendored Lua needs MSVC C++.'
    Write-Warning '  Install "Visual Studio Build Tools" -> "Desktop development with C++",'
    Write-Warning '  then run this script from "x64 Native Tools Command Prompt for VS".'
}

# --- Build ----------------------------------------------------------
$BuildProfile = if ($DebugBuild) { 'debug' } else { 'release' }
$cargoArgs = @('build', '-p', 'mot-gui')
if (-not $DebugBuild) { $cargoArgs += '--release' }

Write-Host ("cargo {0}" -f ($cargoArgs -join ' ')) -ForegroundColor Yellow
$env:CARGO_TERM_COLOR = 'always'
& cargo @cargoArgs
if ($LASTEXITCODE -ne 0) { Write-Error ("cargo build failed (exit {0})." -f $LASTEXITCODE) }

$Exe = Join-Path $RepoRoot ("target\{0}\moterm.exe" -f $BuildProfile)
if (-not (Test-Path $Exe)) { Write-Error ("Build artifact not found: {0}" -f $Exe) }
Write-Host ("built: {0}" -f $Exe) -ForegroundColor Green

# --- Deploy (optional) ----------------------------------------------
if ($Deploy) {
    New-Item -ItemType Directory -Force -Path $Dest | Out-Null
    Copy-Item $Exe -Destination $Dest -Force
    # If no config exists yet, drop a sample. The moterm.lua next to the exe
    # is loaded first (portable mode).
    $cfgDest = Join-Path $Dest 'moterm.lua'
    if (-not (Test-Path $cfgDest)) {
        $sample = Join-Path $RepoRoot 'examples\moterm.demo.lua'
        if (Test-Path $sample) { Copy-Item $sample -Destination $cfgDest }
    }
    Write-Host ("deployed to: {0}" -f $Dest) -ForegroundColor Green
}

# --- Run (optional) -------------------------------------------------
if ($Run) {
    Write-Host 'launching moterm...' -ForegroundColor Cyan
    & $Exe
}
