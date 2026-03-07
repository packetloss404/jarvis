param(
    [Parameter(Mandatory = $true)]
    [string]$Version,

    [string]$InstallerUrl = "https://github.com/dylan/jarvis/releases/download/v$Version/Jarvis.msi",

    [string]$MsiPath = "target/release/Jarvis.msi"
)

$ErrorActionPreference = 'Stop'

$manifestRoot = Join-Path 'packaging/windows/winget' $Version
New-Item -ItemType Directory -Force -Path $manifestRoot | Out-Null

$sha256 = (Get-FileHash $MsiPath -Algorithm SHA256).Hash

$versionManifest = @"
PackageIdentifier: DylanBurton.Jarvis
PackageVersion: $Version
DefaultLocale: en-US
ManifestType: version
ManifestVersion: 1.6.0
"@

$installerManifest = @"
PackageIdentifier: DylanBurton.Jarvis
PackageVersion: $Version
UpgradeBehavior: install
Installers:
  - Architecture: x64
    InstallerType: wix
    InstallerUrl: $InstallerUrl
    InstallerSha256: $sha256
    AppsAndFeaturesEntries:
      - DisplayName: Jarvis
        Publisher: Dylan Burton
ManifestType: installer
ManifestVersion: 1.6.0
"@

$localeManifest = @"
PackageIdentifier: DylanBurton.Jarvis
PackageVersion: $Version
PackageLocale: en-US
Publisher: Dylan Burton
PublisherUrl: https://github.com/dylan/jarvis
PublisherSupportUrl: https://github.com/dylan/jarvis/issues
Author: Dylan Burton
PackageName: Jarvis
PackageUrl: https://github.com/dylan/jarvis
License: MIT
ShortDescription: Jarvis desktop app
Description: Cross-platform Jarvis desktop app with local panels, AI tooling, chat, games, and GPU-accelerated rendering.
Tags:
  - assistant
  - terminal
  - chat
  - desktop
ManifestType: defaultLocale
ManifestVersion: 1.6.0
"@

Set-Content -Path (Join-Path $manifestRoot 'DylanBurton.Jarvis.yaml') -Value $versionManifest -Encoding UTF8
Set-Content -Path (Join-Path $manifestRoot 'DylanBurton.Jarvis.installer.yaml') -Value $installerManifest -Encoding UTF8
Set-Content -Path (Join-Path $manifestRoot 'DylanBurton.Jarvis.locale.en-US.yaml') -Value $localeManifest -Encoding UTF8

Write-Host "Generated winget manifests in $manifestRoot"
