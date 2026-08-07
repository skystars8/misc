@echo off
setlocal
title GitHub Repo Backup - skystars8
color 0A

set "GH_BACKUP_USER=skystars8"
if not defined GH_BACKUP_ROOT set "GH_BACKUP_ROOT=C:\gitbackups"

for /f %%I in ('powershell.exe -NoProfile -Command "Get-Date -Format yyyy-MM-dd_HHmmss"') do set "GH_BACKUP_STAMP=%%I"
set "GH_BACKUP_OUTDIR=%GH_BACKUP_ROOT%\%GH_BACKUP_STAMP%"

echo.
echo ==================================================
echo   Downloading all public repos of %GH_BACKUP_USER%
echo   Saving to: %GH_BACKUP_OUTDIR%
echo ==================================================
echo.

mkdir "%GH_BACKUP_OUTDIR%" 2>nul
if not exist "%GH_BACKUP_OUTDIR%" (
    echo ERROR: Cannot create folder %GH_BACKUP_OUTDIR%
    echo Make sure you have permission to write to %GH_BACKUP_ROOT%
    pause
    exit /b 1
)

echo Finding public repositories on GitHub...
echo.

powershell.exe -NoProfile -ExecutionPolicy Bypass -Command ^
  "$ErrorActionPreference = 'Stop';" ^
  "$userName = $env:GH_BACKUP_USER;" ^
  "$outDir = $env:GH_BACKUP_OUTDIR;" ^
  "$headers = @{ 'Accept' = 'application/vnd.github+json'; 'User-Agent' = 'GitHub-Repo-Backup' };" ^
  "$page = 1;" ^
  "$downloaded = 0;" ^
  "$failed = @();" ^
  "do {" ^
  "  $apiUrl = 'https://api.github.com/users/{0}/repos?type=owner&sort=full_name&per_page=100&page={1}' -f $userName, $page;" ^
  "  $repos = Invoke-RestMethod -Uri $apiUrl -Headers $headers;" ^
  "  foreach ($repo in $repos) {" ^
  "    $zipFile = Join-Path $outDir ($repo.name + '.zip');" ^
  "    $partFile = $zipFile + '.part';" ^
  "    $branchPath = (($repo.default_branch -split '/') | ForEach-Object { [Uri]::EscapeDataString($_) }) -join '/';" ^
  "    $downloadUrl = '{0}/archive/refs/heads/{1}.zip' -f $repo.html_url, $branchPath;" ^
  "    Write-Host ('Downloading {0} [{1}]...' -f $repo.name, $repo.default_branch);" ^
  "    try {" ^
  "      if (Test-Path -LiteralPath $partFile) { Remove-Item -LiteralPath $partFile -Force; }" ^
  "      & curl.exe --fail --location --retry 3 --connect-timeout 20 --silent --show-error --output $partFile $downloadUrl;" ^
  "      if ($LASTEXITCODE -ne 0) { throw ('curl exited with code {0}' -f $LASTEXITCODE); }" ^
  "      Move-Item -LiteralPath $partFile -Destination $zipFile -Force;" ^
  "      $downloaded++;" ^
  "      Write-Host '  OK' -ForegroundColor Green;" ^
  "    } catch {" ^
  "      if (Test-Path -LiteralPath $partFile) { Remove-Item -LiteralPath $partFile -Force; }" ^
  "      $failed += $repo.name;" ^
  "      Write-Warning ('  FAILED: {0}' -f $_.Exception.Message);" ^
  "    }" ^
  "  }" ^
  "  $page++;" ^
  "} while ($repos.Count -eq 100);" ^
  "Write-Host '';" ^
  "Write-Host ('Downloaded {0} repositories.' -f $downloaded);" ^
  "if ($failed.Count -gt 0) {" ^
  "  Write-Error ('Failed repositories: {0}' -f ($failed -join ', '));" ^
  "  exit 1;" ^
  "}"

set "GH_BACKUP_EXIT=%ERRORLEVEL%"

echo.
if not "%GH_BACKUP_EXIT%"=="0" (
    echo ==================================================
    echo   BACKUP FINISHED WITH ERRORS
    echo   Check the messages above.
    echo ==================================================
    echo.
    pause
    exit /b %GH_BACKUP_EXIT%
)

echo ==================================================
echo   DONE!
echo   All ZIP files are here:
echo   %GH_BACKUP_OUTDIR%
echo ==================================================
echo.
pause
exit /b 0
