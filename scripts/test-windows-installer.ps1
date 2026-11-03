[CmdletBinding()]
param([string]$Iscc = 'ISCC.exe', [string]$OldBinary)
$ErrorActionPreference = 'Stop'
$root = Split-Path $PSScriptRoot -Parent
Set-Location -LiteralPath $root
$uninstallKey = 'HKCU:\Software\Microsoft\Windows\CurrentVersion\Uninstall\sshm.qmtdlt.windows.x64_is1'
if (Test-Path $uninstallKey) { throw 'An existing managed installation is present. Use a disposable Windows account for installer tests.' }
$version = [regex]::Match((Get-Content -Raw Cargo.toml), '(?m)^version = "(\d+\.\d+\.\d+)"\r?$').Groups[1].Value
$binary = Join-Path $root 'target\x86_64-pc-windows-msvc\release\sshm.exe'
if (-not (Test-Path -LiteralPath $binary)) { throw 'Build the Windows package first.' }
$testRoot = Join-Path $root ('dist\windows-tests\run-' + [guid]::NewGuid().ToString('N'))
$install = Join-Path $testRoot '中文 空格安装目录'
New-Item -ItemType Directory -Force -Path $testRoot | Out-Null
$originalPath = [Environment]::GetEnvironmentVariable('Path', 'User')
$pathKey = [Microsoft.Win32.Registry]::CurrentUser.OpenSubKey('Environment', $true)
$originalKind = $pathKey.GetValueKind('Path')
$originalRawPath = $pathKey.GetValue('Path', $null, [Microsoft.Win32.RegistryValueOptions]::DoNotExpandEnvironmentNames)
$results = [Collections.Generic.List[object]]::new()
$dataDirs = @((Join-Path $env:APPDATA 'sshm'), (Join-Path $env:USERPROFILE '.ssh'))
function Data-Snapshot {
    $snapshot = @{}
    foreach ($directory in $dataDirs) {
        if (Test-Path -LiteralPath $directory) {
            Get-ChildItem -LiteralPath $directory -File -Recurse | Where-Object Extension -ne '.lock' | ForEach-Object {
                $filePath = $_.FullName
                try { $snapshot[$filePath] = (Get-FileHash -LiteralPath $filePath -Algorithm SHA256).Hash }
                catch [System.UnauthorizedAccessException] {
                    # Do not request elevation or alter ACLs for protected SSH files.
                    $snapshot[$filePath] = "unreadable:$($_.Exception.GetType().Name)"
                }
            }
        }
    }
    return $snapshot
}
$before = Data-Snapshot
# Only retain this in memory. No credential targets or account names are logged.
$credentialsBefore = (& cmdkey.exe /list | Out-String)
function Assert-Test([bool]$Condition, [string]$Name) {
    $results.Add([ordered]@{ test = $Name; passed = $Condition })
    if (-not $Condition) { throw "FAILED: $Name" }
    Write-Host "PASS: $Name"
}
function Invoke-Installer([string]$Package, [string]$Directory, [string]$Label) {
    $log = Join-Path $testRoot "$Label.log"
    $info = [Diagnostics.ProcessStartInfo]::new($Package)
    $info.UseShellExecute = $false
    foreach ($argument in @('/VERYSILENT','/SUPPRESSMSGBOXES','/NORESTART','/NOCLOSEAPPLICATIONS','/NORESTARTAPPLICATIONS',"/DIR=$Directory","/LOG=$log")) { [void]$info.ArgumentList.Add($argument) }
    $process = [Diagnostics.Process]::Start($info)
    if (-not $process.WaitForExit(300000)) { throw "Installer timeout; inspect $log. Do not terminate an in-progress installation blindly." }
    return $process.ExitCode
}
function Uninstall([string]$Directory) {
    $uninstaller = Join-Path $Directory 'unins000.exe'
    if (Test-Path -LiteralPath $uninstaller) {
        $process = Start-Process -FilePath $uninstaller -ArgumentList '/VERYSILENT /SUPPRESSMSGBOXES /NORESTART' -PassThru -WindowStyle Hidden
        if (-not $process.WaitForExit(300000)) { throw 'Uninstaller timeout.' }
        if ($process.ExitCode -ne 0) { throw "Uninstall failed: $($process.ExitCode)" }
    }
}
function Make-Package([string]$Name, [string]$Version, [string]$Binary, [string]$Script) {
    $payload = Join-Path $testRoot $Name
    New-Item -ItemType Directory -Force -Path $payload | Out-Null
    Copy-Item -LiteralPath $Binary -Destination (Join-Path $payload 'sshm.exe')
    & $Iscc /Q "/DAppVersion=$Version" "/DPayloadDir=$payload" "/DTrial=1" "/O$testRoot" "/F$Name" $Script
    if ($LASTEXITCODE) { throw 'Test package compilation failed.' }
    return (Join-Path $testRoot "$Name.exe")
}
try {
    $script = Join-Path $PSScriptRoot 'windows\sshm.iss'
    $newPackage = Make-Package 'current' $version $binary $script
    if ($OldBinary) {
        $oldVersion = ((& $OldBinary --version) -replace '^sshm ', '').Trim()
        Assert-Test ([version]$oldVersion -lt [version]$version) 'old binary version precedes current version'
    } else {
        # CI fixture, clearly distinguished from the real 0.4.0 upgrade run.
        $oldVersion = '0.3.0'
        $fixture = Join-Path $testRoot 'old.rs'
        'fn main() { println!("sshm 0.3.0"); }' | Set-Content -LiteralPath $fixture -Encoding utf8
        $OldBinary = Join-Path $testRoot 'old.exe'
        & rustc --edition=2024 --crate-name installer_fixture $fixture -o $OldBinary
        if ($LASTEXITCODE) { throw 'Old-version fixture compilation failed.' }
    }
    $oldPackage = Make-Package 'old' $oldVersion $OldBinary $script
    Assert-Test ((Invoke-Installer $oldPackage $install 'first-install') -eq 0) 'first installation into a Chinese path containing spaces'
    $entry = Join-Path $install 'sshm.exe'
    Assert-Test ((& $entry --version) -eq "sshm $oldVersion") 'old executable starts'
    $ownedPath = [Environment]::GetEnvironmentVariable('Path', 'User')
    Assert-Test ($ownedPath.Split(';').Where({ $_ -eq $install }).Count -eq 1) 'one user PATH entry added'
    'Unmanaged content must survive uninstall.' | Set-Content -LiteralPath (Join-Path $install 'user-note.txt')

    $lock = [IO.File]::Open($entry, [IO.FileMode]::Open, [IO.FileAccess]::Read, [IO.FileShare]::Read)
    try {
        Assert-Test ((Invoke-Installer $newPackage $install 'locked-file') -eq 7) 'locked executable blocks upgrade without killing processes'
        Assert-Test ((Get-FileHash -LiteralPath $entry).Hash -eq (Get-FileHash -LiteralPath $OldBinary).Hash) 'locked-file failure preserves original bytes'
    } finally { $lock.Dispose() }

    # A genuine running executable (a small fixture) exercises the Windows image lock.
    $holdingSource = Join-Path $testRoot 'holding.rs'
    @'
fn main() {
    if std::env::args().nth(1).as_deref() == Some("--version") { println!("sshm 0.3.0"); }
    else { std::thread::sleep(std::time::Duration::from_secs(12)); }
}
'@ | Set-Content -LiteralPath $holdingSource -Encoding utf8
    $holding = Join-Path $testRoot 'holding.exe'
    & rustc --crate-name holding $holdingSource -o $holding
    if ($LASTEXITCODE) { throw 'Running-image fixture compilation failed.' }
    Copy-Item -LiteralPath $holding -Destination $entry -Force
    $running = Start-Process -FilePath $entry -PassThru -WindowStyle Hidden
    try { Assert-Test ((Invoke-Installer $newPackage $install 'running-image') -eq 7) 'running image blocks replacement' }
    finally { if (-not $running.WaitForExit(20000)) { throw 'Fixture process failed to exit.' }; Copy-Item -LiteralPath $OldBinary -Destination $entry -Force }

    $mutex = [Threading.Mutex]::new($false, 'Local\sshm-inno-install')
    try { Assert-Test ((Invoke-Installer $newPackage $install 'concurrent') -ne 0) 'concurrent installer is rejected' }
    finally { $mutex.Dispose() }

    # Compile an instrumented INSTALLER only in the isolated test directory. The
    # release binary and production update signature checks are never bypassed.
    $faultScript = Join-Path $testRoot 'post-install-failure.iss'
    $source = Get-Content -LiteralPath $script -Raw
    $source.Replace("if ReadVersion(ExpandConstant('{app}\sshm.exe')) <> '{#AppVersion}' then begin", 'if True then begin') | Set-Content -LiteralPath $faultScript -Encoding utf8
    $faultPackage = Make-Package 'failure-after-copy' $version $binary $faultScript
    Assert-Test ((Invoke-Installer $faultPackage $install 'post-copy-failure') -ne 0) 'injected post-copy failure reports unsuccessful installation'
    Assert-Test ((& $entry --version) -eq "sshm $oldVersion") 'post-copy failure restores runnable old version'
    Assert-Test ([Environment]::GetEnvironmentVariable('Path','User') -eq $ownedPath) 'failed upgrade preserves PATH'

    # A test-only cancellation event exercises Inno cancellation before writes.
    $cancelScript = Join-Path $testRoot 'cancel.iss'
    ($source + "`nfunction NextButtonClick(CurPageID: Integer): Boolean;`nbegin`n  Result := CurPageID <> wpReady;`nend;`n") | Set-Content -LiteralPath $cancelScript -Encoding utf8
    $cancelPackage = Make-Package 'cancel' $version $binary $cancelScript
    Assert-Test ((Invoke-Installer $cancelPackage $install 'cancelled') -ne 0) 'test-only cancellation stops installation before writes'
    Assert-Test ((& $entry --version) -eq "sshm $oldVersion") 'cancelled installation preserves old version'

    Assert-Test ((Invoke-Installer $newPackage $install 'upgrade') -eq 0) 'upgrade installer completes'
    Assert-Test ((& $entry --version) -eq "sshm $version") 'upgraded executable version verified'
    Assert-Test ((& (Join-Path $install 'sshm.previous.exe') --version) -eq "sshm $oldVersion") 'previous version retained for recovery'
    Assert-Test ((Invoke-Installer $newPackage $install 'reinstall') -eq 0) 'same-version reinstall succeeds'
    Assert-Test ([Environment]::GetEnvironmentVariable('Path','User') -eq $ownedPath) 'repeated installation does not duplicate or change other PATH entries'
    Assert-Test ((Invoke-Installer $oldPackage $install 'downgrade') -eq 7) 'accidental downgrade rejected'

    $processPath = $env:Path
    try {
        # A new child with the persisted machine+user PATH models the environment
        # of a new terminal; already-open parent shells cannot refresh themselves.
        $env:Path = [Environment]::GetEnvironmentVariable('Path','Machine') + ';' + [Environment]::GetEnvironmentVariable('Path','User')
        Assert-Test ((& "$env:SystemRoot\System32\cmd.exe" /d /c 'sshm --version') -eq "sshm $version") 'new CMD resolves sshm via persisted PATH'
        Assert-Test ((& "$env:SystemRoot\System32\WindowsPowerShell\v1.0\powershell.exe" -NoProfile -Command 'sshm --version') -eq "sshm $version") 'new PowerShell resolves sshm via persisted PATH'
    } finally { $env:Path = $processPath }
    Uninstall $install
    Assert-Test (-not (Test-Path -LiteralPath $entry)) 'uninstall removes managed executable'
    Assert-Test (Test-Path -LiteralPath (Join-Path $install 'user-note.txt')) 'uninstall preserves unmanaged files'
    Assert-Test ([Environment]::GetEnvironmentVariable('Path','User') -eq $originalPath) 'uninstall restores unrelated user PATH exactly'

    # Existing equivalent entry is not owned by sshm and must survive uninstall.
    $preexistingPath = $originalRawPath + ';"' + $install.ToUpperInvariant() + '\"'
    $pathKey.SetValue('Path', $preexistingPath, $originalKind)
    Assert-Test ((Invoke-Installer $newPackage $install 'preexisting-path') -eq 0) 'install accepts pre-existing case-insensitive quoted PATH entry'
    Assert-Test ($pathKey.GetValue('Path',$null,[Microsoft.Win32.RegistryValueOptions]::DoNotExpandEnvironmentNames) -eq $preexistingPath) 'equivalent pre-existing PATH is not duplicated'
    Uninstall $install
    Assert-Test ($pathKey.GetValue('Path',$null,[Microsoft.Win32.RegistryValueOptions]::DoNotExpandEnvironmentNames) -eq $preexistingPath) 'uninstall retains PATH entry not added by sshm'
    $pathKey.SetValue('Path', $originalRawPath, $originalKind)

    $default = Join-Path $env:LOCALAPPDATA 'Programs\sshm'
    if (Test-Path -LiteralPath $default) { throw 'Default install directory already exists; refusing to modify it during tests.' }
    Assert-Test ((Invoke-Installer $newPackage $default 'default-directory') -eq 0) 'default per-user installation succeeds without elevation'
    Assert-Test ((& (Join-Path $default 'sshm.exe') --version) -eq "sshm $version") 'default-directory version verified'
    Uninstall $default

    $after = Data-Snapshot
    $same = $before.Count -eq $after.Count
    foreach ($name in $before.Keys) { if ($before[$name] -ne $after[$name]) { $same = $false } }
    Assert-Test $same 'readable real AppData connections, bindings and SSH files remain byte-identical (protected files excluded)'
    Assert-Test ((& cmdkey.exe /list | Out-String) -eq $credentialsBefore) 'Windows credential listing unchanged (not a login validation)'
} finally {
    # This script owns only test installations registered at these exact paths.
    try {
        if (Test-Path $uninstallKey) {
            $registered = (Get-ItemProperty $uninstallKey).InstallLocation.TrimEnd('\')
            $allowed = @($install, (Join-Path $env:LOCALAPPDATA 'Programs\sshm'))
            if ($allowed -contains $registered) { Uninstall $registered }
            else { Write-Warning 'Unrecognized installation appeared during testing; not uninstalling it.' }
        }
    } finally {
        $pathKey.SetValue('Path', $originalRawPath, $originalKind)
        $pathKey.Dispose()
        $results | ConvertTo-Json -Depth 4 | Set-Content -LiteralPath (Join-Path $root 'dist\windows-tests\results.json') -Encoding utf8
        Write-Host "Installer logs and fixtures: $testRoot"
    }
}
