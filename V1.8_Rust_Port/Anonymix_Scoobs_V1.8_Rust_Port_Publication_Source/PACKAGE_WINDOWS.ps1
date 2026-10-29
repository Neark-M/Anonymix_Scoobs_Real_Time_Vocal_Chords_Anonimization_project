# Maintainer-only packaging. Never invoked by the application at runtime.
$ErrorActionPreference = 'Stop'
$root = $PSScriptRoot
$exe = Join-Path $root 'target\release\anonymix-scoobs.exe'
if (-not (Test-Path -LiteralPath $exe -PathType Leaf)) { throw 'Build the Windows x64 Release executable first.' }
# Refuse an accidental Linux/other-architecture artifact in a Windows package.
$bytes = [IO.File]::ReadAllBytes($exe)
if ($bytes.Length -lt 64 -or $bytes[0] -ne 0x4D -or $bytes[1] -ne 0x5A) { throw 'Executable is not a Windows PE file.' }
$pe = [BitConverter]::ToInt32($bytes, 0x3C)
if ($pe -lt 0 -or $pe + 96 -gt $bytes.Length) { throw 'Invalid PE header.' }
if ([BitConverter]::ToUInt32($bytes, $pe) -ne 0x00004550 -or [BitConverter]::ToUInt16($bytes, $pe+4) -ne 0x8664) { throw 'Expected a Windows x64 executable.' }
# GUI subsystem (2), not Console (3). The Rust entry point sets this at link time.
if ([BitConverter]::ToUInt16($bytes, $pe+24+68) -ne 2) { throw 'Public executable still has the console subsystem.' }
$runtime = Join-Path $root 'runtime'
foreach ($file in @('SDL3.dll','SDL3_ttf.dll','portaudio.dll','logo.bmp','THIRD_PARTY_NOTICES.txt','LICENSES')) {
    if (-not (Test-Path -LiteralPath (Join-Path $runtime $file))) { throw "Missing runtime item: $file" }
}
$dist = Join-Path $root 'dist'
New-Item -ItemType Directory -Path $dist -Force | Out-Null
$stage = Join-Path $dist ('package-' + [guid]::NewGuid().ToString('N'))
$folder = Join-Path $stage 'Anonymix_Scoobs_V1.8_Rust_Port_Windows_x64'
$zip = Join-Path $dist 'Anonymix_Scoobs_V1.8_Rust_Port_Windows_x64.zip'
try {
    New-Item -ItemType Directory -Path $folder -Force | Out-Null
    Copy-Item -LiteralPath $exe -Destination $folder
    Get-ChildItem -LiteralPath $runtime | Copy-Item -Destination $folder -Recurse
    Copy-Item -LiteralPath (Join-Path $root 'README_USER.md') -Destination (Join-Path $folder 'README.md')
    Copy-Item -LiteralPath (Join-Path $root 'RELEASE_NOTES.md') -Destination $folder
    New-Item -ItemType Directory -Path (Join-Path $folder 'recordings') | Out-Null
    Compress-Archive -LiteralPath $folder -DestinationPath $zip -Force
    $hash = (Get-FileHash -LiteralPath $zip -Algorithm SHA256).Hash.ToLowerInvariant()
    [IO.File]::WriteAllText($zip + '.sha256', $hash + '  ' + [IO.Path]::GetFileName($zip) + [Environment]::NewLine)
    Write-Host "Ready-to-run package: $zip"
    Write-Host "SHA256: $hash"
} finally {
    if (Test-Path -LiteralPath $stage) { Remove-Item -LiteralPath $stage -Recurse -Force }
}
