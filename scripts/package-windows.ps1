param(
    [string]$OutputDirectory = (Join-Path $PSScriptRoot '../dist'),
    [switch]$AppOnly
)

$ErrorActionPreference = 'Stop'
$root = (Resolve-Path -LiteralPath (Join-Path $PSScriptRoot '..')).Path
$licenses = Join-Path $OutputDirectory 'licenses'

New-Item -ItemType Directory -Force -Path $licenses | Out-Null
Copy-Item -LiteralPath (Join-Path $root 'target/release/device-hub.exe') -Destination $OutputDirectory
$documents = @(
    (Join-Path $root 'docs/QUICKSTART_JA.md'),
    (Join-Path $root 'LICENSE-MIT'),
    (Join-Path $root 'LICENSE-APACHE'),
    (Join-Path $root 'THIRD_PARTY_NOTICES.md')
)
Copy-Item -LiteralPath $documents -Destination $OutputDirectory

Copy-Item -Path (Join-Path $root 'licenses/*') -Destination $licenses
foreach ($name in 'gpui-pre', 'gpui-pre-windows') {
    Copy-Item -LiteralPath (Join-Path $root "vendor/$name/LICENSE-APACHE") `
        -Destination (Join-Path $licenses "$name-LICENSE-APACHE")
}

if ($AppOnly) { return }

$plugin = Join-Path $OutputDirectory 'devhub_obs'
$bin = Join-Path $plugin 'bin/64bit'
$locale = Join-Path $plugin 'data/locale'
$pluginLicenses = Join-Path $plugin 'licenses'
New-Item -ItemType Directory -Force -Path $bin, $locale, $pluginLicenses | Out-Null
Copy-Item -LiteralPath (Join-Path $root 'target/release/devhub_obs.dll') -Destination $bin
Copy-Item -Path (Join-Path $root 'crates/devhub-obs/data/locale/*.ini') -Destination $locale
Copy-Item -LiteralPath $documents -Destination $plugin
Copy-Item -Path (Join-Path $licenses '*') -Destination $pluginLicenses
