# ==============================================================================
# Exo-OS Formal Verification Suite (TLA+ / TLC Model Checker)
# ==============================================================================
# Strict enforcement:
# - Hard timeout of 90 seconds per specification.
# - Immediate and permanent cleanup of the "states" directory after each run.
# - Memory bounded to 2GB heap (-Xmx2g).
# ==============================================================================

$ErrorActionPreference = "Continue"

$BaseDir = (Get-Item $PSScriptRoot).Parent.Parent.FullName
Set-Location $BaseDir

$JarPath = Join-Path $BaseDir "verification\tla\tla2tools.jar"
if (-not (Test-Path $JarPath)) {
    Write-Error "tla2tools.jar introuvable a l'emplacement: $JarPath"
    exit 1
}

$Models = @(
    # --- ARCH ---
    @{ Name = "SmpBoot";           Category = "arch";     Cfg = "docs\Exo-OS-TLA+\Proof V1\SmpBoot.toolbox\SmpBoot.cfg";                     Tla = "docs\Exo-OS-TLA+\Proof V1\SmpBoot.toolbox\SmpBoot.tla";                     Extra = @() },
    @{ Name = "ContextSwitch";     Category = "arch";     Cfg = "docs\Exo-OS-TLA+\Proof V1\ContextSwitch.toolbox\ContextSwitch.cfg";         Tla = "docs\Exo-OS-TLA+\Proof V1\ContextSwitch.toolbox\ContextSwitch.tla";         Extra = @() },
    @{ Name = "IrqRouting";        Category = "arch";     Cfg = "docs\Exo-OS-TLA+\Proof V1\IrqRouting.toolbox\IrqRouting.cfg";               Tla = "docs\Exo-OS-TLA+\Proof V1\IrqRouting.toolbox\IrqRouting.tla";               Extra = @() },
    @{ Name = "IrqRoutingStress";  Category = "arch";     Cfg = "docs\Exo-OS-TLA+\Proof V1\IrqRoutingStress.toolbox\IrqRoutingStress.cfg";   Tla = "docs\Exo-OS-TLA+\Proof V1\IrqRoutingStress.toolbox\IrqRoutingStress.tla";   Extra = @() },

    # --- MEMORY ---
    @{ Name = "Memory";            Category = "memory";   Cfg = "docs\Exo-OS-TLA+\Proof V1\Memory.toolbox\Memory.cfg";                       Tla = "docs\Exo-OS-TLA+\Proof V1\Memory.toolbox\Memory.tla";                       Extra = @() },

    # --- IPC ---
    @{ Name = "CapTokens";         Category = "ipc";      Cfg = "docs\Exo-OS-TLA+\Proof V1\CapTokens.toolbox\CapTokens.cfg";                 Tla = "docs\Exo-OS-TLA+\Proof V1\CapTokens.toolbox\CapTokens.tla";                 Extra = @() },
    @{ Name = "IommuQueue";        Category = "ipc";      Cfg = "docs\Exo-OS-TLA+\Proof V1\IommuQueue.toolbox\IommuQueue.cfg";               Tla = "docs\Exo-OS-TLA+\Proof V1\IommuQueue.toolbox\IommuQueue.tla";               Extra = @("-deadlock") },

    # --- SECURITE ---
    @{ Name = "ExoShield";         Category = "securite"; Cfg = "docs\Exo-OS-TLA+\ExoShield.cfg";                                           Tla = "docs\Exo-OS-TLA+\ExoShield.tla";                                           Extra = @() },
    @{ Name = "Adversarial";       Category = "securite"; Cfg = "docs\Exo-OS-TLA+\Proof V1\Adversarial.toolbox\ExoOS_Adversarial.cfg";      Tla = "docs\Exo-OS-TLA+\Proof V1\Adversarial.toolbox\Adversarial.tla";             Extra = @() },

    # --- PROCESS ---
    @{ Name = "ProcessDeath";      Category = "process";  Cfg = "docs\Exo-OS-TLA+\Proof V1\ProcessDeath.toolbox\ProcessDeath.cfg";           Tla = "docs\Exo-OS-TLA+\Proof V1\ProcessDeath.toolbox\ProcessDeath.tla";           Extra = @() },
    @{ Name = "PciDoExit";         Category = "process";  Cfg = "docs\Exo-OS-TLA+\Proof V1\PciDoExit.toolbox\PciDoExit.cfg";                 Tla = "docs\Exo-OS-TLA+\Proof V1\PciDoExit.toolbox\PciDoExit.tla";                 Extra = @() },
    @{ Name = "PhoenixState";      Category = "process";  Cfg = "docs\Exo-OS-TLA+\Proof V1\PhoenixState.toolbox\PhoenixState.cfg";           Tla = "docs\Exo-OS-TLA+\Proof V1\PhoenixState.toolbox\PhoenixState.tla";           Extra = @() },
    @{ Name = "ExoPhoenixHandoff"; Category = "process";  Cfg = "docs\Exo-OS-TLA+\Proof V1\ExoPhoenixHandoff.toolbox\ExoPhoenixHandoff.cfg"; Tla = "docs\Exo-OS-TLA+\Proof V1\ExoPhoenixHandoff.toolbox\ExoPhoenixHandoff.tla"; Extra = @() },

    # --- FS ---
    @{ Name = "ExoFS";             Category = "fs";       Cfg = "docs\Exo-OS-TLA+\Proof V1\ExoFS.toolbox\ExoFS.cfg";                         Tla = "docs\Exo-OS-TLA+\Proof V1\ExoFS.toolbox\ExoFS.tla";                         Extra = @() }
)

Write-Host "==========================================================================" -ForegroundColor Cyan
Write-Host "  Exo-OS TLA+ Formal Verification Suite (Timeout: 90s, RAM: 2GB, Auto-Purge) " -ForegroundColor Cyan
Write-Host "==========================================================================" -ForegroundColor Cyan

$Results = @()
$TotalPassed = 0
$TotalFailed = 0
$TotalTimeout = 0

foreach ($m in $Models) {
    # 1. Nettoyage strict préalable
    Remove-Item -Path "states" -Recurse -Force -ErrorAction SilentlyContinue

    $name = $m.Name
    $cat = $m.Category
    $cfgPath = Join-Path $BaseDir $m.Cfg
    $tlaPath = Join-Path $BaseDir $m.Tla

    Write-Host -NoNewline "[RUNNING] " -ForegroundColor Yellow
    Write-Host ("{0,-18} [{1,-8}] ... " -f $name, $cat) -NoNewline

    $outFile = [System.IO.Path]::GetTempFileName()
    $errFile = [System.IO.Path]::GetTempFileName()

    $argsList = @(
        "-Xmx2g",
        "-XX:+UseParallelGC",
        "-jar", "`"$JarPath`"",
        "-workers", "2",
        "-cleanup",
        "-config", "`"$cfgPath`""
    ) + $m.Extra + @("`"$tlaPath`"")

    $sw = [System.Diagnostics.Stopwatch]::StartNew()
    $psi = New-Object System.Diagnostics.ProcessStartInfo
    $psi.FileName = "java"
    $psi.Arguments = ($argsList -join " ")
    $psi.RedirectStandardOutput = $true
    $psi.RedirectStandardError = $true
    $psi.UseShellExecute = $false
    $psi.CreateNoWindow = $true

    $proc = [System.Diagnostics.Process]::Start($psi)

    $timeoutMs = 90000 # 1m30s
    $finished = $proc.WaitForExit($timeoutMs)

    $sw.Stop()
    $elapsedSec = [math]::Round($sw.Elapsed.TotalSeconds, 1)

    $stdout = ""
    $stderr = ""

    if (-not $finished) {
        try {
            $proc.Kill()
        } catch {}
        $status = "TIMEOUT"
        $TotalTimeout++
        Write-Host "TIMEOUT (>90s) [KILLED]" -ForegroundColor Red
    } else {
        $stdout = $proc.StandardOutput.ReadToEnd()
        $stderr = $proc.StandardError.ReadToEnd()

        $isOk = ($stdout -match "Model checking completed\. No error has been found") -or
                ($stdout -match "0 distinct states left on queue") -or
                ($proc.ExitCode -eq 0 -and ($stdout -match "states generated"))

        if ($isOk) {
            $status = "PASS"
            $TotalPassed++

            # Extraire les statistiques
            $genMatch = [regex]::Match($stdout, "([0-9\?]+) states generated")
            $distMatch = [regex]::Match($stdout, "([0-9\?]+) distinct states found")
            $gen = if ($genMatch.Success) { $genMatch.Groups[1].Value } else { "N/A" }
            $dist = if ($distMatch.Success) { $distMatch.Groups[1].Value } else { "N/A" }

            Write-Host ("PASS in {0}s (gen: {1}, distinct: {2})" -f $elapsedSec, $gen, $dist) -ForegroundColor Green
        } else {
            $status = "FAIL"
            $TotalFailed++
            Write-Host ("FAIL in {0}s (exit {1})" -f $elapsedSec, $proc.ExitCode) -ForegroundColor Red
            if ($stdout -match "Error:.*") {
                Write-Host "  -> $($matches[0])" -ForegroundColor Magenta
            }
        }
    }

    # 2. Nettoyage strict et définitif immédiat du dossier "states" (dans le cwd et dans le dossier du .tla)
    $tlaDir = Split-Path -Parent $tlaPath
    $tlaStates = Join-Path $tlaDir "states"
    if (Test-Path $tlaStates) {
        Remove-Item -Path $tlaStates -Recurse -Force -ErrorAction SilentlyContinue
    }
    Remove-Item -Path "states" -Recurse -Force -ErrorAction SilentlyContinue
    Remove-Item -Path $outFile -Force -ErrorAction SilentlyContinue
    Remove-Item -Path $errFile -Force -ErrorAction SilentlyContinue

    $Results += [PSCustomObject]@{
        Module   = $name
        Organ    = $cat
        Status   = $status
        Duration = "$($elapsedSec)s"
    }
}

# Nettoyage final
Remove-Item -Path "states" -Recurse -Force -ErrorAction SilentlyContinue

Write-Host "`n==========================================================================" -ForegroundColor Cyan
Write-Host "                      RÉSUMÉ DES VÉRIFICATIONS FORMELLES" -ForegroundColor Cyan
Write-Host "==========================================================================" -ForegroundColor Cyan
$Results | Format-Table -AutoSize

Write-Host ("Total : {0} modules | Réussis : {1} | Échoués : {2} | Timeout : {3}" -f $Models.Count, $TotalPassed, $TotalFailed, $TotalTimeout) -ForegroundColor $(if ($TotalFailed -eq 0 -and $TotalTimeout -eq 0) { "Green" } else { "Red" })

if ($TotalFailed -gt 0 -or $TotalTimeout -gt 0) {
    exit 1
} else {
    exit 0
}
