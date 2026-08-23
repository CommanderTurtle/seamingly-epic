[CmdletBinding()]
param()

$ErrorActionPreference = 'Stop'
$ProjectRoot = Split-Path -Parent $MyInvocation.MyCommand.Path
$Cargo = Get-Command cargo -ErrorAction SilentlyContinue
if (-not $Cargo) {
    throw 'Rust/Cargo is required. Install Rust from https://rustup.rs and run this script again.'
}

Push-Location $ProjectRoot
try {
    & $Cargo.Source build --release --locked
    if ($LASTEXITCODE -ne 0) {
        throw "cargo build failed with exit code $LASTEXITCODE"
    }

    $Source = Join-Path $ProjectRoot 'target\release\seamingly-epic.exe'
    if (-not (Test-Path -LiteralPath $Source -PathType Leaf)) {
        throw "Native binary was not produced at $Source"
    }
    $BinDirectory = Join-Path $ProjectRoot 'bin'
    New-Item -ItemType Directory -Path $BinDirectory -Force | Out-Null
    Copy-Item -LiteralPath $Source -Destination (Join-Path $BinDirectory 'seamingly-epic.exe') -Force
    Write-Host 'Seamingly Epic is ready. Restart ComfyUI to load the custom nodes.' -ForegroundColor Green
} finally {
    Pop-Location
}
