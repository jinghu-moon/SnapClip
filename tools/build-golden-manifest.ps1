param(
    [string]$Prototype = "prototypes/ocr-location-test.html",
    [string]$ImageDirectory = "OCR-test-image",
    [string]$Output = "OCR-test-image/golden-manifest.json"
)

$ErrorActionPreference = "Stop"
$html = Get-Content -Raw -LiteralPath $Prototype
$sections = [regex]::Matches($html, '<section class="canvas[^>]*>([\s\S]*?)</section>')
$images = @(Get-ChildItem -LiteralPath $ImageDirectory -File | Where-Object { $_.Extension -match '^\.png$' } | Sort-Object Name)
if ($sections.Count -ne $images.Count) {
    throw "prototype sections ($($sections.Count)) do not match PNG fixtures ($($images.Count))"
}

$manifest = for ($i = 0; $i -lt $sections.Count; $i++) {
    $body = $sections[$i].Groups[1].Value
    $body = [regex]::Replace($body, '<span class="region-label">[\s\S]*?</span>', ' ')
    $body = [regex]::Replace($body, '<script[\s\S]*?</script>', ' ')
    $body = [regex]::Replace($body, '<style[\s\S]*?</style>', ' ')
    $body = [regex]::Replace($body, '<[^>]+>', ' ')
    $body = [System.Net.WebUtility]::HtmlDecode($body)
    $body = [regex]::Replace($body, '\s+', ' ').Trim()
    $fileStem = [System.IO.Path]::GetFileNameWithoutExtension($images[$i].Name)
    $dataFile = [regex]::Match($sections[$i].Value, 'data-file="([^"]+)"').Groups[1].Value
    [ordered]@{
        image = $images[$i].Name
        source = "prototypes/ocr-location-test.html[data-file=$dataFile]"
        annotation_scope = "source-dom-visible-text"
        text = $body
        boxes = @()
    }
}

$parent = Split-Path -Parent $Output
if ($parent) { New-Item -ItemType Directory -Force -Path $parent | Out-Null }
$manifest | ConvertTo-Json -Depth 5 | Set-Content -Encoding utf8 -LiteralPath $Output
Write-Output "Wrote $($manifest.Count) cases to $Output"
