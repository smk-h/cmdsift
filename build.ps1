# * =====================================================
# * Copyright (c) hk. 2022-2025. All rights reserved.
# * File name  : build.ps1
# * Author     : sumu
# * Description: cmdsift project build script (PowerShell)
# *   - Build / Run / Clean / Test  (local Windows toolchain)
# *   - Build Linux release binary  (via Docker image)
# * ======================================================

[CmdletBinding()]
param(
    [switch]$Build,
    [switch]$Run,
    [switch]$Clean,
    [switch]$Test,
    # Target platform: 'win' (default) builds with the local toolchain;
    # 'linux' builds inside Docker. Accepts the alias -p.
    [Alias('p')]
    [ValidateSet('win', 'linux')]
    [string]$Platform = 'win',
    [switch]$Help
)

# Normalize for safe comparisons.
$Platform = $Platform.ToLower()

# Whether -p was explicitly passed (used by Show-Menu to mark the default).
# Captured here (script scope) because $PSBoundParameters inside Show-Menu
# would refer to Show-Menu's own parameters, not the script's.
$PlatformExplicit = $PSBoundParameters.ContainsKey('Platform')

# ========================================================
# Constants
# ========================================================
# Project root = directory that contains this script.
$PROJECT_ROOT = $PSScriptRoot
$BINARY_NAME  = "cmdsift"
$TARGET_DIR   = Join-Path $PROJECT_ROOT "target"

# Local Windows artifacts live under target\win, mirroring the Linux build's
# target\linux. Routing each platform through its own CARGO_TARGET_DIR means
# switching between Windows and Linux builds never triggers a full rebuild.
# Set once for the whole script; the Linux action overrides it inside the
# container with its own -e value, so host and container never collide.
$WIN_TARGET   = Join-Path $TARGET_DIR "win"
$RELEASE_BIN  = Join-Path $WIN_TARGET "release\$BINARY_NAME.exe"
$env:CARGO_TARGET_DIR = $WIN_TARGET

# Docker image used to cross-compile the Linux release binary.
$DOCKER_IMAGE = "docker.cnb.cool/sumu.h/rust-dev-env/rust-1.96.x"
# Container-relative target dir; /workspace is the bind-mount of PROJECT_ROOT.
$LINUX_TARGET = "/workspace/target/linux"
$LINUX_BIN    = Join-Path $TARGET_DIR "linux\release\$BINARY_NAME"

# ========================================================
# Logging helpers (colored, no emoji -> ASCII safe for PS 5.1)
# ========================================================
function Step([string]$msg) { Write-Host ">>>  $msg" -ForegroundColor Cyan }
function Warn([string]$msg) { Write-Host "[WARN] $msg" -ForegroundColor Yellow }
function Err([string]$msg)  { Write-Host "[ERR]  $msg" -ForegroundColor Red }
function Ok([string]$msg)   { Write-Host "[OK]   $msg" -ForegroundColor Green }
function Info([string]$msg) { Write-Host "[INFO] $msg" -ForegroundColor Green }

# Run a native command with echo + exit-code check.
# All argv come in as one array so leading-dash tokens (e.g. --release, -v)
# never confuse PowerShell parameter binding.
# Returns $true on success (exit code 0), $false otherwise.
function Invoke-Exec {
    param(
        [Parameter(Mandatory = $true)][string[]]$Argv
    )
    if ($Argv.Count -lt 1) { return $false }

    $exe  = $Argv[0]
    if ($Argv.Count -gt 1) {
        $rest = $Argv[1..($Argv.Count - 1)]
    } else {
        $rest = @()
    }

    $display = ($Argv -join ' ')
    Write-Host "[CMD]  $display" -ForegroundColor Magenta

    # PowerShell 5.1 wraps any line a native command writes to stderr into an
    # ErrorRecord (NativeCommandError) and renders it as a red error block,
    # which is misleading since tools like cargo/rustc routinely stream build
    # progress to stderr. Merging with 2>&1 and down-grading those records to
    # plain strings keeps the output readable. The pipe uses cmdlets only
    # (ForEach-Object / Out-Host), so $LASTEXITCODE still reflects $exe.
    & $exe @rest 2>&1 | ForEach-Object {
        if ($_ -is [System.Management.Automation.ErrorRecord]) {
            $_.Exception.Message
        } else {
            $_
        }
    } | Out-Host
    $code = $LASTEXITCODE

    if ($null -ne $code -and $code -ne 0) {
        Err "Command failed (exit code: $code): $display"
        return $false
    }
    return $true
}

# Directory push/pop helpers (mirrors cdi/cdo in build.sh).
function cdi([string]$path) { Push-Location -LiteralPath $path }
function cdo { Pop-Location }

# ========================================================
# Environment checks
# ========================================================
function Test-Cargo {
    if (-not (Get-Command cargo -ErrorAction SilentlyContinue)) {
        Err "cargo not found! Please install the Rust toolchain first."
        Err "  install via: https://www.rust-lang.org/tools/install"
        return $false
    }
    return $true
}

function Test-Docker {
    if (-not (Get-Command docker -ErrorAction SilentlyContinue)) {
        Err "docker not found! Please install Docker Desktop first."
        return $false
    }
    return $true
}

# ========================================================
# Build (local Windows toolchain, release mode)
# ========================================================
function Invoke-BuildWindows {
    Step "building project (windows release)..."

    if (-not (Test-Cargo)) { return $false }

    cdi $PROJECT_ROOT
    try {
        $ok = Invoke-Exec @('cargo', 'build', '--release')
    } finally { cdo }

    if ($ok) {
        if (Test-Path $RELEASE_BIN) {
            Ok "build success: $RELEASE_BIN"
        } else {
            Ok "build success."
        }
    } else {
        Err "build failed!"
    }
    return $ok
}

# ========================================================
# Run the release binary (build first if missing)
# ========================================================
function Invoke-RunAction {
    Step "running project..."

    # -Run executes the Windows binary directly on the host; a Linux ELF
    # cannot be run natively on Windows. Block the combination explicitly so
    # the user gets a clear message instead of a cryptic failure.
    if ($Platform -eq 'linux') {
        Err "-Run is Windows-only; it cannot execute the Linux binary on Windows."
        Err "  Use: .\build.ps1 -b -p linux   then run the binary inside a container."
        return $false
    }

    if (-not (Test-Cargo)) { return $false }

    cdi $PROJECT_ROOT
    try {
        if (-not (Test-Path $RELEASE_BIN)) {
            Warn "release binary not found, building first..."
            if (-not (Invoke-Exec @('cargo', 'build', '--release'))) {
                Err "build failed, cannot run."
                return $false
            }
        }

        # The program forwards the underlying build command's exit code, so a
        # non-zero exit is a normal result here (only warned, not failed).
        & $RELEASE_BIN 2>&1 | Out-Host
        $code = $LASTEXITCODE

        if ($null -eq $code -or $code -eq 0) {
            Ok "run finished."
        } else {
            Warn "program exited with code: $code"
        }
        return $true
    } finally { cdo }
}

# ========================================================
# Clean build artifacts (both Windows and Linux)
# ========================================================
function Invoke-CleanAction {
    Step "cleaning build artifacts..."

    if (-not (Test-Cargo)) { return $false }

    cdi $PROJECT_ROOT
    try {
        # cargo clean honors CARGO_TARGET_DIR, so on its own it would only
        # remove target\win and leave target\linux behind. Since this action
        # means "clean everything", drop the whole target/ tree (matching
        # build.sh's `cargo clean` from a context where CARGO_TARGET_DIR is
        # unset). A missing directory is not an error.
        if (Test-Path -LiteralPath $TARGET_DIR) {
            Write-Host "[CMD]  Remove-Item -Recurse -Force $TARGET_DIR" -ForegroundColor Magenta
            try {
                Remove-Item -LiteralPath $TARGET_DIR -Recurse -Force -ErrorAction Stop
                $ok = $true
            } catch {
                Err "clean failed: $_"
                $ok = $false
            }
        } else {
            $ok = $true
        }
    } finally { cdo }

    if ($ok) {
        Ok "clean done. ($TARGET_DIR removed)"
    } else {
        Err "clean failed!"
    }
    return $ok
}

# ========================================================
# Run unit tests (cargo test)
# ========================================================
function Invoke-TestAction {
    Step "running tests..."

    if (-not (Test-Cargo)) { return $false }

    cdi $PROJECT_ROOT
    try {
        $ok = Invoke-Exec @('cargo', 'test')
    } finally { cdo }

    if ($ok) {
        Ok "all tests passed."
    } else {
        Err "tests failed!"
    }
    return $ok
}

# ========================================================
# Build the Linux release binary inside Docker
# ========================================================
function Invoke-BuildLinux {
    Step "building project (linux release via docker)..."

    if (-not (Test-Docker)) { return $false }

    # Bind-mount the project root into the container so the artifacts land
    # back on the host filesystem. On Docker Desktop (Windows) make sure the
    # drive that hosts $PROJECT_ROOT is shared (Settings > Resources > File
    # Sharing). WSL2 backend shares all drives by default.
    $mount = "${PROJECT_ROOT}:/workspace"

    $ok = Invoke-Exec @(
        'docker', 'run', '--rm',
        '-v', $mount,
        '-w', '/workspace',
        '-e', "CARGO_TARGET_DIR=$LINUX_TARGET",
        $DOCKER_IMAGE,
        'cargo', 'build', '--release'
    )

    if ($ok) {
        if (Test-Path $LINUX_BIN) {
            Ok "linux build success: $LINUX_BIN"
        } else {
            Ok "linux build success."
        }
    } else {
        Err "linux build failed!"
    }
    return $ok
}

# ========================================================
# Build dispatcher: routes to the platform-specific builder
# ========================================================
function Invoke-Build {
    if ($Platform -eq 'linux') { return (Invoke-BuildLinux) }
    return (Invoke-BuildWindows)
}

# ========================================================
# Help
# ========================================================
function Show-Help {
    $txt = @'
=================================================
           cmdsift build script (PowerShell)
=================================================
Usage: .\build.ps1 [options]

Options:
  -Build   (-b)        Build project (release). Platform set by -p.
  -Run    (-r)        Run project (build first if needed; Windows only)
  -Clean   (-c)        Clean build artifacts (removes target\)
  -Test    (-t)        Run unit tests (cargo test)
  -Platform (-p) <val> Target platform: win (default) | linux
  -Help    (-h)        Show this help message

Notes:
  - Switches can be combined, e.g. .\build.ps1 -b -r
  - PowerShell matches unique name prefixes, so -b/-r/-c/-t/-p/-h work
  - Clean runs first; if only -c is given the script exits after cleaning
  - Build artifacts are kept under per-platform subdirs of target\:
      Windows -> target\win\release\cmdsift.exe
      Linux   -> target\linux\release\cmdsift   (-p linux)
    This avoids cross-platform rebuild churn. -Clean removes target\ entirely.
  - -p linux uses image: docker.cnb.cool/sumu.h/rust-dev-env/rust-1.96.x

Examples:
  .\build.ps1 -b                # Build (Windows)
  .\build.ps1 -b -p linux       # Build (Linux via Docker)
  .\build.ps1 -b -r             # Build and run (Windows)
  .\build.ps1 -t                # Run tests
  .\build.ps1 -c                # Clean
=================================================
'@
    Write-Host $txt
}

# ========================================================
# Menu banner (mirrors do_echo_menu in build.sh)
# ========================================================
function Show-Menu([string]$Params) {
    Write-Host "=================================================" -ForegroundColor White
    Write-Host "           cmdsift build script (PowerShell)"
    Write-Host "================================================="
    Write-Host "PROJECT_ROOT : $PROJECT_ROOT"
    Write-Host "BINARY_NAME  : $BINARY_NAME"
    Write-Host "TARGET_DIR   : $TARGET_DIR"
    Write-Host "PLATFORM     : $Platform$(if (-not $PlatformExplicit) { ' (default)' })"
    Write-Host "DOCKER_IMAGE : $DOCKER_IMAGE"
    Write-Host "PARAMS       : ($Params)"
    Write-Host "================================================="
}

# ========================================================
# Main
# ========================================================
$params = if ($PSBoundParameters.Count -gt 0) {
    ($PSBoundParameters.Keys | Sort-Object) -join ', '
} else {
    'none'
}
Show-Menu -Params $params

if ($Help) {
    Show-Help
    exit 0
}

# No action specified -> show help.
$anyAction = $Build -or $Run -or $Clean -or $Test
if (-not $anyAction) {
    Show-Help
    exit 0
}

# Clean runs first; if nothing else was requested, stop after cleaning.
if ($Clean) {
    if (-not (Invoke-CleanAction)) { exit 1 }
    if (-not ($Build -or $Run -or $Test)) { exit 0 }
}

if ($Build) {
    if (-not (Invoke-Build)) { exit 1 }
}

if ($Test) {
    if (-not (Invoke-TestAction)) { exit 1 }
}

if ($Run) {
    if (-not (Invoke-RunAction)) { exit 1 }
}

exit 0
