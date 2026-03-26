# ExoOS - WSL bootstrap (run in elevated PowerShell)
# Usage (admin): powershell -ExecutionPolicy Bypass -File .\docs\special\setup_wsl_windows.ps1

$ErrorActionPreference = 'Stop'

function Test-IsAdmin {
	$id = [Security.Principal.WindowsIdentity]::GetCurrent()
	$principal = New-Object Security.Principal.WindowsPrincipal($id)
	return $principal.IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator)
}

if (-not (Test-IsAdmin)) {
	Write-Host "[INFO] Relaunching with administrator privileges..."
	Start-Process PowerShell -Verb RunAs -ArgumentList @(
		'-ExecutionPolicy','Bypass','-File',('"' + $PSCommandPath + '"')
	)
	exit 0
}

Write-Host "[1/5] Checking CPU virtualization firmware state..."
$cpu = Get-CimInstance Win32_Processor | Select-Object -First 1 Name, VirtualizationFirmwareEnabled, VMMonitorModeExtensions, SecondLevelAddressTranslationExtensions
$cpu | Format-List

if (-not $cpu.VirtualizationFirmwareEnabled) {
	Write-Warning "Virtualization is disabled in BIOS/UEFI. Enable Intel VT-x/AMD-V, save, and reboot before WSL2 can work."
	Write-Host "No further action can complete until BIOS virtualization is enabled."
	exit 1
}

Write-Host "[2/5] Enabling required Windows optional features (no immediate reboot)..."
dism.exe /online /enable-feature /featurename:Microsoft-Windows-Subsystem-Linux /all /norestart | Out-Null
dism.exe /online /enable-feature /featurename:VirtualMachinePlatform /all /norestart | Out-Null

Write-Host "[3/5] Configuring WSL default version..."
wsl --set-default-version 2

Write-Host "[4/5] Installing Ubuntu distro registration..."
wsl --install Ubuntu

Write-Host "[5/5] Bootstrap complete. A reboot is required to finalize changes."
Write-Host "After reboot:"
Write-Host "  1) Launch Ubuntu once to create your Linux user"
Write-Host "  2) Run: bash /mnt/c/Users/xavie/Desktop/Exo-OS/docs/special/setup_exoos_wsl.sh"
