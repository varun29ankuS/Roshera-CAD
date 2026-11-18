param([string]$List)
$sp='C:\Users\VARUNS~1\AppData\Local\Temp\claude\C--Users-Varun-Sharma-Roshera-CAD\07c0e886-e5b8-4835-9acc-70ef6fa6aea7\scratchpad'
$c='C:\Users\Varun Sharma\Downloads\standard-manufacturing-co_smc-harness-v1-2-fable-5-2-max_20261113-034752'
$exe='C:\Users\Varun Sharma\Roshera-CAD\roshera-backend\target\release\examples\face_probe.exe'
foreach ($p in $List.Split(",")) {
  $t = Get-Date
  $o = & $exe "$c\$p\output.step" 2>&1 | Select-String "errors; first|^blocking|rror" | Select-Object -First 3
  $s = [int]((Get-Date) - $t).TotalSeconds
  if ($o.Count -ge 1 -and $o[0].Line -like '0 errors*') { $line = "$p sound (${s}s)" }
  else { $line = "$p NOT (${s}s): " + (($o | ForEach-Object { $_.Line.Substring(0,[Math]::Min(150,$_.Line.Length)) }) -join ' | ') }
  $line
  Add-Content -Path "$sp\inv81.txt" -Value $line
}
