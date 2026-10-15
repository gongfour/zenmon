# Tauri embeds Common Controls v6 in the application binary, but its resource
# compiler deliberately excludes library unit-test executables. Without this
# manifest the Windows loader exits with STATUS_ENTRYPOINT_NOT_FOUND before the
# harness can run (the dialog dependency imports v6 controls).
# Attach the same dependency to the exact test artifact reported by Cargo;
# production binaries and their manifests are not modified.
$ErrorActionPreference = 'Stop'
$buildOutput = & cargo test --locked -p zenmon-tray --lib --no-run --message-format=json
if ($LASTEXITCODE -ne 0) { throw 'Tray test compilation failed' }
$artifacts = @($buildOutput | ForEach-Object { $_ | ConvertFrom-Json } | Where-Object {
    $_.reason -eq 'compiler-artifact' -and $_.profile.test -and
    $_.target.name -eq 'zenmon_tray' -and $_.executable
})
if ($artifacts.Count -ne 1) { throw 'Expected exactly one tray library test executable' }
$testExecutable = $artifacts[0].executable
$kits = Join-Path ${env:ProgramFiles(x86)} 'Windows Kits\10\bin'
$manifestTool = Get-ChildItem "$kits\*\x64\mt.exe" |
    Sort-Object { [version] $_.Directory.Parent.Name } -Descending |
    Select-Object -First 1
if (!$manifestTool) { throw 'Windows SDK manifest tool (mt.exe) not found' }
$manifest = Join-Path $PSScriptRoot '..\tray\src-tauri\tests\windows.manifest'
& $manifestTool.FullName -nologo -manifest $manifest "-outputresource:$testExecutable;#1"
if ($LASTEXITCODE -ne 0) { throw 'Could not embed test manifest' }
& $testExecutable
if ($LASTEXITCODE -ne 0) { throw 'Tray tests failed' }
