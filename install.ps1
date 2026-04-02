$ErrorActionPreference = 'Stop'

$repo = "ooboai/oobo"
$apiUrl = "https://api.github.com/repos/$repo/releases/latest"

Write-Host ""
Write-Host "  oobo installer for Windows" -ForegroundColor Cyan
Write-Host ""

# --- Architecture detection ---

$arch = "x86_64"
try {
    $osArch = [System.Runtime.InteropServices.RuntimeInformation]::OSArchitecture
    if ($osArch -eq 'Arm64') {
        Write-Host "  Note: ARM64 detected. Using x86_64 binary (runs via emulation)." -ForegroundColor Yellow
    }
} catch {
    if ($env:PROCESSOR_ARCHITECTURE -eq 'ARM64') {
        Write-Host "  Note: ARM64 detected. Using x86_64 binary (runs via emulation)." -ForegroundColor Yellow
    }
}

# --- Fetch latest release ---

Write-Host "  Fetching latest release..."
try {
    $release = Invoke-RestMethod -Uri $apiUrl -Headers @{ "User-Agent" = "oobo-installer" }
} catch {
    Write-Host "  Error: Could not reach GitHub API. Check your internet connection." -ForegroundColor Red
    exit 1
}
$version = $release.tag_name
Write-Host "  Latest version: $version"

# --- Download ---

$assetName = "oobo-$version-$arch-pc-windows-msvc.zip"
$downloadUrl = "https://github.com/$repo/releases/download/$version/$assetName"
$installDir = "$env:USERPROFILE\.oobo\bin"

New-Item -ItemType Directory -Force -Path $installDir | Out-Null

$tmp = "$env:TEMP\oobo-$version.zip"
Write-Host "  Downloading $assetName..."
try {
    Invoke-WebRequest -Uri $downloadUrl -OutFile $tmp -UseBasicParsing
} catch {
    Write-Host "  Error: Download failed. Asset may not exist for this version." -ForegroundColor Red
    exit 1
}

# --- Extract ---

Write-Host "  Extracting to $installDir..."
Expand-Archive -Path $tmp -DestinationPath $installDir -Force
Remove-Item $tmp -Force

if (-not (Test-Path "$installDir\oobo.exe")) {
    Write-Host "  Error: oobo.exe not found after extraction." -ForegroundColor Red
    exit 1
}

# --- Update PATH (insert before any Git entry so oobo alias works) ---

$userPath = [Environment]::GetEnvironmentVariable("PATH", "User")
if ($userPath -notlike "*$installDir*") {
    $pathEntries = $userPath -split ';' | Where-Object { $_ -ne '' }

    $insertIndex = $pathEntries.Count
    for ($i = 0; $i -lt $pathEntries.Count; $i++) {
        if ($pathEntries[$i] -match '(?i)\\git\\') {
            $insertIndex = $i
            break
        }
    }

    $newEntries = [System.Collections.ArrayList]::new($pathEntries)
    $newEntries.Insert($insertIndex, $installDir)
    $newPath = $newEntries -join ';'

    [Environment]::SetEnvironmentVariable("PATH", $newPath, "User")
    Write-Host "  Added $installDir to PATH (before Git)." -ForegroundColor Green
    Write-Host "  Restart your terminal for PATH changes to take effect."
}

# --- Patch Git Bash ~/.bashrc if Git Bash is installed ---

$gitBashRc = "$env:USERPROFILE\.bashrc"
$pathExport = "export PATH=`"`$HOME/.oobo/bin:`$PATH`""
if (Test-Path "$env:ProgramFiles\Git\bin\bash.exe") {
    if (-not (Test-Path $gitBashRc)) {
        New-Item -ItemType File -Path $gitBashRc -Force | Out-Null
    }
    $content = Get-Content $gitBashRc -Raw -ErrorAction SilentlyContinue
    if (-not $content -or $content -notlike "*/.oobo/bin*") {
        $nl = [Environment]::NewLine
        Add-Content -Path $gitBashRc -Value "${nl}# oobo${nl}${pathExport}" -Encoding UTF8
        Write-Host "  Patched ~/.bashrc for Git Bash." -ForegroundColor Green
    }
}

# --- Done ---

Write-Host ""
Write-Host "  oobo $version installed successfully!" -ForegroundColor Green
Write-Host ""
Write-Host "  Run: oobo --help"
Write-Host ""
