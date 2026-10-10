$ErrorActionPreference = 'Stop'
Add-Type -AssemblyName System.Speech

$out = Join-Path $PSScriptRoot '..\tests\fixtures'
New-Item -ItemType Directory -Force $out | Out-Null

$fixtures = @(
    @{ Name = 'en'; Voice = 'Microsoft Zira Desktop'; Text = 'The quick brown fox jumps over the lazy dog. Please send the report by Friday.' },
    @{ Name = 'pt'; Voice = 'Microsoft Maria Desktop'; Text = 'Olá, tudo bem? Amanhã vou ao médico às três horas da tarde.' }
)

$format = New-Object System.Speech.AudioFormat.SpeechAudioFormatInfo(16000, [System.Speech.AudioFormat.AudioBitsPerSample]::Sixteen, [System.Speech.AudioFormat.AudioChannel]::Mono)

foreach ($f in $fixtures) {
    $synth = New-Object System.Speech.Synthesis.SpeechSynthesizer
    $synth.SelectVoice($f.Voice)
    $wav = Join-Path $out "$($f.Name).wav"
    $synth.SetOutputToWaveFile($wav, $format)
    $synth.Speak($f.Text)
    $synth.Dispose()
    Set-Content -Path (Join-Path $out "$($f.Name).txt") -Value $f.Text -Encoding utf8NoBOM
    Write-Output "wrote $wav"
}
