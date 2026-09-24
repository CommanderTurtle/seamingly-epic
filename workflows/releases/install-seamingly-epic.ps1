# Total-noob seamingly-epic installer (native PowerShell, no git required)
# If a comfyshell node already exists anywhere under Documents (detected via
# its distinctive README.md SHA-256), the clone is skipped - and if the built
# exe is missing too, you are offered a build anyway. The final workflow
# pointers are always printed.

$ErrorActionPreference = "Stop"

# --- detect an existing comfyshell under Documents ----------------------------
# The node always lives at <root>\workflows\custom-node\comfyshell, so the
# cheapest possible detection is finding literal "comfyshell" folders anywhere
# under Documents and sha'ing their single README.md (distinctive SHA-256).
# A match means the node is already installed - nothing to clone.
$ComfyShellReadmeSha = "A436ED4C16E6B9EEDE6E85AFA65AE389CF637FB3D18B8EBA2C4438FC013BEF0F"
$documents = Join-Path $HOME "Documents"
$repoDir = $null
foreach ($node in Get-ChildItem -Path $documents -Directory -Filter "comfyshell" -Recurse -ErrorAction SilentlyContinue) {
    $readme = Join-Path $node.FullName "README.md"
    if (Test-Path -LiteralPath $readme) {
        if ((Get-FileHash -Algorithm SHA256 -LiteralPath $readme).Hash -eq $ComfyShellReadmeSha) {
            # <root>\workflows\custom-node\comfyshell -> three parents up
            $repoDir = Split-Path -Parent (Split-Path -Parent (Split-Path -Parent $node.FullName))
            Write-Host "comfyshell already detected at $($node.FullName)"
            Write-Host "Nothing to clone; skipping."
            break
        }
    }
}

$build = $false
if ($repoDir) {
    $releaseDir = Join-Path $repoDir "target\release"
    $exe = Join-Path $releaseDir "seamingly-epic.exe"
    if (-not (Test-Path -LiteralPath $releaseDir)) {
        Write-Host "No target\release under $repoDir; nothing was ever built - exiting."
    } elseif (Test-Path -LiteralPath $exe) {
        Write-Host "Built target found at $exe; skipping build."
    } else {
        Write-Host "target\release exists but $exe is missing"
        $answer = Read-Host "Build it anyways? [Y/n]"
        if ($answer -ne "n" -and $answer -ne "N") {
            $build = $true
        }
    }
} else {
    # --- clone ----------------------------------------------------------------
    $repoDir = Join-Path $documents "seemingly-epic"
    Write-Host "Cloning seamingly-epic to $repoDir ..."
    $git = Get-Command git -ErrorAction SilentlyContinue
    if ($git) {
        git clone --depth 1 "https://github.com/CommanderTurtle/seamingly-epic" $repoDir
    } else {
        $zip = Join-Path $env:TEMP "seamingly-epic.zip"
        Invoke-WebRequest -Uri "https://github.com/CommanderTurtle/seamingly-epic/archive/refs/heads/main.zip" -OutFile $zip
        $extractDir = Join-Path $env:TEMP "seamingly-epic-extract"
        Expand-Archive -Path $zip -DestinationPath $extractDir -Force
        Move-Item -Path ((Get-ChildItem -Path $extractDir -Directory | Select-Object -First 1).FullName) -Destination $repoDir
        Remove-Item -Recurse -Force $zip, $extractDir
    }
    $build = $true
}

# --- cargo build (with winget fallback) ----------------------------------------
if ($build) {
    if (-not (Get-Command cargo -ErrorAction SilentlyContinue)) {
        # Pick up cargo from a previous user-level install without a new shell
        $cargoBin = Join-Path $HOME ".cargo\bin"
        if (Get-Command (Join-Path $cargoBin "cargo.exe") -ErrorAction SilentlyContinue) {
            $env:Path = "$cargoBin;$env:Path"
        }
    }
    if (-not (Get-Command cargo -ErrorAction SilentlyContinue)) {
        Write-Host "Cargo is not installed, install with winget? NOTE: Update later with 'winget update --source winget --id Rustlang.Rustup' ?"
        $install = Read-Host "Install now? [Y/n]"
        if ($install -ne "n" -and $install -ne "N") {
            winget install --source winget --id Rustlang.Rustup
            $env:Path = "$HOME\.cargo\bin;$env:Path"
        } else {
            Write-Error "cargo is required to continue."
        }
    }

    Write-Host "Building seamingly-epic (this can take a while on first run)..."
    Push-Location $repoDir
    try {
        cargo build --release
    } finally {
        Pop-Location
    }
}

# --- final messages (always printed) -------------------------------------------
@"
Make sure to update the workflow manually (final node's config) with target:
"$repoDir\target\release\seamingly-epic.exe"
"@ | Write-Host

Start-Sleep -Seconds 2

@"
Note! Workflows available at:
"https://huggingface.co/sHEL1562/shelling/blob/main/src/1-5%20Latest%20Edit%20Pipeline.json"
"https://huggingface.co/sHEL1562/shelling/blob/main/src/1-5%20Latest%20Gen%20Pipeline.json"

and a slimmed down upscale-only variant is at:
"$repoDir\workflows\1-2 Upscale Pipeline.json"
"@ | Write-Host
