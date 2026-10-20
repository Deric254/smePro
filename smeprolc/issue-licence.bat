@echo off
setlocal EnableExtensions
title SME Pro - issue a licence

rem ===== Works from any folder on any computer: nothing here needs editing. =====
rem The tools are found next to this file (the licence kit made by make_kit.py) or, inside the
rem project, in ..\licensing. Keep your private owner key next to this file as smp-owner.key,
rem or change the KEY line to where you keep it (outside any project folder).
set "TOOLS=%~dp0"
if not exist "%TOOLS%issue_licence.py" set "TOOLS=%~dp0..\licensing\"
set "KEY=%~dp0smp-owner.key"
set "OUT=%USERPROFILE%\SMEPro-licences"
rem ===============================================================================

set "LOG=%OUT%\licences-issued.csv"
set "LAST=%OUT%\latest-licence.txt"

set "PY="
where python >nul 2>&1 && set "PY=python"
if not defined PY where py >nul 2>&1 && set "PY=py"
if not defined PY goto :nopython
if not exist "%TOOLS%issue_licence.py" goto :norepo
if not exist "%KEY%" goto :nokey
if not exist "%OUT%" mkdir "%OUT%"
if not exist "%OUT%" goto :noout

:again
echo.
echo ----- New licence (press Enter on an empty code to quit) -----
set "CODE="
set "CNAME="
set "DAYS="
set /p "CODE=Activation code (like ABCD-EFGH-JKLM): "
if not defined CODE goto :done
set "CODE=%CODE:"=%"
set /p "CNAME=Customer name: "
set "CNAME=%CNAME:"=%"
if not defined CNAME goto :needname
set /p "DAYS=Days [30]: "
if not defined DAYS set "DAYS=30"
set "DAYS=%DAYS:"=%"

set "RAW=%TEMP%\smp-licence-%RANDOM%%RANDOM%.txt"
"%PY%" "%TOOLS%issue_licence.py" --code "%CODE%" --user "%CNAME%" --days "%DAYS%" --key-file "%KEY%" --log "%LOG%" > "%RAW%" 2>&1
if errorlevel 1 goto :notissued

powershell -NoProfile -Command "$t = Get-Content -LiteralPath $env:RAW | Where-Object { $_ -like 'SMP1.*' } | Select-Object -First 1; if (-not $t) { exit 1 }; Set-Content -LiteralPath $env:LAST -Value $t -Encoding ascii; Set-Clipboard -Value $t"
if errorlevel 1 goto :notoken
del "%RAW%" >nul 2>&1

rem Prove the licence is valid for that code before it goes to the customer.
set "TOK="
set /p "TOK=" < "%LAST%"
"%PY%" "%TOOLS%verify_licence.py" --token "%TOK%" --code "%CODE%" > "%RAW%" 2>&1
if errorlevel 1 goto :notverified
del "%RAW%" >nul 2>&1

echo.
echo DONE. The licence for %CNAME% (%CODE%), %DAYS% days, is copied to your clipboard.
echo Paste it into WhatsApp or email. It is also saved in:
echo   %LAST%
echo Record of all licences issued:
echo   %LOG%
goto :again

:needname
echo A customer name is required.
goto :again

:notissued
echo.
type "%RAW%"
del "%RAW%" >nul 2>&1
echo.
echo NOT ISSUED. Fix the problem above and try again.
goto :again

:notoken
echo.
echo The licence was made but could not be copied. See the file:
echo   %RAW%
pause
goto :again

:notverified
echo.
type "%RAW%"
del "%RAW%" >nul 2>&1
echo.
echo WARNING: the licence did not verify. Do NOT send it. Check the code and try again.
goto :again

:nopython
echo Python was not found. Install Python 3 from python.org (tick "Add python.exe to PATH"), then run this again.
pause
exit /b 1

:norepo
echo Cannot find the licence tools (issue_licence.py) next to this file or in ..\licensing.
echo Make the licence kit with: python licensing\make_kit.py  and copy that whole folder.
pause
exit /b 1

:nokey
echo Cannot find your private owner key at: %KEY%
echo Put smp-owner.key next to this file, or edit the KEY line at the top of this file.
pause
exit /b 1

:noout
echo Cannot create the folder: %OUT%
echo Edit the OUT line at the top of this file.
pause
exit /b 1

:done
endlocal
exit /b 0
