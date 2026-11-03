[CmdletBinding()]
param(
    [string]$OAuthConfig = $env:SSHM_GOOGLE_OAUTH_CONFIG,
    [string]$UpdatePublicKeyFile,
    [string]$Iscc = 'ISCC.exe',
    [switch]$Sign,
    [string]$SignTool = 'signtool.exe'
)
$ErrorActionPreference = 'Stop'
$root = Split-Path $PSScriptRoot -Parent
Set-Location -LiteralPath $root
$version = [regex]::Match((Get-Content -Raw Cargo.toml), '(?m)^version = "(\d+\.\d+\.\d+)"\r?$').Groups[1].Value
if (-not $version) { throw 'Cargo version must be a stable three-part version.' }
if ($env:GITHUB_REF_TYPE -eq 'tag' -and $env:GITHUB_REF_NAME -ne "v$version") { throw 'Tag does not match Cargo version.' }
$names = @('SSHM_GOOGLE_CLIENT_ID','SSHM_GOOGLE_CLIENT_SECRET','SSHM_UPDATE_PUBLIC_KEY')
$saved = @{}
foreach ($name in $names) { $saved[$name] = [Environment]::GetEnvironmentVariable($name) }
$stage = Join-Path ([IO.Path]::GetTempPath()) ('sshm-package-' + [guid]::NewGuid().ToString('N'))
try {
    if ($OAuthConfig) {
        # Never print this object, the JSON filename, or command-line secret values.
        try { $config = (Get-Content -LiteralPath $OAuthConfig -Raw | ConvertFrom-Json).installed }
        catch { throw 'Cannot read the existing desktop OAuth configuration (contents omitted).' }
        $env:SSHM_GOOGLE_CLIENT_ID = $config.client_id
        $env:SSHM_GOOGLE_CLIENT_SECRET = $config.client_secret
    }
    if ($env:SSHM_GOOGLE_CLIENT_ID -notlike '*.apps.googleusercontent.com') {
        throw 'Existing desktop OAuth build configuration required. Supply -OAuthConfig or the existing build environment.'
    }
    if ($UpdatePublicKeyFile) { $env:SSHM_UPDATE_PUBLIC_KEY = (Get-Content -LiteralPath $UpdatePublicKeyFile -Raw).Trim() }
    if ($env:SSHM_UPDATE_PUBLIC_KEY -and [Convert]::FromBase64String($env:SSHM_UPDATE_PUBLIC_KEY).Length -ne 32) {
        throw 'Update public key must be the existing 32-byte Ed25519 public key.'
    }
    if ($Sign) {
        if (-not $env:SSHM_UPDATE_PUBLIC_KEY -or $env:SSHM_WINDOWS_CERT_SHA256 -notmatch '^[a-fA-F0-9]{64}$' -or $env:SSHM_WINDOWS_SIGN_SHA1 -notmatch '^[a-fA-F0-9]{40}$') {
            throw 'Signed builds require the shared update public key, SSHM_WINDOWS_CERT_SHA256 and SSHM_WINDOWS_SIGN_SHA1.'
        }
        if ($env:SSHM_TIMESTAMP_URL -notmatch '^https://') { throw 'An HTTPS SSHM_TIMESTAMP_URL is required for signing.' }
    }
    & cargo build --release --locked --bin sshm --target x86_64-pc-windows-msvc
    if ($LASTEXITCODE) { throw 'Cargo build failed.' }
    $binary = Join-Path $root 'target\x86_64-pc-windows-msvc\release\sshm.exe'
    if ((& $binary --version) -ne "sshm $version" -or $LASTEXITCODE) { throw 'Built binary version mismatch.' }
    $bytes = [IO.File]::ReadAllBytes($binary)
    $text = [Text.Encoding]::UTF8.GetString($bytes)
    if (-not $text.Contains($env:SSHM_GOOGLE_CLIENT_ID)) { throw 'OAuth injection verification failed.' }
    if ($env:SSHM_UPDATE_PUBLIC_KEY -and -not $text.Contains($env:SSHM_UPDATE_PUBLIC_KEY)) { throw 'Update public key injection verification failed.' }
    New-Item -ItemType Directory -Path $stage | Out-Null
    Copy-Item -LiteralPath $binary -Destination (Join-Path $stage 'sshm.exe')
    function Sign-Artifact([string]$Path) {
        & $SignTool sign /sha1 $env:SSHM_WINDOWS_SIGN_SHA1 /s My /fd SHA256 /tr $env:SSHM_TIMESTAMP_URL /td SHA256 $Path
        if ($LASTEXITCODE) { throw 'Authenticode signing failed.' }
        $signature = Get-AuthenticodeSignature -LiteralPath $Path
        if ($signature.Status -ne 'Valid') { throw 'Windows trust verification failed.' }
        $hash = [Security.Cryptography.SHA256]::Create()
        try { $fingerprint = ([BitConverter]::ToString($hash.ComputeHash($signature.SignerCertificate.RawData))).Replace('-', '') } finally { $hash.Dispose() }
        if ($fingerprint -ne $env:SSHM_WINDOWS_CERT_SHA256) { throw 'Signing certificate does not match pinned publisher.' }
    }
    if ($Sign) { Sign-Artifact (Join-Path $stage 'sshm.exe') }
    $destination = Join-Path $root $(if ($Sign) { 'dist\windows-signed' } else { 'dist\windows-local-trial' })
    New-Item -ItemType Directory -Force -Path $destination | Out-Null
    $trial = if ($Sign) { '0' } else { '1' }
    $name = "sshm-$version-windows-x86_64" + $(if ($Sign) { '' } else { '-local-trial' })
    $compilerArgs = @('/Qp', "/DAppVersion=$version", "/DPayloadDir=$stage", "/DTrial=$trial", "/O$destination", "/F$name")
    if ($Sign) {
        $signPath = (Get-Command $SignTool -ErrorAction Stop).Source
        $signCommand = '$q' + $signPath + '$q sign /sha1 ' + $env:SSHM_WINDOWS_SIGN_SHA1 + ' /s My /fd SHA256 /tr $q' + $env:SSHM_TIMESTAMP_URL + '$q /td SHA256 $f'
        $compilerArgs += '/Ssshm=' + $signCommand
    }
    & $Iscc @compilerArgs (Join-Path $PSScriptRoot 'windows\sshm.iss')
    if ($LASTEXITCODE) { throw 'Inno Setup compilation failed.' }
    $package = Join-Path $destination "$name.exe"
    if ($Sign) { Sign-Artifact $package }
    $report = [ordered]@{
        version = $version; target = 'x86_64-pc-windows-msvc'; tag = "v$version"
        package = "$name.exe"; size = (Get-Item -LiteralPath $package).Length
        sha256 = (Get-FileHash -LiteralPath $package -Algorithm SHA256).Hash.ToLowerInvariant()
        signed = [bool]$Sign; oauth_embedded = $true
        update_public_key_embedded = [bool]$env:SSHM_UPDATE_PUBLIC_KEY
        publisher_pinned = [bool]$env:SSHM_WINDOWS_CERT_SHA256
        source_commit = (& git rev-parse HEAD); source_dirty = [bool](& git status --porcelain)
        note = 'Build evidence only; not a signed update manifest. No Google login or cross-device sync verified.'
    }
    $report | ConvertTo-Json | Set-Content -LiteralPath (Join-Path $destination 'build-info.json') -Encoding utf8
    Write-Host "Package: $package"
    if (-not $Sign) { Write-Host 'LOCAL TRIAL ONLY: unsigned; not eligible for trusted online distribution.' }
} finally {
    foreach ($name in $names) { [Environment]::SetEnvironmentVariable($name, $saved[$name]) }
    # Delete only the exact temporary staging directory created by this invocation.
    if (Test-Path -LiteralPath $stage) {
        $resolved = [IO.Path]::GetFullPath($stage)
        $tempRoot = [IO.Path]::GetFullPath([IO.Path]::GetTempPath())
        if (-not $resolved.StartsWith($tempRoot, [StringComparison]::OrdinalIgnoreCase) -or (Split-Path $resolved -Leaf) -notlike 'sshm-package-*') { throw 'Unsafe staging cleanup path.' }
        Remove-Item -LiteralPath $resolved -Recurse -Force
    }
}
