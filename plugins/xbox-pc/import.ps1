# Video Game Library plugin: PC games installed through the Xbox app / Microsoft Store.
#
# Protocol: read {"config": {...}} from stdin, print a JSON array of games to stdout.
# Each game: externalId, title, and optionally platform, store, playtimeMinutes,
# lastPlayed, coverUrl, ownership, format, status.

$ErrorActionPreference = 'Stop'
[Console]::OutputEncoding = [System.Text.UTF8Encoding]::new($false)

$config = $null
try {
    $raw = [Console]::In.ReadToEnd()
    if ($raw) { $config = ($raw | ConvertFrom-Json).config }
} catch { }

if ($config -and $config.gamesFolders) {
    $roots = $config.gamesFolders -split ';' | ForEach-Object { $_.Trim() } | Where-Object { $_ }
} else {
    $roots = Get-PSDrive -PSProvider FileSystem | ForEach-Object { Join-Path $_.Root 'XboxGames' }
}

$games = @(foreach ($root in $roots) {
    if (-not (Test-Path -LiteralPath $root)) { continue }
    foreach ($dir in Get-ChildItem -LiteralPath $root -Directory) {
        $configPath = Join-Path $dir.FullName 'Content\MicrosoftGame.config'
        if (-not (Test-Path -LiteralPath $configPath)) { continue }
        try { [xml]$xml = Get-Content -LiteralPath $configPath -Raw } catch { continue }
        $game = $xml.Game
        # Add-ons (armor packs, soundtracks...) have nothing to launch.
        if (-not $game.ExecutableList.Executable) { continue }

        $title = $game.ShellVisuals.DefaultDisplayName
        if (-not $title -or $title -like 'ms-resource:*') { $title = $dir.Name }
        [pscustomobject]@{
            externalId = if ($game.StoreId) { $game.StoreId } else { $game.Identity.Name }
            title      = $title
            platform   = 'PC'
            store      = 'Xbox'
            coverUrl   = $null
            storeId    = $game.StoreId
        }
    }
})

# Box art from the public Microsoft Store catalog, 20 products per request.
$withIds = @($games | Where-Object { $_.storeId })
for ($i = 0; $i -lt $withIds.Count; $i += 20) {
    $batch = $withIds[$i..([Math]::Min($i + 19, $withIds.Count - 1))]
    $ids = ($batch.storeId) -join ','
    try {
        $resp = Invoke-RestMethod -TimeoutSec 20 -Uri "https://displaycatalog.mp.microsoft.com/v7.0/products?bigIds=$ids&market=US&languages=en-us"
        foreach ($product in $resp.Products) {
            $images = $product.LocalizedProperties[0].Images
            $poster = $images | Where-Object { $_.ImagePurpose -eq 'Poster' } | Select-Object -First 1
            if (-not $poster) { $poster = $images | Where-Object { $_.ImagePurpose -eq 'BoxArt' } | Select-Object -First 1 }
            if ($poster) {
                $url = if ($poster.Uri.StartsWith('//')) { "https:$($poster.Uri)" } else { $poster.Uri }
                $batch | Where-Object { $_.storeId -eq $product.ProductId } | ForEach-Object { $_.coverUrl = $url }
            }
        }
    } catch {
        [Console]::Error.WriteLine("Cover lookup failed: $($_.Exception.Message)")
    }
}

$out = @($games | Select-Object externalId, title, platform, store, coverUrl)
ConvertTo-Json -InputObject $out -Depth 3 -Compress
