# Builds ClippiBoy Camera: the media source DLL first, then the setup helper
# that carries it. Run from the vcam folder. cargo reports progress on stderr,
# so success is read off the exit code. GitHub's pwsh starts with "Stop",
# which would end the script at cargo's first progress line.
$ErrorActionPreference = 'Continue'
cargo build --release -p clippiboy-vcam 2>&1 | Out-Null
if ($LASTEXITCODE -ne 0) { Write-Error 'building the DLL failed'; exit 1 }
$env:VCAM_DLL = (Resolve-Path 'target\release\clippiboy_vcam.dll').Path
cargo build --release -p clippiboy-vcam-setup 2>&1 | Out-Null
if ($LASTEXITCODE -ne 0) { Write-Error 'building the setup failed'; exit 1 }
Get-Item 'target\release\clippiboy-vcam-setup.exe' | Select-Object Name, Length
