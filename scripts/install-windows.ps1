param(
    [string]$Version = "latest",
    [string]$InstallDir = "$env:LOCALAPPDATA\agent-ctrl\bin",
    [switch]$NoPath
)

$ErrorActionPreference = "Stop"

$repo = "k4cper-g/agent-ctrl"
$assetNamePattern = "x86_64-pc-windows-msvc.zip"

if ($Version -eq "latest") {
    $release = Invoke-RestMethod "https://api.github.com/repos/$repo/releases/latest"
} else {
    $tag = if ($Version.StartsWith("v")) { $Version } else { "v$Version" }
    $release = Invoke-RestMethod "https://api.github.com/repos/$repo/releases/tags/$tag"
}

$asset = $release.assets | Where-Object { $_.name -like "*$assetNamePattern" } | Select-Object -First 1
if (-not $asset) {
    throw "No Windows release asset matching *$assetNamePattern found for $($release.tag_name)."
}

$temp = Join-Path ([System.IO.Path]::GetTempPath()) "agent-ctrl-install-$([System.Guid]::NewGuid())"
$zip = Join-Path $temp $asset.name
$extract = Join-Path $temp "extract"

New-Item -ItemType Directory -Force -Path $temp, $extract, $InstallDir | Out-Null

try {
    Invoke-WebRequest -Uri $asset.browser_download_url -OutFile $zip
    Expand-Archive -Path $zip -DestinationPath $extract -Force

    $exe = Get-ChildItem -Path $extract -Recurse -Filter "agent-ctrl.exe" | Select-Object -First 1
    if (-not $exe) {
        throw "Release asset did not contain agent-ctrl.exe."
    }

    Copy-Item -Path $exe.FullName -Destination (Join-Path $InstallDir "agent-ctrl.exe") -Force

    if (-not $NoPath) {
        $userPath = [Environment]::GetEnvironmentVariable("Path", "User")
        $parts = @($userPath -split ";" | Where-Object { $_ })
        if ($parts -notcontains $InstallDir) {
            $newPath = (@($parts) + $InstallDir) -join ";"
            [Environment]::SetEnvironmentVariable("Path", $newPath, "User")
            Write-Host "Added $InstallDir to the user PATH. Open a new terminal to use it."
        }
    }

    & (Join-Path $InstallDir "agent-ctrl.exe") info
    Write-Host "Installed agent-ctrl $($release.tag_name) to $InstallDir"
} finally {
    Remove-Item -LiteralPath $temp -Recurse -Force -ErrorAction SilentlyContinue
}
