# CI(윈도우): 앱을 --self-test 로 띄워 샘플 문서를 그리게 하고, 결과 JSON 과 화면 캡처를 selftest-windows\ 에 남긴다.
# 문서 하나짜리 창, 같은 샘플을 세 칸에 나란히 띄운 창, 칸 닫기·끌어다 놓기·새 창으로 떼기 시나리오를 차례로 점검한다.
#   ./scripts/ci-selftest.ps1 -Bin <md-viewer.exe>
param([string]$Bin, [string]$Label = 'windows')

$out = "selftest-$Label"
New-Item -ItemType Directory -Force $out | Out-Null
Add-Type -AssemblyName System.Windows.Forms, System.Drawing

function Run-SelfTest([int]$Panes, [string]$Name, [string[]]$Extra) {
  $report = Join-Path (Resolve-Path $out) "$Name.json"
  $argList = @('--self-test', 'samples/sample.md', '--report', $report, '--hold', '6') + $Extra
  $proc = Start-Process -FilePath $Bin -ArgumentList $argList -PassThru
  for ($i = 0; $i -lt 120; $i++) {
    if ((Test-Path $report) -and (Get-Item $report).Length -gt 0) { break }
    Start-Sleep -Seconds 1
  }
  Start-Sleep -Seconds 2

  $b = [System.Windows.Forms.Screen]::PrimaryScreen.Bounds
  $bmp = New-Object System.Drawing.Bitmap $b.Width, $b.Height
  $g = [System.Drawing.Graphics]::FromImage($bmp)
  $g.CopyFromScreen($b.Location, [System.Drawing.Point]::Empty, $b.Size)
  $bmp.Save((Join-Path $out "$Name.png"))

  $proc.WaitForExit(30000) | Out-Null
  # 함수 안의 출력은 돌려주는 값에 섞이므로 화면으로만 보낸다
  node scripts/check-selftest.mjs $report $Label $Panes | Out-Host
  return $LASTEXITCODE
}

# 시나리오용 두 번째 문서 (같은 창에 같은 파일은 두 번 열리지 않는다)
$second = Join-Path $out 'second.md'
Copy-Item samples/sample.md $second
$single = Run-SelfTest 1 'report' @()
$triple = Run-SelfTest 3 'report-3' @('samples/sample.md', 'samples/sample.md')
$scenario = Run-SelfTest 1 'scenario' @('--scenario', $second)
if ($single -ne 0 -or $triple -ne 0 -or $scenario -ne 0) { exit 1 }
exit 0
