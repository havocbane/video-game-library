# Video Game Library

A desktop app for tracking your games, platforms, progress and backlog, browsed as a 3D cover-flow carousel instead of a spreadsheet. Imports your store libraries through a plugin system and pulls your [Backloggery](https://backloggery.com/) list. Physical and other hand-added games live alongside them.

Built with Rust + [Tauri 2](https://tauri.app/) and SQLite. The UI is plain HTML/CSS/JS in `ui/`, with no Node build step.

## Run

```bash
cargo run              # dev build + window
cargo test             # unit tests
cargo test -- --ignored   # also hits live Backloggery and reads this PC's Galaxy/Epic installs
```

Release installer (MSI/NSIS) needs the Tauri CLI: `cargo install tauri-cli --locked`, then `cargo tauri build`.

Data lives in `%APPDATA%\com.havoc.videogamelibrary\` (`library.db`, `covers\` and `plugins\`). Set `VGL_DATA_DIR` to use a different folder, for example a test library.

## Using it

- **← / →**, mouse wheel, drag, or click a side card to browse. **PgUp/PgDn** jump 10, **Home/End** go to the ends.
- **1–5** switch shelves (All, Now Playing, Backlog, Finished, Wishlist). **/** searches, **N** adds a game.
- Edits in the detail panel save automatically. Status and priority changes are written to the game's log, and you can log your own sessions there.

## Importing store libraries

Click **⇩ Import** and pick a source. Each source keeps its own settings, and detected values (install paths, your Steam ID) are filled in for you.

| Source | What it imports | Needs |
| --- | --- | --- |
| Steam | Every owned game, playtime, last played, box art | Free [Web API key](https://steamcommunity.com/dev/apikey); profile "Game details" set to Public |
| GOG Galaxy 2.0 | GOG plus **every store connected in Galaxy**: Epic, Ubisoft, EA, Battle.net, Xbox, PlayStation, Humble, Amazon… | Galaxy installed (reads its local database) |
| Epic Games | Your whole Epic library, installed or not, with box art | Epic Games Launcher (reads its local catalog cache; open its Library once so the cache is current) |
| PlayStation | PS4/PS5 games you've played, with playtime | NPSSO token from your signed-in browser (unofficial API) |
| Xbox app / Microsoft Store (PC) | Installed PC/Game Pass games with Store box art | Nothing. This is a bundled **external plugin** (PowerShell) |
| CSV file | Anything else: Nintendo purchases, a physical shelf, Humble/itch exports | A CSV with a `title` column. Optional: platform, store, status, ownership, format, hours, last played |

How imports merge:

- A store copy links to an existing game with the same title on the same platform family. Case, ™/®, punctuation and edition suffixes like "GOTY" or "Complete Edition" are ignored when comparing. So Hades from Steam, GOG and your hand-added physical copy all become one entry on PC, and the detail panel lists each store under **In your stores**.
- Imports only **raise** hours played, fill in a **missing** cover and move last played forward. Status, priority, rating, notes, format and ownership stay as you set them. New games start as Unplayed, or Unfinished with 30+ minutes played.
- Deleting an imported game remembers its store ids, so re-imports don't bring it back.
- Backloggery sync also links to games you already have, instead of duplicating them.
- Platforms group by machine: PC/Windows/Steam/GOG/Epic are all "PC", and PS4, PS5, Xbox One, Xbox Series and Switch are each their own group. Galaxy sometimes reports a console only as "PlayStation" or "Xbox"; those match whichever generation you already have.
- The same game on **different platforms** stays separate on purpose (its own status, progress and log). The detail panel shows **Also on** chips that jump to the other copies.

### Fixing matches by hand

- **Merge…** (detail panel): folds another entry into this one. Its store links, log and Backloggery link move over, and it fills in anything this game is missing. Use it for titles that differ too much to match automatically, like "GTA V" vs "Grand Theft Auto V".
- **Split off** (hover a store under *In your stores*): moves a wrongly matched store copy into its own game, for example DOOM (2016) on Steam that matched your DOOM (1993). Later imports keep it separate.

### Writing a plugin

A plugin is a folder in `plugins\` with a `plugin.json`, plus a program in any language:

```json
{
  "id": "my-store",
  "name": "My Store",
  "description": "What it imports.",
  "command": ["powershell.exe", "-NoProfile", "-File", "import.ps1"],
  "fields": [{ "key": "token", "label": "API token", "kind": "secret", "help": "Where to find it" }]
}
```

The app runs `command` in the plugin folder and writes `{"config": {"token": "..."}}` to its stdin. The program prints a JSON array (or `{"games": [...]}`) to stdout:

```json
[{ "externalId": "123", "title": "Halo Infinite", "platform": "PC", "store": "Xbox",
   "playtimeMinutes": 340, "lastPlayed": "2025-06-01 20:00:00", "coverUrl": "https://…",
   "ownership": "subscription", "format": "Digital", "status": "unfinished" }]
```

Only `externalId` and `title` are required. A non-zero exit shows stderr to the user, and there's a 5-minute timeout. Field kinds are `text`, `secret` and `path`. Full `https://` URLs in a description or `help` become links that open in the user's browser. See [`plugins/xbox-pc`](plugins/xbox-pc) for a complete example. Plugins are rescanned every time the Import dialog opens. Plugins run with your user's permissions, so only install ones you trust.

Built-in sources implement the `Source` trait in [`src/sources/`](src/sources) and register in `builtin_sources()`.

## Physical games

Use **+ Add** or **Edit** with a format like "Physical (Complete In Box)". You can also import a CSV of your shelf. The **All formats** filter switches between digital and physical, and **All stores** narrows to one storefront (Steam, GOG, Epic, Xbox… or games with no store). You can also sort by store. Hand-entered games are never modified by imports, only linked.

## Backloggery sync

Settings → enter your Backloggery username → **Sync**. Sync is **one-way (Backloggery → here)** and uses the same endpoint the Backloggery site uses to show a public profile (`POST /api/fetch_library.php`). That endpoint is undocumented, so it may change without notice.

- New games are imported. A game is updated only if it changed on Backloggery since the last sync, so local edits to unchanged games are kept. Those games are flagged "Edited locally".
- Progress %, hours played, cover art and the log are local-only and never overwritten.
- Games removed from Backloggery are reported but not deleted.
- Pushing changes back would mean logging in with your Backloggery password, so it isn't implemented.

## Cover art

Add a free [SteamGridDB](https://www.steamgriddb.com/) API key in Settings, then use **Find cover** on a game or **Fetch missing covers**. You can also paste any image URL via **Edit**. Games without art get a generated cover.

Covers are saved to a `covers` folder in the data folder the first time they load, and the rest of the library downloads in the background, so art keeps working offline and after a store moves or removes an image. Files no game uses any more are deleted automatically. Delete the folder to re-download everything.

## Layout

| Path | What |
| --- | --- |
| `src/main.rs` | Tauri setup and command registration |
| `src/commands.rs` | Commands called from the UI |
| `src/db.rs` | SQLite schema/migrations, queries, sync merge logic (+ tests) |
| `src/backloggery.rs` | Backloggery client and code mappings (+ tests) |
| `src/covers.rs` | SteamGridDB lookup |
| `src/cover_cache.rs` | Local cover cache served to the UI as `cover://` |
| `src/sources/` | Import plugin system: `Source` trait, matching, built-in sources, external plugin runner |
| `plugins/` | Bundled external plugins (installed into the data folder on first run) |
| `src/models.rs` | Game, status/priority/ownership enums |
| `ui/` | Carousel UI |

## License

Copyright (C) 2026 Joseph Bane

This program is free software: you can redistribute it and/or modify it under the terms of the GNU General Public License as published by the Free Software Foundation, either version 3 of the License, or (at your option) any later version. It is distributed in the hope that it will be useful, but WITHOUT ANY WARRANTY; without even the implied warranty of MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE. See [LICENSE](LICENSE) for the full text.

Third-party libraries keep their own licenses (mostly MIT/Apache-2.0, plus a few MPL-2.0 libraries used by Tauri), all compatible with GPL-3.0.

Video Game Library is not affiliated with or endorsed by Backloggery, Sony Interactive Entertainment, Valve, GOG, Epic Games, Microsoft, or SteamGridDB. It uses some of their services through unofficial, undocumented endpoints that may change or stop working at any time; use them at your own risk and in line with each service's terms.
