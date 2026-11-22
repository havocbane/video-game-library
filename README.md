# Video Game Library

A desktop app for tracking your games, platforms, progress and backlog, browsed as a 3D cover-flow carousel instead of a spreadsheet. Pulls your library from [Backloggery](https://backloggery.com/).

Built with Rust + [Tauri 2](https://tauri.app/) and SQLite. The UI is plain HTML/CSS/JS in `ui/`, with no Node build step.

## Run

```bash
cargo run              # dev build + window
cargo test             # unit tests
cargo test -- --ignored   # also hits the live Backloggery API
```

Release installer (MSI/NSIS) needs the Tauri CLI: `cargo install tauri-cli --locked`, then `cargo tauri build`.

Data lives in `%APPDATA%\com.havoc.videogamelibrary\library.db`.

## Using it

- **← / →**, mouse wheel, drag, or click a side card to browse. **PgUp/PgDn** jump 10, **Home/End** go to the ends.
- **1–5** switch shelves (All, Now Playing, Backlog, Finished, Wishlist). **/** searches, **N** adds a game.
- Edits in the detail panel save automatically. Status and priority changes are written to the game's log, and you can log your own sessions there.

## Backloggery sync

Settings → enter your Backloggery username → **Sync**. Sync is **one-way (Backloggery → here)** and uses the same endpoint the Backloggery site uses to show a public profile (`POST /api/fetch_library.php`). That endpoint is undocumented, so it may change without notice.

- New games are imported. A game is updated only if it changed on Backloggery since the last sync, so local edits to unchanged games are kept. Those games are flagged "Edited locally".
- Progress %, hours played, cover art and the log are local-only and never overwritten.
- Games removed from Backloggery are reported but not deleted.
- Pushing changes back would mean logging in with your Backloggery password, so it isn't implemented.

## Cover art

Add a free [SteamGridDB](https://www.steamgriddb.com/) API key in Settings, then use **Find cover** on a game or **Fetch missing covers**. You can also paste any image URL via **Edit**. Games without art get a generated cover.

## Layout

| Path | What |
| --- | --- |
| `src/main.rs` | Tauri setup and command registration |
| `src/commands.rs` | Commands called from the UI |
| `src/db.rs` | SQLite schema/migrations, queries, sync merge logic (+ tests) |
| `src/backloggery.rs` | Backloggery client and code mappings (+ tests) |
| `src/covers.rs` | SteamGridDB lookup |
| `src/models.rs` | Game, status/priority/ownership enums |
| `ui/` | Carousel UI |
