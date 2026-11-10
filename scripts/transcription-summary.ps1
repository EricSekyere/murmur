param([Parameter(Mandatory=$true)][string]$ReportDirectory)
$ErrorActionPreference = 'Stop'

$summaries = @(foreach ($backend in @('parakeet-v3', 'parakeet-v2', 'whisper-small', 'whisper-turbo')) {
    $path = Join-Path $ReportDirectory ($backend + '.json')
    if (-not (Test-Path -LiteralPath $path)) { continue }
    $report = Get-Content -LiteralPath $path -Raw | ConvertFrom-Json
    foreach ($group in ($report.observations | Group-Object condition)) {
        $all = @($group.Group)
        # Numeral formatting and intentional disfluency cleanup need separate review.
        $scored = @($all | Where-Object { $_.id -notmatch '_(number|disfluent)_' })
        $general = @($scored | Where-Object { $_.id -notmatch '_dictionary_(owneli|supabase)$' })
        $words = ($scored | Measure-Object reference_words -Sum).Sum
        $rawErrors = ($scored | Measure-Object raw_word_errors -Sum).Sum
        $correctedErrors = ($scored | Measure-Object corrected_word_errors -Sum).Sum
        $errors = ($scored | Measure-Object word_errors -Sum).Sum
        [pscustomobject]@{
            Model = $backend
            Condition = $group.Name
            Cases = $all.Count
            ScoredCases = $scored.Count
            ReferenceWords = $words
            RawErrors = $rawErrors
            CorrectedErrors = $correctedErrors
            DeliveredErrors = $errors
            RawWER = $(if ($words) { [math]::Round(100 * $rawErrors / $words, 2) } else { $null })
            DeliveredWER = $(if ($words) { [math]::Round(100 * $errors / $words, 2) } else { $null })
            GeneralCases = $general.Count
            GeneralWords = ($general | Measure-Object reference_words -Sum).Sum
            GeneralRawErrors = ($general | Measure-Object raw_word_errors -Sum).Sum
            GeneralDeliveredErrors = ($general | Measure-Object word_errors -Sum).Sum
            RejectedCases = @($all | Where-Object { $_.rejected.Count -gt 0 }).Count
            NonSpeechOutputs = @($all | Where-Object { $_.condition -eq 'non_speech' -and $_.actual }).Count
        }
    }
})
if (-not $summaries.Count) { throw 'No supported transcription reports found.' }
ConvertTo-Json -InputObject $summaries -Depth 4 |
    Set-Content -LiteralPath (Join-Path $ReportDirectory 'summary.json') -Encoding UTF8
$summaries | Format-Table Model,Condition,Cases,RawErrors,CorrectedErrors,DeliveredErrors,DeliveredWER,RejectedCases -AutoSize
