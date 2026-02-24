Set-Location "c:\Users\GALAXY BOOK FLEX\Documents\Exo-OS\kernel\src\process"
$files = Get-ChildItem -Recurse -Filter "*.rs" | Select-Object -ExpandProperty FullName
$results = @()
foreach ($f in $files) {
    $content = Get-Content $f
    for ($i = 0; $i -lt $content.Count; $i++) {
        if ($content[$i] -match 'unsafe\s*\{') {
            $found = $false
            $start = [Math]::Max(0, $i - 4)
            for ($j = $start; $j -lt $i; $j++) {
                if ($content[$j] -match 'SAFETY') { $found = $true; break }
            }
            if (-not $found) {
                $rel = $f.Replace('c:\Users\GALAXY BOOK FLEX\Documents\Exo-OS\kernel\src\process\', '')
                $results += "${rel}:$($i+1): $($content[$i].Trim())"
            }
        }
    }
}
if ($results.Count -eq 0) {
    Write-Host "OK: tous les unsafe ont un commentaire SAFETY dans les 4 lignes precedentes"
} else {
    $results | ForEach-Object { Write-Host $_ }
}
