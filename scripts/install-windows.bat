@echo off
setlocal enabledelayedexpansion

:: ECNU-VPN Windows Install Script
:: Installs exv helper as a Windows Service

set "EXE_NAME=exv.exe"
set "INSTALL_DIR=C:\Program Files\ECNU-VPN"
set "SERVICE_NAME=exv-helper"
set "SERVICE_DISPLAY=ECNU VPN Helper"

:: Check for admin privileges
net session >nul 2>&1
if %errorlevel% neq 0 (
    echo ERROR: This script requires Administrator privileges.
    echo Please run from an elevated Command Prompt.
    exit /b 1
)

:: Locate the executable
set "EXE_PATH=%~dp0%EXE_NAME%"
if not exist "%EXE_PATH%" (
    echo ERROR: %EXE_NAME% not found in %~dp0
    echo Please run this script from the directory containing %EXE_NAME%
    exit /b 1
)

echo === ECNU-VPN Windows Install ===
echo.

:: Create install directory
if not exist "%INSTALL_DIR%" (
    mkdir "%INSTALL_DIR%"
    if %errorlevel% neq 0 (
        echo ERROR: Failed to create %INSTALL_DIR%
        exit /b 1
    )
)

:: Copy executable
echo Installing %EXE_NAME% to %INSTALL_DIR%...
copy /y "%EXE_PATH%" "%INSTALL_DIR%\%EXE_NAME%" >nul
if %errorlevel% neq 0 (
    echo ERROR: Failed to copy executable.
    exit /b 1
)

:: Install the helper service
echo Installing helper service...
"%INSTALL_DIR%\%EXE_NAME%" service install
if %errorlevel% neq 0 (
    echo WARNING: Service install returned error. The service may need manual setup.
    echo You can install manually with:
    echo   sc create %SERVICE_NAME% binPath= "%INSTALL_DIR%\%EXE_NAME% __helper-daemon" start= auto DisplayName= "%SERVICE_DISPLAY%"
)

echo.
echo Installation complete.
echo.
echo The helper service will start automatically on login.
echo Use 'exv service status' to check, or 'exv service uninstall' to remove.
echo.

endlocal
