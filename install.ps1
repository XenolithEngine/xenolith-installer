# Xenolith Installer CLI — one-line installer for Windows (PowerShell).
#
#   irm https://raw.githubusercontent.com/XenolithEngine/xenolith-installer/main/install.ps1 | iex
#
# Downloads the CLI from the latest GitHub release, verifies its SHA-256, and puts
# it on your PATH. Then:
#   xenolith-installer-cli install   # provision the SDK for this machine
#   xenolith-installer-cli new myapp
#   xenolith-installer-cli build myapp --run
#
# Override the install dir with  $env:XENOLITH_BIN = 'C:\some\dir'.
$ErrorActionPreference = 'Stop'

$Repo   = 'XenolithEngine/xenolith-installer'
$Bin    = 'xenolith-installer-cli'
$Triple = 'x86_64-pc-windows-msvc'
$Dest   = if ($env:XENOLITH_BIN) { $env:XENOLITH_BIN } else { "$env:LOCALAPPDATA\Xenolith\bin" }

if (-not (Get-Command tar -ErrorAction SilentlyContinue)) {
    throw "need 'tar' (ships with Windows 10 1803+). Update Windows, or download the .exe from the Releases page."
}

$url = "https://github.com/$Repo/releases/latest/download/$Bin-$Triple.tar.gz"
$tmp = Join-Path $env:TEMP ("xeno-" + [Guid]::NewGuid())
New-Item -ItemType Directory -Force -Path $tmp | Out-Null
try {
    Write-Host "Downloading $Bin ($Triple)..."
    Invoke-WebRequest -Uri $url          -OutFile "$tmp\cli.tar.gz"        -UseBasicParsing
    Invoke-WebRequest -Uri "$url.sha256" -OutFile "$tmp\cli.tar.gz.sha256" -UseBasicParsing

    # Verify integrity against the published checksum.
    $want = ((Get-Content "$tmp\cli.tar.gz.sha256" -Raw) -split '\s+')[0].Trim().ToLower()
    $got  = (Get-FileHash "$tmp\cli.tar.gz" -Algorithm SHA256).Hash.ToLower()
    if (-not $want) { throw "checksum file was empty" }
    if ($got -ne $want) {
        throw "checksum mismatch - download may be corrupt or tampered`n  expected $want`n  got      $got"
    }
    Write-Host "  checksum OK"

    tar -xzf "$tmp\cli.tar.gz" -C "$tmp"
    if (-not (Test-Path "$tmp\$Bin.exe")) { throw "archive did not contain $Bin.exe" }

    New-Item -ItemType Directory -Force -Path $Dest | Out-Null
    Move-Item -Force "$tmp\$Bin.exe" "$Dest\$Bin.exe"
}
finally {
    Remove-Item -Recurse -Force $tmp -ErrorAction SilentlyContinue
}

Write-Host ""
Write-Host "Installed $Bin to $Dest\$Bin.exe"

# Put it on the user PATH if it isn't already.
$userPath = [Environment]::GetEnvironmentVariable('Path', 'User')
if (($userPath -split ';') -notcontains $Dest) {
    [Environment]::SetEnvironmentVariable('Path', "$Dest;$userPath", 'User')
    Write-Host "Added $Dest to your PATH - restart the terminal for it to take effect."
}

Write-Host ""
Write-Host "Next steps:"
Write-Host "  $Bin install            # download the SDK for this machine"
Write-Host "  $Bin new myapp          # scaffold .\myapp"
Write-Host "  $Bin build myapp --run  # build and launch it"
