# CI(윈도우): 앱을 --self-test 로 띄워 샘플 문서를 그리게 하고, 결과 JSON 과 화면 캡처를 selftest-windows\ 에 남긴다.
#   ./scripts/ci-selftest.ps1 -Bin <md-viewer.exe>
param([string]$Bin, [string]$Label = 'windows')

$out = "selftest-$Label"
New-Item -ItemType Directory -Force $out | Out-Null
$report = Join-Path (Resolve-Path $out) 'report.json'

$proc = Start-Process -FilePath $Bin -ArgumentList @('--self-test', 'samples/sample.md', '--report', $report, '--hold', '6') -PassThru
for ($i = 0; $i -lt 120; $i++) {
  if ((Test-Path $report) -and (Get-Item $report).Length -gt 0) { break }
  Start-Sleep -Seconds 1
}
Start-Sleep -Seconds 2

Add-Type -AssemblyName System.Windows.Forms, System.Drawing
$b = [System.Windows.Forms.Screen]::PrimaryScreen.Bounds
$bmp = New-Object System.Drawing.Bitmap $b.Width, $b.Height
$g = [System.Drawing.Graphics]::FromImage($bmp)
$g.CopyFromScreen($b.Location, [System.Drawing.Point]::Empty, $b.Size)
$bmp.Save((Join-Path $out 'screen.png'))

$proc.WaitForExit(30000) | Out-Null
node scripts/check-selftest.mjs $report $Label
exit $LASTEXITCODE
