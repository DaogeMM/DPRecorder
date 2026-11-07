# Rebuild the web build of the engine (the wasm player used by the editor preview) and lay out
# the assets it fetches at runtime.
#
# Output: public/player/wasm/{dps_player.js,dps_player_bg.wasm} and public/player/assets/ (a copy
# of assets/). Both are build artifacts and are gitignored, so a fresh clone has to run this once.
#
# This is `cargo rustc` rather than a plain bin build because:
#   - the engine's default features include `video` (ffmpeg static lib), which has no wasm build,
#     hence --no-default-features --features play;
#   - miniquad's wasm backend imports host functions with no Rust declarations, so linking needs
#     --allow-undefined.
# tools/patch_bindgen.mjs is required afterwards: wasm-bindgen turns those imports into
# `import * as importN from "env"`, and no browser has an "env" module (see the script header).
#
# Usage (from the repository root):
#   powershell -NoProfile -ExecutionPolicy Bypass -File DPR\public\player\build.ps1

$ErrorActionPreference = "Stop"

# ...\repository\DPR\public\player -> ...\repository
$repo = (Resolve-Path (Join-Path $PSScriptRoot "..\..\..")).Path
$player = $PSScriptRoot
$target = $env:CARGO_TARGET_DIR
if (-not $target) { $target = Join-Path (Split-Path $repo) "target" }
$wasmTarget = Join-Path $target "wasm32-unknown-unknown\release"

# The sandbox denies %TEMP%; cargo/cl/lib fail there with misleading errors.
$env:TEMP = Join-Path (Split-Path $repo) ".tmp\cargo-temp"
$env:TMP = $env:TEMP
New-Item -ItemType Directory -Force $env:TEMP | Out-Null

$env:RUSTFLAGS = '--cfg getrandom_backend="wasm_js"'

Push-Location $repo
try {
    cargo rustc -p dps --bin wasm_player --release --target wasm32-unknown-unknown `
        --no-default-features --features play -- -C link-arg=--allow-undefined
    if ($LASTEXITCODE -ne 0) { throw "cargo rustc failed" }

    & "$env:USERPROFILE\.cargo\bin\wasm-bindgen.exe" (Join-Path $wasmTarget "wasm_player.wasm") `
        --out-dir (Join-Path $player "wasm") --out-name dps_player --target web --no-typescript
    if ($LASTEXITCODE -ne 0) { throw "wasm-bindgen failed" }

    node (Join-Path $player "tools\patch_bindgen.mjs") (Join-Path $player "wasm\dps_player.js") dps_player
    if ($LASTEXITCODE -ne 0) { throw "patch_bindgen failed" }

    # The player page loads the font, resource pack skins and sounds via `assets/...`, so keep a
    # mirror of the repository's assets next to it.
    robocopy (Join-Path $repo "assets") (Join-Path $player "assets") /MIR /NFL /NDL /NJH /NJS /NC /NS | Out-Null
} finally {
    Pop-Location
}

Write-Host "web engine rebuilt: $player"
