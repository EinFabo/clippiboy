# Builds the plugin and puts it into the Stream Deck software.
#
#   powershell -ExecutionPolicy Bypass -File streamdeck\install.ps1
#
# The route past the .streamDeckPlugin file: that one has to be double-clicked
# and confirmed, which is right for somebody installing it once and wrong for
# somebody changing a line and wanting to see it. This copies the folder straight
# into the Stream Deck software's plugin directory instead.
#
# The Stream Deck software is closed and started again on the way - it reads the
# plugin directory once at startup and holds the running plugin open, so there is
# no way around it. Nothing is lost by it; the keys come back as they were.

$ErrorActionPreference = "Stop"

$here = Split-Path -Parent $MyInvocation.MyCommand.Path
$uuid = "com.einfabo.clippiboy"
$source = Join-Path $here "$uuid.sdPlugin"
$target = Join-Path $env:APPDATA "Elgato\StreamDeck\Plugins\$uuid.sdPlugin"
$streamDeck = "C:\Program Files\Elgato\StreamDeck\StreamDeck.exe"

if (-not (Test-Path $streamDeck)) {
    throw "The Stream Deck software is not in $streamDeck - install it first."
}

Push-Location $here
try {
    # The plugin needs the SDK inside the .sdPlugin folder, because that folder is
    # what the Stream Deck software runs. See README.md.
    if (-not (Test-Path (Join-Path $here "node_modules"))) {
        Write-Host "Fetching the build tools..."
        npm install --silent
    }
    if (-not (Test-Path (Join-Path $source "node_modules"))) {
        Write-Host "Fetching what the plugin needs at runtime..."
        npm install --silent --prefix "$uuid.sdPlugin"
    }

    Write-Host "Compiling..."
    npm run build --silent
} finally {
    Pop-Location
}

$wasRunning = $null -ne (Get-Process StreamDeck -ErrorAction SilentlyContinue)
if ($wasRunning) {
    Write-Host "Closing the Stream Deck software..."
    Stop-Process -Name StreamDeck -Force
    # It lets go of the plugin's files a moment after the window is gone; copying
    # into them too early fails with "file in use".
    Start-Sleep -Seconds 3
}

Write-Host "Installing into $target"
if (Test-Path $target) {
    Remove-Item $target -Recurse -Force
}
# `node_modules` travels along on purpose - without it the plugin does not start.
Copy-Item $source $target -Recurse -Force

if ($wasRunning) {
    Write-Host "Starting the Stream Deck software again..."
    Start-Process $streamDeck
}

Write-Host ""
Write-Host "Done. The three keys are under 'ClippiBoy' in the action list."
Write-Host "They show a dash until ClippiBoy itself is running with the control"
Write-Host "port switched on (Settings -> Stream Deck)."
