@echo off
title GitHub Repo Backup - skystars8
color 0A

set USER=skystars8
set OUTDIR=C:\gitbackups\%date:~-4,4%-%date:~-7,2%-%date:~-10,2%_%time:~0,2%%time:~3,2%
set OUTDIR=%OUTDIR: =0%

echo.
echo ==================================================
echo   Downloading all public repos of %USER%
echo   Saving to: %OUTDIR%
echo ==================================================
echo.

mkdir "%OUTDIR%" 2>nul
if not exist "%OUTDIR%" (
    echo ERROR: Cannot create folder C:\gitbackups
    echo Make sure you have permission to write to C:\
    pause
    exit /b
)

echo Downloading... this may take a minute...
echo.

curl -L -o "%OUTDIR%\acsim.zip"                "https://github.com/%USER%/acsim/archive/refs/heads/master.zip"
curl -L -o "%OUTDIR%\agents.zip"                "https://github.com/%USER%/agents/archive/refs/heads/main.zip"
curl -L -o "%OUTDIR%\ancient-zig-stuff.zip"     "https://github.com/%USER%/ancient-zig-stuff/archive/refs/heads/main.zip"
curl -L -o "%OUTDIR%\HDBoard.zip"               "https://github.com/%USER%/HDBoard/archive/refs/heads/main.zip"
curl -L -o "%OUTDIR%\imageboards.zip"           "https://github.com/%USER%/imageboards/archive/refs/heads/main.zip"
curl -L -o "%OUTDIR%\ironlock.zip"              "https://github.com/%USER%/ironlock/archive/refs/heads/main.zip"
curl -L -o "%OUTDIR%\misc.zip"                  "https://github.com/%USER%/misc/archive/refs/heads/main.zip"
curl -L -o "%OUTDIR%\newcrypt.zip"              "https://github.com/%USER%/newcrypt/archive/refs/heads/main.zip"
curl -L -o "%OUTDIR%\PQ-File-Encryption.zip"    "https://github.com/%USER%/PQ-File-Encryption/archive/refs/heads/main.zip"
curl -L -o "%OUTDIR%\rage.zip"                  "https://github.com/%USER%/rage/archive/refs/heads/main.zip"
curl -L -o "%OUTDIR%\RustChan.zip"              "https://github.com/%USER%/RustChan/archive/refs/heads/main.zip"
curl -L -o "%OUTDIR%\skystars8.zip"             "https://github.com/%USER%/skystars8/archive/refs/heads/main.zip"
curl -L -o "%OUTDIR%\stardust.zip"              "https://github.com/%USER%/stardust/archive/refs/heads/main.zip"
curl -L -o "%OUTDIR%\stylesheets.zip"           "https://github.com/%USER%/stylesheets/archive/refs/heads/main.zip"
curl -L -o "%OUTDIR%\vichan.zip"                "https://github.com/%USER%/vichan/archive/refs/heads/master.zip"
curl -L -o "%OUTDIR%\vichan-fixed.zip"          "https://github.com/%USER%/vichan-fixed/archive/refs/heads/main.zip"
curl -L -o "%OUTDIR%\Win11Debloat.zip"          "https://github.com/%USER%/Win11Debloat/archive/refs/heads/master.zip"

echo.
echo ==================================================
echo   DONE!
echo   All ZIP files are here:
echo   %OUTDIR%
echo ==================================================
echo.
pause