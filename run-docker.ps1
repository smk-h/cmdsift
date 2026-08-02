# * =====================================================
# * Copyright (c) hk. 2022-2025. All rights reserved.
# * File name  : run-docker.ps1
# * Author     : sumu
# * Description: Launch a dev container for the current directory.
# *   1. Ensure Docker Desktop is running (start + wait if not)
# *   2. Bind-mount the current directory into /workspace
# *   3. Run a given command, or drop into an interactive shell
# * ======================================================

[CmdletBinding()]
param(
    # Container image. Defaults to the project's Rust dev env image.
    [string]$Image = "docker.cnb.cool/sumu.h/rust-dev-env/rust-1.96.x",

    # Host-side mount source. Defaults to the directory the script is run from.
    [string]$Path,

    # Max seconds to wait for the Docker daemon to become ready after launch.
    [int]$StartupTimeout = 120,

    [switch]$Help,

    # Command to run inside the container, given as ONE quoted string, e.g.
    #   -Command "cargo build --release"
    # Passing it as a single string is deliberate: PowerShell parses each
    # unquoted/dashed token (like `--release`) as a script parameter, which
    # would corrupt the command. If omitted, an interactive bash shell opens.
    [string]$Command
)

# Split the command string into argv for `docker run`. Using a single string
# avoids PowerShell re-parsing dashed tokens; here we do our own quote-aware
# split so quoted substrings (e.g. sh -c 'echo hi; ls') stay together.
function Split-CommandArgv {
    param([string]$Text)
    $tokens = New-Object System.Collections.Generic.List[string]
    $cur = [System.Text.StringBuilder]::new()
    $inSingle = $false
    $inDouble = $false
    $hasTok   = $false

    foreach ($ch in $Text.ToCharArray()) {
        if ($inSingle) {
            if ($ch -eq "'") { $inSingle = $false } else { [void]$cur.Append($ch) }
            continue
        }
        if ($inDouble) {
            if ($ch -eq '"') { $inDouble = $false } else { [void]$cur.Append($ch) }
            continue
        }
        switch ($ch) {
            "'" { $inSingle = $true; $hasTok = $true }
            '"' { $inDouble = $true; $hasTok = $true }
            default {
                if ($ch -eq ' ' -or $ch -eq "`t") {
                    if ($hasTok) { $tokens.Add($cur.ToString()); [void]$cur.Clear(); $hasTok = $false }
                } else {
                    [void]$cur.Append($ch); $hasTok = $true
                }
            }
        }
    }
    if ($hasTok) { $tokens.Add($cur.ToString()) }
    return $tokens.ToArray()
}

$CommandArgv = @()
if ($Command) {
    $CommandArgv = Split-CommandArgv -Text $Command
}

# ========================================================
# Logging helpers
# ========================================================
function Step([string]$msg) { Write-Host ">>>  $msg" -ForegroundColor Cyan }
function Warn([string]$msg) { Write-Host "[WARN] $msg" -ForegroundColor Yellow }
function Err([string]$msg)  { Write-Host "[ERR]  $msg" -ForegroundColor Red }
function Ok([string]$msg)   { Write-Host "[OK]   $msg" -ForegroundColor Green }
function Info([string]$msg) { Write-Host "[INFO] $msg" -ForegroundColor Green }

# ========================================================
# Constants
# ========================================================
# Docker Desktop main GUI process name (matches what Task Manager shows).
$DD_PROCESS_NAME = "Docker Desktop"
# Default install path of Docker Desktop on Windows.
$DD_EXE = Join-Path $env:ProgramFiles "Docker\Docker\Docker Desktop.exe"
# Mount target inside the container.
$CONTAINER_MOUNT = "/workspace"

# ========================================================
# Step 1: Ensure Docker Desktop is running
# ========================================================
function Test-DockerDesktopRunning {
    # The GUI process is the reliable liveness signal for the desktop app
    # itself. The daemon (see next step) is the separate thing we then wait on.
    return $null -ne (Get-Process -Name $DD_PROCESS_NAME -ErrorAction SilentlyContinue)
}

function Start-DockerDesktop {
    Step "starting Docker Desktop..."

    if (-not (Test-Path $DD_EXE)) {
        Err "Docker Desktop not found at: $DD_EXE"
        Err "Please install Docker Desktop or pass -DD_EXE."
        return $false
    }

    # Start the app without bringing its window to the foreground.
    try {
        Start-Process -FilePath $DD_EXE -ErrorAction Stop | Out-Null
    } catch {
        Err "failed to launch Docker Desktop: $_"
        return $false
    }
    return $true
}

function Wait-DockerDaemon {
    param([int]$TimeoutSec)
    Step "waiting for Docker daemon to be ready (timeout ${TimeoutSec}s)..."

    # The GUI process may be up while the engine is still booting, so poll the
    # engine itself via `docker info`. Using --format keeps the output minimal
    # and avoids dumping the full info block on every retry.
    $deadline = (Get-Date).AddSeconds($TimeoutSec)
    do {
        $null = docker info --format '{{.ServerVersion}}' 2>$null
        if ($LASTEXITCODE -eq 0) {
            Ok "Docker daemon is ready."
            return $true
        }
        Start-Sleep -Seconds 2
    } while ((Get-Date) -lt $deadline)

    Err "Docker daemon did not become ready within ${TimeoutSec}s."
    return $false
}

function Ensure-Docker {
    if (Test-DockerDesktopRunning) {
        Info "Docker Desktop is already running."
        # GUI up does not guarantee engine up (e.g. right after a reboot).
        # Validate the engine too; if it's not ready, fall through to waiting.
        $null = docker info --format '{{.ServerVersion}}' 2>$null
        if ($LASTEXITCODE -eq 0) { return $true }
        Warn "Docker Desktop GUI is up but the daemon is not ready yet."
        return (Wait-DockerDaemon -TimeoutSec $StartupTimeout)
    }

    if (-not (Start-DockerDesktop)) { return $false }
    # Give the GUI process a moment to register before we start polling the
    # engine; otherwise the very first Get-Process may still miss it.
    Start-Sleep -Seconds 2
    return (Wait-DockerDaemon -TimeoutSec $StartupTimeout)
}

# ========================================================
# Step 2/3: Resolve mount source and run the container
# ========================================================
function Resolve-MountSource {
    if ($Path) {
        # Allow callers in either a Windows (E:\dir) or MSYS/Git-Bash (/e/dir)
        # shell to pass -Path. Normalize to a Windows absolute path, which is
        # what `docker run -v` expects on a Windows host.
        return (Convert-ToWindowsPath $Path)
    }
    # No explicit -Path: mount the current working directory of the caller.
    return (Get-Location).Path
}

# Convert a path that may be in POSIX/MSYS form (/e/AI/cmdsift) into a
# Windows absolute path (E:\AI\cmdsift). Native Windows paths pass through.
function Convert-ToWindowsPath {
    param([string]$P)
    if ($P -match '^/(?<drive>[a-zA-Z])/(.*)$') {
        $drive = $matches['drive'].ToUpper()
        $rest  = $matches[2] -replace '/', '\'
        return "${drive}:\${rest}"
    }
    # Already a Windows path (possibly with forward slashes); normalize slashes.
    return ($P -replace '/', '\')
}

function Invoke-Container {
    param(
        [string]$MountSource,
        [string[]]$CommandArgv
    )

    if (-not (Test-Path -LiteralPath $MountSource)) {
        Err "mount source does not exist: $MountSource"
        return 1
    }

    # Docker Desktop requires the host path to exist and, on the Hyper-V
    # backend, to be under a shared drive. The WSL2 backend shares all drives
    # by default, so this is usually a no-op concern.
    $mount = "${MountSource}:${CONTAINER_MOUNT}"

    $argv = @('docker', 'run', '--rm')

    if ($CommandArgv -and $CommandArgv.Count -gt 0) {
        # One-shot command: stay non-interactive (no -it). This keeps the call
        # usable from non-TTY parents (mintty/Git-Bash, CI shells) and avoids
        # "the input device is not a TTY" errors. Commands rarely need stdin.
        $argv += '-v', $mount, '-w', $CONTAINER_MOUNT, $Image
        $argv += $CommandArgv
        $mode = "run command: $($CommandArgv -join ' ')"
    } else {
        # Interactive shell: needs a real TTY, so -it is mandatory here. Run
        # this form from a PowerShell/Windows console, not from mintty/Git-Bash.
        $argv += '-it', '-v', $mount, '-w', $CONTAINER_MOUNT, $Image, 'bash'
        $mode = "interactive shell (bash)"
    }

    Step "launching container"
    Write-Host "  image  : $Image"
    Write-Host "  mount  : $mount"
    Write-Host "  workdir: $CONTAINER_MOUNT"
    Write-Host "  mode   : $mode"
    Write-Host ("[CMD]  " + ($argv -join ' ')) -ForegroundColor Magenta

    # Invoke docker directly, inheriting the current terminal's stdio. That's
    # all there is to it: docker handles its own IO, and the host shell
    # handles any TTY concerns. In Git-Bash, prefix the call with `winpty`:
    #   winpty powershell.exe -File .\run-docker.ps1
    #
    # Do NOT `return $LASTEXITCODE` from this function: a return value mixes
    # into the same output stream as docker's stdout and would show up as a
    # stray number on the terminal (and, if the caller captured the call,
    # would hang interactive mode -- see Main). The caller reads
    # $LASTEXITCODE directly instead.
    & docker @($argv | Select-Object -Skip 1)
}

# ========================================================
# Help
# ========================================================
function Show-Help {
    $txt = @'
=================================================
        run-docker.ps1  (dev container launcher)
=================================================
Starts (or reuses) Docker Desktop, bind-mounts the
current directory into /workspace, then either runs a
command or opens an interactive shell in the container.

Usage:
  .\run-docker.ps1 [options]
  .\run-docker.ps1 [options] -Command "<cmd>"

Options:
  -Image <name>       Container image
                      (default: docker.cnb.cool/sumu.h/rust-dev-env/rust-1.96.x)
  -Path  <host_dir>   Host directory to mount (default: current dir)
  -StartupTimeout <s> Seconds to wait for the daemon (default: 120)
  -Command "<cmd>"    Command to run in the container, as ONE quoted
                      string. If omitted, an interactive bash shell opens.
  -Help               Show this help

Why -Command is a single string:
  PowerShell parses each dashed token (e.g. --release, -j8) as a script
  parameter, which would corrupt an unquoted command. Wrap the whole
  command in quotes so it is forwarded to the container verbatim.

Git-Bash interactive shell:
  Interactive mode (`-it`) needs a real TTY. From Git-Bash, prefix with
  winpty so docker gets a controlling terminal:
    winpty powershell.exe -File .\run-docker.ps1

Examples:
  .\run-docker.ps1                                    # interactive bash shell
  .\run-docker.ps1 -Command "cargo build --release"   # build (Linux) in container
  .\run-docker.ps1 -Command "cargo test"              # run tests in container
  .\run-docker.ps1 -Command "make -j8"                # run make
  .\run-docker.ps1 -Path D:\proj                      # mount a different dir, then shell
  .\run-docker.ps1 -Path D:\proj -Command "cargo run" # mount + run command
=================================================
'@
    Write-Host $txt
}

# ========================================================
# Main
# ========================================================
if ($Help) { Show-Help; exit 0 }

if (-not (Ensure-Docker)) { exit 1 }

$mountSource = Resolve-MountSource
# NOTE: do NOT write `Invoke-Container` as `$code = Invoke-Container ...`.
# Capturing the call makes PowerShell collect the native command's stdout
# into the variable as object streams, which buffers the stream and hangs
# an interactive container (bash never EOFs, so the assignment never
# completes and no output reaches the terminal). Calling it bare lets the
# native docker process inherit the terminal's stdio directly.
Invoke-Container -MountSource $mountSource -CommandArgv $CommandArgv
exit $LASTEXITCODE
