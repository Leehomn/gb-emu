$ErrorActionPreference = 'Stop'
$root = Split-Path -Parent $PSScriptRoot
Push-Location $root
try {
    cargo build --release --locked --target wasm32-unknown-unknown --example web_core @args
    if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }
    $wasm = Join-Path $root 'target/wasm32-unknown-unknown/release/examples/web_core.wasm'
    if (!(Test-Path -LiteralPath $wasm)) { throw '构建目录里找不到 web_core.wasm。' }
    Copy-Item -LiteralPath $wasm -Destination (Join-Path $root 'web/gba.wasm') -Force
}
finally {
    Pop-Location
}
