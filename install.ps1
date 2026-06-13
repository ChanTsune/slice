<#
.SYNOPSIS
    Install the latest (or a pinned) `slice` release binary on Windows.
.DESCRIPTION
    Run via:
        irm https://raw.githubusercontent.com/ChanTsune/slice/main/install.ps1 | iex

    Environment overrides:
        SLICE_VERSION       version/tag to install (default: latest release)
        SLICE_INSTALL_DIR   directory to install into
                            (default: %LOCALAPPDATA%\slice\bin)
#>
$ErrorActionPreference = 'Stop'

$Repo = 'ChanTsune/slice'
$Bin = 'slice'
$InstallDir = if ($env:SLICE_INSTALL_DIR) { $env:SLICE_INSTALL_DIR } else { Join-Path $env:LOCALAPPDATA 'slice\bin' }

function Get-Target {
    # PROCESSOR_ARCHITEW6432 reflects the native arch when running under WOW64.
    $arch = if ($env:PROCESSOR_ARCHITEW6432) { $env:PROCESSOR_ARCHITEW6432 } else { $env:PROCESSOR_ARCHITECTURE }
    switch ($arch) {
        'AMD64' { 'x86_64-pc-windows-msvc' }
        'ARM64' { 'aarch64-pc-windows-msvc' }
        'x86' { 'i686-pc-windows-msvc' }
        default { throw "unsupported architecture: $arch" }
    }
}

function Get-LatestVersion {
    if ($env:SLICE_VERSION) { return $env:SLICE_VERSION }
    # Follow the /releases/latest redirect to read the tag without the API.
    $resp = Invoke-WebRequest -Uri "https://github.com/$Repo/releases/latest" -UseBasicParsing
    $final = $resp.BaseResponse.ResponseUri
    if (-not $final) { $final = $resp.BaseResponse.RequestMessage.RequestUri }
    # The redirect lands on .../releases/tag/<version>. Anything else (e.g. a
    # repo with no releases redirects to .../releases) is not a version.
    if (-not $final -or $final.AbsoluteUri -notmatch '/releases/tag/(.+)$') {
        throw "no published release found for $Repo (resolved to: $($final.AbsoluteUri))"
    }
    return $Matches[1]
}

$target = Get-Target
$version = Get-LatestVersion
if (-not $version) { throw 'could not determine a version to install' }

$stem = "$Bin-$version-$target"
$archive = "$stem.zip"
$url = "https://github.com/$Repo/releases/download/$version/$archive"

Write-Host "Installing $Bin $version ($target)"

$tmp = Join-Path ([System.IO.Path]::GetTempPath()) ("slice-" + [System.Guid]::NewGuid().ToString('N'))
New-Item -ItemType Directory -Path $tmp -Force | Out-Null
try {
    $zip = Join-Path $tmp $archive
    try {
        Invoke-WebRequest -Uri $url -OutFile $zip -UseBasicParsing
    }
    catch {
        throw "download failed: $url ($($_.Exception.Message))"
    }
    Expand-Archive -Path $zip -DestinationPath $tmp -Force

    # Archives expand into a <stem>\ directory containing the binary.
    $src = Join-Path $tmp "$stem\$Bin.exe"
    if (-not (Test-Path $src)) { throw "binary not found in archive (expected $stem\$Bin.exe)" }

    New-Item -ItemType Directory -Path $InstallDir -Force | Out-Null
    Copy-Item -Path $src -Destination (Join-Path $InstallDir "$Bin.exe") -Force
    Write-Host "Installed $Bin to $(Join-Path $InstallDir "$Bin.exe")"
}
finally {
    Remove-Item -Path $tmp -Recurse -Force -ErrorAction SilentlyContinue
}

# Add the install directory to the user PATH if it is not already there. Read and
# write the raw registry value: [Environment]::GetEnvironmentVariable expands
# %VARS% and SetEnvironmentVariable rewrites the key as REG_SZ, which would
# permanently flatten an existing REG_EXPAND_SZ user PATH.
$key = [Microsoft.Win32.Registry]::CurrentUser.OpenSubKey('Environment', $true)
try {
    $userPath = $key.GetValue('Path', '', [Microsoft.Win32.RegistryValueOptions]::DoNotExpandEnvironmentNames)
    if (($userPath -split ';') -notcontains $InstallDir) {
        $newPath = if ([string]::IsNullOrEmpty($userPath)) { $InstallDir } else { "$userPath;$InstallDir" }
        $key.SetValue('Path', $newPath, [Microsoft.Win32.RegistryValueKind]::ExpandString)
        $env:Path = "$env:Path;$InstallDir"
        Write-Host "Added $InstallDir to your user PATH (restart your shell to pick it up)."
    }
}
finally {
    $key.Dispose()
}

Write-Host ''
Write-Host 'Shell completions and a man page can be generated with:'
Write-Host "  $Bin --generate complete-powershell   # or complete-bash, complete-zsh, complete-fish, man"
