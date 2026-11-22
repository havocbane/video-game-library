//! GOG Galaxy 2.0's local database. Besides GOG itself, Galaxy aggregates every
//! store connected through its integrations (Epic, Ubisoft, EA, Battle.net,
//! Xbox, PlayStation, Humble, Amazon, ...), so this one source covers many.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use rusqlite::{Connection, OpenFlags};
use serde_json::Value;

use super::{BoxFuture, FieldKind, Source, SourceContext, SourceField, SourceGame, SourceInfo};
use crate::error::{Error, Result};

pub struct GogGalaxy;

const DEFAULT_DB: &str = r"C:\ProgramData\GOG.com\Galaxy\storage\galaxy-2.0.db";

impl Source for GogGalaxy {
    fn info(&self) -> SourceInfo {
        SourceInfo {
            id: "gog-galaxy".into(),
            name: "GOG Galaxy 2.0".into(),
            description: "Your GOG library plus every store connected in Galaxy (Epic, Ubisoft, EA, Xbox, \
                PlayStation, Humble, Amazon…). Reads Galaxy's local database; no login needed."
                .into(),
            fields: vec![SourceField::new("dbPath", "Galaxy database", FieldKind::Path).placeholder(DEFAULT_DB)],
            origin: "built-in".into(),
        }
    }

    fn detect(&self) -> HashMap<String, String> {
        if Path::new(DEFAULT_DB).exists() {
            HashMap::from([("dbPath".to_string(), DEFAULT_DB.to_string())])
        } else {
            HashMap::new()
        }
    }

    fn fetch<'a>(&'a self, ctx: &'a SourceContext) -> BoxFuture<'a, Result<Vec<SourceGame>>> {
        let path = PathBuf::from(ctx.get("dbPath").unwrap_or(DEFAULT_DB));
        Box::pin(async move {
            tauri::async_runtime::spawn_blocking(move || read_library(&path))
                .await
                .map_err(|e| Error::msg(format!("Galaxy import task failed: {e}")))?
        })
    }
}

fn read_library(db_path: &Path) -> Result<Vec<SourceGame>> {
    if !db_path.exists() {
        return Err(Error::msg(format!("GOG Galaxy database not found at {}.", db_path.display())));
    }
    // Galaxy keeps the database open; read a snapshot copy (with its WAL) instead.
    let snapshot = std::env::temp_dir().join(format!("vgl-galaxy-{}", std::process::id()));
    std::fs::create_dir_all(&snapshot).map_err(io_err)?;
    let copy = snapshot.join("galaxy-2.0.db");
    for suffix in ["", "-wal", "-shm"] {
        let from = PathBuf::from(format!("{}{suffix}", db_path.display()));
        if from.exists() {
            std::fs::copy(&from, format!("{}{suffix}", copy.display())).map_err(io_err)?;
        }
    }
    let result = query(&copy);
    let _ = std::fs::remove_dir_all(&snapshot);
    result
}

fn query(path: &Path) -> Result<Vec<SourceGame>> {
    let conn = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_WRITE)?;
    let piece = |ty: &str| {
        format!(
            "(SELECT p.value FROM GamePieces p JOIN GamePieceTypes t ON t.id = p.gamePieceTypeId
              WHERE p.releaseKey = l.releaseKey AND t.type = '{ty}' AND p.value IS NOT NULL LIMIT 1)"
        )
    };
    let sql = format!(
        "SELECT l.releaseKey, {title}, {original_title}, {images},
            (SELECT SUM(minutesInGame) FROM GameTimes g WHERE g.releaseKey = l.releaseKey),
            (SELECT MAX(lastPlayedDate) FROM LastPlayedDates d WHERE d.gameReleaseKey = l.releaseKey)
         FROM (SELECT DISTINCT releaseKey FROM LibraryReleases) l
         LEFT JOIN ReleaseProperties r ON r.releaseKey = l.releaseKey
         WHERE COALESCE(r.isDlc, 0) = 0 AND COALESCE(r.isVisibleInLibrary, 1) = 1",
        title = piece("title"),
        original_title = piece("originalTitle"),
        images = piece("originalImages"),
    );

    let mut stmt = conn.prepare(&sql)?;
    let rows = stmt.query_map([], |r| {
        Ok((
            r.get::<_, String>(0)?,
            r.get::<_, Option<String>>(1)?,
            r.get::<_, Option<String>>(2)?,
            r.get::<_, Option<String>>(3)?,
            r.get::<_, Option<i64>>(4)?,
            r.get::<_, Option<String>>(5)?,
        ))
    })?;

    let mut games = Vec::new();
    for row in rows {
        let (key, title, original_title, images, minutes, last_played) = row?;
        let Some((store, platform)) = store_for(&key) else { continue };
        let Some(title) = json_str(title.as_deref(), "title").or_else(|| json_str(original_title.as_deref(), "title"))
        else {
            continue;
        };
        games.push(SourceGame {
            external_id: key,
            title,
            platform: platform.into(),
            store: Some(store.into()),
            playtime_minutes: minutes.filter(|m| *m > 0),
            last_played,
            cover_url: json_str(images.as_deref(), "verticalCover"),
            ownership: None,
            format: None,
            status: None,
        });
    }
    Ok(games)
}

/// Maps a Galaxy release-key prefix to (store, platform). `None` skips it.
fn store_for(release_key: &str) -> Option<(&'static str, &'static str)> {
    let prefix = release_key.split_once('_').map_or(release_key, |(p, _)| p);
    Some(match prefix {
        "gog" => ("GOG", "PC"),
        "steam" => ("Steam", "PC"),
        "epic" => ("Epic", "PC"),
        "uplay" => ("Ubisoft Connect", "PC"),
        "origin" | "eaApp" => ("EA app", "PC"),
        "battlenet" => ("Battle.net", "PC"),
        "bethesda" => ("Bethesda.net", "PC"),
        "paradox" => ("Paradox", "PC"),
        "humble" => ("Humble", "PC"),
        "itch" => ("itch.io", "PC"),
        "amazon" => ("Amazon Games", "PC"),
        "rockstar" => ("Rockstar", "PC"),
        "winstore" => ("Microsoft Store", "PC"),
        "riot" => ("Riot", "PC"),
        "xboxone" => ("Xbox", "Xbox"),
        // PlayStation title ids tell the generation: PPSA… is PS5, CUSA… is PS4.
        "psn" if release_key.contains("_PPSA") => ("PlayStation", "PlayStation 5"),
        "psn" if release_key.contains("_CUSA") => ("PlayStation", "PlayStation 4"),
        "psn" => ("PlayStation", "PlayStation"),
        "nswitch" => ("Nintendo", "Nintendo Switch"),
        // Galaxy's own placeholders and manually added entries.
        "generic" | "test" => return None,
        _ => ("Other", "PC"),
    })
}

fn json_str(value: Option<&str>, key: &str) -> Option<String> {
    let v: Value = serde_json::from_str(value?).ok()?;
    v.get(key)?.as_str().map(str::trim).filter(|s| !s.is_empty()).map(str::to_string)
}

fn io_err(e: std::io::Error) -> Error {
    Error::msg(format!("Couldn't copy the Galaxy database: {e}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn release_key_platforms() {
        assert_eq!(store_for("psn_PPSA03184_00"), Some(("PlayStation", "PlayStation 5")));
        assert_eq!(store_for("psn_CUSA00552_00"), Some(("PlayStation", "PlayStation 4")));
        assert_eq!(store_for("steam_986310"), Some(("Steam", "PC")));
        assert_eq!(store_for("generic_51154159937556949"), None);
    }

    /// Reads this machine's Galaxy install: `cargo test -- --ignored local_galaxy --nocapture`
    #[test]
    #[ignore]
    fn local_galaxy() {
        let games = read_library(Path::new(DEFAULT_DB)).unwrap();
        let with_cover = games.iter().filter(|g| g.cover_url.is_some()).count();
        let played = games.iter().filter(|g| g.playtime_minutes.is_some()).count();
        println!("galaxy: {} games, {with_cover} with covers, {played} with playtime", games.len());
        for g in games.iter().take(3) {
            println!("  {:?} {:?} {:?} {:?}", g.title, g.store, g.playtime_minutes, g.last_played);
        }
        assert!(!games.is_empty());
    }
}
