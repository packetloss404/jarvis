param(
    [string]$Version = $env:JARVIS_VERSION
)

$ErrorActionPreference = 'Stop'

$ScriptDir = Split-Path -Parent $MyInvocation.MyCommand.Path
$ProjectDir = Split-Path -Parent $ScriptDir
Set-Location $ProjectDir

if (-not $Version) {
    $cargo = Get-Content (Join-Path $ProjectDir 'Cargo.toml') -Raw
    if ($cargo -match 'version\s*=\s*"([^"]+)"') {
        $Version = $Matches[1]
    } else {
        $Version = '0.1.0'
    }
}

$appName = 'Jarvis'
$binaryName = 'jarvis.exe'
$upgradeCode = 'D4A8B39D-7E15-4E42-A8D4-2F1D1B7B1D33'

Write-Host "Packaging $appName v$Version for Windows..."

cargo build --release

$releaseDir = Join-Path $ProjectDir 'target\release'
$stagingRoot = Join-Path $releaseDir 'windows-installer'
$installRoot = Join-Path $stagingRoot 'Jarvis'
$assetsRoot = Join-Path $installRoot 'assets'
$sourcePanels = Join-Path $ProjectDir 'assets\panels'
$sourceBinary = Join-Path $releaseDir $binaryName
$sourceIcon = Join-Path $ProjectDir 'assets\jarvis.ico'
$msiPath = Join-Path $releaseDir 'Jarvis.msi'
$zipPath = Join-Path $releaseDir 'jarvis-windows-x64.zip'
$wxsPath = Join-Path $stagingRoot 'jarvis.wxs'

Remove-Item $stagingRoot -Recurse -Force -ErrorAction SilentlyContinue
Remove-Item $msiPath -Force -ErrorAction SilentlyContinue
Remove-Item $zipPath -Force -ErrorAction SilentlyContinue

New-Item -ItemType Directory -Path $assetsRoot -Force | Out-Null
Copy-Item $sourceBinary (Join-Path $installRoot 'Jarvis.exe')
Copy-Item $sourcePanels (Join-Path $assetsRoot 'panels') -Recurse
Copy-Item $sourceIcon (Join-Path $installRoot 'jarvis.ico')

$componentNodes = [System.Collections.Generic.List[string]]::new()
$componentRefs = [System.Collections.Generic.List[string]]::new()
$dirIds = @{}
$componentIndex = 0

function Normalize-Id([string]$value) {
    $normalized = ($value -replace '[^A-Za-z0-9_]', '_')
    if ($normalized -notmatch '^[A-Za-z_]') {
        $normalized = "D_$normalized"
    }
    return $normalized
}

function Normalize-PathKey([string]$value) {
    return $value.Replace('/', '\').TrimEnd('\')
}

function Get-DirectoryId([string]$dirPath) {
    $dirPath = Normalize-PathKey $dirPath
    $rootPath = Normalize-PathKey $installRoot
    if ($dirPath -eq $rootPath) {
        return 'INSTALLFOLDER'
    }

    $relativePath = $dirPath.Substring($rootPath.Length).TrimStart('\\')
    if (-not $relativePath) {
        return 'INSTALLFOLDER'
    }

    return Normalize-Id $relativePath
}

function Ensure-Directory([string]$dirPath) {
    $dirPath = Normalize-PathKey $dirPath
    if ($dirIds.ContainsKey($dirPath)) {
        return $dirIds[$dirPath]
    }

    $parent = Split-Path -Parent $dirPath
    Ensure-Directory $parent | Out-Null
    $id = Get-DirectoryId $dirPath
    $dirIds[$dirPath] = $id
    return $id
}

Get-ChildItem $installRoot -Recurse -Directory | ForEach-Object {
    Ensure-Directory $_.FullName | Out-Null
}

$relativeDirs = Get-ChildItem $installRoot -Recurse -Directory |
    Sort-Object FullName |
    ForEach-Object { $_.FullName.Substring($installRoot.Length).TrimStart('\\') } |
    Where-Object { $_ }

$directoryLines = [System.Collections.Generic.List[string]]::new()
$previousParts = @()

foreach ($relativeDir in $relativeDirs) {
    $parts = $relativeDir -split '\\'
    $common = 0
    while ($common -lt $parts.Length -and $common -lt $previousParts.Length -and $parts[$common] -eq $previousParts[$common]) {
        $common += 1
    }

    for ($i = $previousParts.Length; $i -gt $common; $i--) {
        $indent = '      ' + ('  ' * ($i - 1))
        $directoryLines.Add("$indent</Directory>") | Out-Null
    }

    for ($i = $common; $i -lt $parts.Length; $i++) {
        $currentRelative = ($parts[0..$i] -join '\\')
        $dirId = Normalize-Id $currentRelative
        $indent = '      ' + ('  ' * $i)
        $directoryLines.Add(($indent + '<Directory Id="' + $dirId + '" Name="' + $parts[$i] + '">')) | Out-Null
    }

    $previousParts = $parts
}

for ($i = $previousParts.Length; $i -gt 0; $i--) {
    $indent = '      ' + ('  ' * ($i - 1))
    $directoryLines.Add("$indent</Directory>") | Out-Null
}

Get-ChildItem $installRoot -Recurse -File | ForEach-Object {
    $dirId = Ensure-Directory $_.DirectoryName
    $componentIndex += 1
    $componentId = "Cmp$componentIndex"
    $fileId = "File$componentIndex"
    $sourcePath = $_.FullName.Replace('&', '&amp;')
    $componentNodes.Add(@"
    <DirectoryRef Id="$dirId">
      <Component Id="$componentId" Guid="*">
        <File Id="$fileId" Source="$sourcePath" KeyPath="yes" />
      </Component>
    </DirectoryRef>
"@) | Out-Null
    $componentRefs.Add(('      <ComponentRef Id="' + $componentId + '" />')) | Out-Null
}

$menuComponent = @"
    <DirectoryRef Id="ProgramMenuDir">
      <Component Id="ApplicationShortcut" Guid="*">
        <Shortcut Id="JarvisStartMenuShortcut"
                  Name="Jarvis"
                  Description="Launch Jarvis"
                  Target="[INSTALLFOLDER]Jarvis.exe"
                  WorkingDirectory="INSTALLFOLDER" />
        <RemoveFolder Id="ProgramMenuDirRemove" On="uninstall" />
        <RegistryValue Root="HKCU"
                       Key="Software\Jarvis"
                       Name="StartMenuShortcut"
                       Type="integer"
                       Value="1"
                       KeyPath="yes" />
      </Component>
    </DirectoryRef>
"@

$componentRefs.Add('      <ComponentRef Id="ApplicationShortcut" />') | Out-Null

$directoriesXml = @"
      <Directory Id="INSTALLFOLDER" Name="Jarvis">
$(($directoryLines -join "`n"))
      </Directory>
"@

$wxs = @"
<?xml version="1.0" encoding="UTF-8"?>
<Wix xmlns="http://wixtoolset.org/schemas/v4/wxs">
  <Package Name="Jarvis"
           Manufacturer="Dylan Burton"
           Version="$Version"
           UpgradeCode="$upgradeCode"
           Language="1033"
           Scope="perMachine">
    <SummaryInformation Description="Jarvis desktop app" Manufacturer="Dylan Burton" />
    <MediaTemplate EmbedCab="yes" />
    <MajorUpgrade DowngradeErrorMessage="A newer version of Jarvis is already installed." />
    <Icon Id="JarvisIcon" SourceFile="$($sourceIcon.Replace('&', '&amp;'))" />
    <Property Id="ARPPRODUCTICON" Value="JarvisIcon" />

    <StandardDirectory Id="ProgramFiles64Folder">
$directoriesXml
    </StandardDirectory>

    <StandardDirectory Id="ProgramMenuFolder">
      <Directory Id="ProgramMenuDir" Name="Jarvis" />
    </StandardDirectory>

$(($componentNodes -join "`n"))
$menuComponent
    <Feature Id="MainFeature" Title="Jarvis" Level="1">
$(($componentRefs -join "`n"))
    </Feature>
  </Package>
</Wix>
"@

Set-Content -Path $wxsPath -Value $wxs -Encoding UTF8

if (-not (Get-Command wix -ErrorAction SilentlyContinue)) {
    throw 'WiX CLI not found on PATH. Install with: dotnet tool install --global wix'
}

wix build $wxsPath -arch x64 -o $msiPath

Compress-Archive -Path (Join-Path $installRoot '*') -DestinationPath $zipPath

Write-Host "Built MSI: $msiPath"
Write-Host "Built zip: $zipPath"
