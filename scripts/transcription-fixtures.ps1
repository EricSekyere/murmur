param([string]$OutputDirectory = 'target/transcription-stress')
$ErrorActionPreference = 'Stop'
Add-Type -AssemblyName System.Speech
$directory = [System.IO.Path]::GetFullPath((Join-Path (Get-Location) $OutputDirectory))
New-Item -ItemType Directory -Path $directory -Force | Out-Null
$phrases = @(
    @{ id='prose'; expected='Please update the settings and restart the application.' },
    @{ id='negation'; expected='Do not delete the backup until the migration is complete.' },
    @{ id='technical'; expected='Use async await with TypeScript and install the package with npm.' },
    @{ id='names'; expected='Ask Claude to update the Fresha integration.' },
    @{ id='camel'; expected='Camel case get user profile'; identifier='getUserProfile' },
    @{ id='snake'; expected='Snake case user account settings'; identifier='user_account_settings' },
    @{ id='constant'; expected='Constant max retry count'; identifier='MAX_RETRY_COUNT' },
    @{ id='pascal'; expected='Pascal case user profile card'; identifier='UserProfileCard' },
    @{ id='thanks'; expected='Thank you.' },
    @{ id='okay'; expected='Okay.' }
)
$manifest = @()
$voice = New-Object System.Speech.Synthesis.SpeechSynthesizer
try {
    $format = New-Object System.Speech.AudioFormat.SpeechAudioFormatInfo(16000, [System.Speech.AudioFormat.AudioBitsPerSample]::Sixteen, [System.Speech.AudioFormat.AudioChannel]::Mono)
    foreach ($installed in $voice.GetInstalledVoices()) {
        $name = $installed.VoiceInfo.Name
        $voice.SelectVoice($name)
        $prefix = $name.Replace(' ', '_')
        foreach ($phrase in $phrases) {
            $id = $prefix + '_' + $phrase.id
            $file = $id + '.wav'
            $voice.SetOutputToWaveFile((Join-Path $directory $file), $format)
            $voice.Speak($phrase.expected)
            $voice.SetOutputToNull()
            $entry = @{ id=$id; file=$file; expected=$phrase.expected }
            if ($phrase.ContainsKey('identifier')) { $entry.identifier=$phrase.identifier }
            $manifest += $entry
        }
    }
} finally {
    $voice.Dispose()
}
$json = ConvertTo-Json -InputObject $manifest -Depth 4
[System.IO.File]::WriteAllText((Join-Path $directory 'manifest.json'), $json)
Write-Output ('Created ' + $manifest.Count + ' local synthetic speech fixtures in ' + $directory)
