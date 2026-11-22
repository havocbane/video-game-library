use std::collections::HashSet;
use std::path::Path;
use std::sync::{Mutex, MutexGuard};

use rusqlite::{Connection, OptionalExtension, Row, params};

use crate::error::{Error, Result};
use crate::models::{Game, GameInput, ImportedGame, LogEntry, Settings, Status, SyncReport};

/// Schema migrations, applied in order. `PRAGMA user_version` records how many have run.
const MIGRATIONS: &[&str] = &[r#"
    CREATE TABLE games (
        id                    INTEGER PRIMARY KEY,
        title                 TEXT NOT NULL,
        platform              TEXT NOT NULL DEFAULT '',
        platform_abbr         TEXT,
        status                TEXT NOT NULL DEFAULT 'unplayed',
        priority              TEXT NOT NULL DEFAULT 'normal',
        ownership             TEXT NOT NULL DEFAULT 'owned',
        format                TEXT,
        region                TEXT,
        rating                INTEGER,
        progress              INTEGER NOT NULL DEFAULT 0,
        hours_played          REAL NOT NULL DEFAULT 0,
        notes                 TEXT NOT NULL DEFAULT '',
        cover_url             TEXT,
        achievements_done     INTEGER NOT NULL DEFAULT 0,
        achievements_total    INTEGER NOT NULL DEFAULT 0,
        backloggery_id        INTEGER UNIQUE,
        backloggery_parent_id INTEGER,
        backloggery_updated   TEXT,
        local_changes         INTEGER NOT NULL DEFAULT 0,
        created_at            TEXT NOT NULL DEFAULT (datetime('now')),
        updated_at            TEXT NOT NULL DEFAULT (datetime('now'))
    );
    CREATE INDEX games_status ON games(status);

    CREATE TABLE game_log (
        id      INTEGER PRIMARY KEY,
        game_id INTEGER NOT NULL REFERENCES games(id) ON DELETE CASCADE,
        at      TEXT NOT NULL DEFAULT (datetime('now')),
        note    TEXT NOT NULL
    );
    CREATE INDEX game_log_game ON game_log(game_id, at);

    CREATE TABLE settings (
        key   TEXT PRIMARY KEY,
        value TEXT NOT NULL
    );
"#];

const GAME_COLUMNS: &str = "id, title, platform, platform_abbr, status, priority, ownership, format, \
    region, rating, progress, hours_played, notes, cover_url, achievements_done, achievements_total, \
    backloggery_id, local_changes, created_at, updated_at";

const KEY_BL_USER: &str = "backloggery_username";
const KEY_SGDB: &str = "steamgriddb_key";
const KEY_LAST_SYNC: &str = "backloggery_last_sync";

pub struct Db {
    conn: Mutex<Connection>,
}

impl Db {
    pub fn open(path: &Path) -> Result<Self> {
        let mut conn = Connection::open(path)?;
        conn.pragma_update(None, "journal_mode", "WAL")?;
        conn.pragma_update(None, "foreign_keys", "ON")?;
        migrate(&mut conn)?;
        Ok(Self { conn: Mutex::new(conn) })
    }

    fn conn(&self) -> MutexGuard<'_, Connection> {
        // A panic while holding the lock leaves SQLite itself consistent, so recover.
        self.conn.lock().unwrap_or_else(|e| e.into_inner())
    }

    pub fn list_games(&self) -> Result<Vec<Game>> {
        let conn = self.conn();
        let mut stmt =
            conn.prepare(&format!("SELECT {GAME_COLUMNS} FROM games ORDER BY title COLLATE NOCASE"))?;
        let games = stmt.query_map([], row_to_game)?.collect::<rusqlite::Result<_>>()?;
        Ok(games)
    }

    pub fn create_game(&self, input: GameInput) -> Result<Game> {
        let g = validated(input)?;
        let mut conn = self.conn();
        let tx = conn.transaction()?;
        tx.execute(
            "INSERT INTO games (title, platform, platform_abbr, status, priority, ownership, format,
                region, rating, progress, hours_played, notes, cover_url, achievements_done, achievements_total)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15)",
            params![
                g.title, g.platform, g.platform_abbr, g.status, g.priority, g.ownership, g.format,
                g.region, g.rating, g.progress, g.hours_played, g.notes, g.cover_url,
                g.achievements_done, g.achievements_total
            ],
        )?;
        let id = tx.last_insert_rowid();
        add_log(&tx, id, &format!("Added as {}", g.status.label()))?;
        let game = get_game(&tx, id)?;
        tx.commit()?;
        Ok(game)
    }

    pub fn update_game(&self, id: i64, input: GameInput) -> Result<Game> {
        let g = validated(input)?;
        let mut conn = self.conn();
        let tx = conn.transaction()?;
        let old = get_game(&tx, id)?;

        // Fields Backloggery also tracks; editing them marks the game as diverged.
        let diverged = old.backloggery_id.is_some()
            && (old.title != g.title
                || old.platform != g.platform
                || old.status != g.status
                || old.priority != g.priority
                || old.ownership != g.ownership
                || old.format != g.format
                || old.region != g.region
                || old.rating != g.rating
                || old.notes != g.notes);

        tx.execute(
            "UPDATE games SET title = ?1, platform = ?2, platform_abbr = ?3, status = ?4, priority = ?5,
                ownership = ?6, format = ?7, region = ?8, rating = ?9, progress = ?10, hours_played = ?11,
                notes = ?12, cover_url = ?13, achievements_done = ?14, achievements_total = ?15,
                local_changes = local_changes OR ?16, updated_at = datetime('now')
             WHERE id = ?17",
            params![
                g.title, g.platform, g.platform_abbr, g.status, g.priority, g.ownership, g.format,
                g.region, g.rating, g.progress, g.hours_played, g.notes, g.cover_url,
                g.achievements_done, g.achievements_total, diverged, id
            ],
        )?;

        if old.status != g.status {
            add_log(&tx, id, &format!("{} → {}", old.status.label(), g.status.label()))?;
        }
        if old.priority != g.priority {
            add_log(&tx, id, &format!("Priority: {}", g.priority.label()))?;
        }

        let game = get_game(&tx, id)?;
        tx.commit()?;
        Ok(game)
    }

    pub fn delete_game(&self, id: i64) -> Result<()> {
        self.conn().execute("DELETE FROM games WHERE id = ?1", [id])?;
        Ok(())
    }

    pub fn set_cover(&self, id: i64, url: &str) -> Result<()> {
        self.conn().execute(
            "UPDATE games SET cover_url = ?1, updated_at = datetime('now') WHERE id = ?2",
            params![url, id],
        )?;
        Ok(())
    }

    pub fn game_title(&self, id: i64) -> Result<String> {
        Ok(self.conn().query_row("SELECT title FROM games WHERE id = ?1", [id], |r| r.get(0))?)
    }

    pub fn games_missing_covers(&self) -> Result<Vec<(i64, String)>> {
        let conn = self.conn();
        let mut stmt = conn.prepare(
            "SELECT id, title FROM games WHERE cover_url IS NULL OR cover_url = '' ORDER BY title COLLATE NOCASE",
        )?;
        let rows = stmt.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?.collect::<rusqlite::Result<_>>()?;
        Ok(rows)
    }

    pub fn list_log(&self, game_id: i64) -> Result<Vec<LogEntry>> {
        let conn = self.conn();
        let mut stmt = conn.prepare(
            "SELECT id, game_id, at, note FROM game_log WHERE game_id = ?1 ORDER BY at DESC, id DESC",
        )?;
        let rows = stmt.query_map([game_id], row_to_log)?.collect::<rusqlite::Result<_>>()?;
        Ok(rows)
    }

    pub fn add_note(&self, game_id: i64, note: &str) -> Result<LogEntry> {
        let note = note.trim();
        if note.is_empty() {
            return Err(Error::msg("Note is empty."));
        }
        let conn = self.conn();
        let id = add_log(&conn, game_id, note)?;
        Ok(conn.query_row("SELECT id, game_id, at, note FROM game_log WHERE id = ?1", [id], row_to_log)?)
    }

    pub fn settings(&self) -> Result<Settings> {
        let conn = self.conn();
        Ok(Settings {
            backloggery_username: get_setting(&conn, KEY_BL_USER)?,
            steamgriddb_key: get_setting(&conn, KEY_SGDB)?,
            last_sync: get_setting(&conn, KEY_LAST_SYNC)?,
        })
    }

    pub fn save_settings(&self, s: &Settings) -> Result<()> {
        let conn = self.conn();
        set_setting(&conn, KEY_BL_USER, s.backloggery_username.as_deref())?;
        set_setting(&conn, KEY_SGDB, s.steamgriddb_key.as_deref())?;
        Ok(())
    }

    /// Merges a Backloggery library into the local one.
    ///
    /// Backloggery is the source of truth for the fields it tracks, but only when
    /// the game changed there since the last sync; otherwise local edits are kept.
    /// Local-only data (progress, hours, cover, log) is never touched.
    pub fn apply_backloggery(&self, games: &[ImportedGame]) -> Result<SyncReport> {
        let mut report = SyncReport::default();
        let mut conn = self.conn();
        let tx = conn.transaction()?;
        {
            let mut find = tx.prepare(
                "SELECT id, backloggery_updated, local_changes FROM games WHERE backloggery_id = ?1",
            )?;
            let mut insert = tx.prepare(
                "INSERT INTO games (title, platform, platform_abbr, status, priority, ownership, format, region,
                    rating, progress, notes, achievements_done, achievements_total, backloggery_id,
                    backloggery_parent_id, backloggery_updated)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16)",
            )?;
            let mut update = tx.prepare(
                "UPDATE games SET title = ?1, platform = ?2, platform_abbr = ?3, status = ?4, priority = ?5,
                    ownership = ?6, format = ?7, region = ?8, rating = ?9, notes = ?10, achievements_done = ?11,
                    achievements_total = ?12, backloggery_parent_id = ?13, backloggery_updated = ?14,
                    local_changes = 0, updated_at = datetime('now')
                 WHERE id = ?15",
            )?;

            for g in games {
                let existing: Option<(i64, Option<String>, bool)> = find
                    .query_row([g.backloggery_id], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))
                    .optional()?;
                match existing {
                    None => {
                        let progress: u8 = match g.status {
                            Status::Beaten | Status::Completed => 100,
                            _ => 0,
                        };
                        insert.execute(params![
                            g.title, g.platform, g.platform_abbr, g.status, g.priority, g.ownership,
                            g.format, g.region, g.rating, progress, g.notes, g.achievements_done,
                            g.achievements_total, g.backloggery_id, g.parent_id, g.updated
                        ])?;
                        add_log(&tx, tx.last_insert_rowid(), "Imported from Backloggery")?;
                        report.added += 1;
                    }
                    Some((_, ref updated, _)) if *updated == g.updated => report.unchanged += 1,
                    Some((id, _, had_local_changes)) => {
                        update.execute(params![
                            g.title, g.platform, g.platform_abbr, g.status, g.priority, g.ownership,
                            g.format, g.region, g.rating, g.notes, g.achievements_done,
                            g.achievements_total, g.parent_id, g.updated, id
                        ])?;
                        report.updated += 1;
                        if had_local_changes {
                            report.overwritten_local += 1;
                        }
                    }
                }
            }

            let remote: HashSet<i64> = games.iter().map(|g| g.backloggery_id).collect();
            let mut known = tx.prepare("SELECT backloggery_id FROM games WHERE backloggery_id IS NOT NULL")?;
            for id in known.query_map([], |r| r.get::<_, i64>(0))? {
                if !remote.contains(&id?) {
                    report.missing_remote += 1;
                }
            }
        }
        tx.execute(
            "INSERT INTO settings (key, value) VALUES (?1, datetime('now'))
             ON CONFLICT(key) DO UPDATE SET value = excluded.value",
            [KEY_LAST_SYNC],
        )?;
        tx.commit()?;
        Ok(report)
    }
}

fn migrate(conn: &mut Connection) -> Result<()> {
    let version: i64 = conn.pragma_query_value(None, "user_version", |r| r.get(0))?;
    let tx = conn.transaction()?;
    for (i, sql) in (1..).zip(MIGRATIONS).skip_while(|(i, _)| *i <= version) {
        tx.execute_batch(sql)?;
        tx.pragma_update(None, "user_version", i)?;
    }
    tx.commit()?;
    Ok(())
}

fn validated(input: GameInput) -> Result<GameInput> {
    let g = input.normalized();
    if g.title.is_empty() {
        return Err(Error::msg("A game needs a title."));
    }
    Ok(g)
}

fn get_game(conn: &Connection, id: i64) -> Result<Game> {
    conn.query_row(&format!("SELECT {GAME_COLUMNS} FROM games WHERE id = ?1"), [id], row_to_game)
        .optional()?
        .ok_or_else(|| Error::msg(format!("Game {id} not found.")))
}

fn add_log(conn: &Connection, game_id: i64, note: &str) -> Result<i64> {
    conn.execute("INSERT INTO game_log (game_id, note) VALUES (?1, ?2)", params![game_id, note])?;
    Ok(conn.last_insert_rowid())
}

fn get_setting(conn: &Connection, key: &str) -> Result<Option<String>> {
    Ok(conn.query_row("SELECT value FROM settings WHERE key = ?1", [key], |r| r.get(0)).optional()?)
}

fn set_setting(conn: &Connection, key: &str, value: Option<&str>) -> Result<()> {
    match value.map(str::trim).filter(|v| !v.is_empty()) {
        Some(v) => conn.execute(
            "INSERT INTO settings (key, value) VALUES (?1, ?2)
             ON CONFLICT(key) DO UPDATE SET value = excluded.value",
            [key, v],
        )?,
        None => conn.execute("DELETE FROM settings WHERE key = ?1", [key])?,
    };
    Ok(())
}

fn row_to_game(r: &Row<'_>) -> rusqlite::Result<Game> {
    Ok(Game {
        id: r.get(0)?,
        title: r.get(1)?,
        platform: r.get(2)?,
        platform_abbr: r.get(3)?,
        status: r.get(4)?,
        priority: r.get(5)?,
        ownership: r.get(6)?,
        format: r.get(7)?,
        region: r.get(8)?,
        rating: r.get(9)?,
        progress: r.get(10)?,
        hours_played: r.get(11)?,
        notes: r.get(12)?,
        cover_url: r.get(13)?,
        achievements_done: r.get(14)?,
        achievements_total: r.get(15)?,
        backloggery_id: r.get(16)?,
        local_changes: r.get(17)?,
        created_at: r.get(18)?,
        updated_at: r.get(19)?,
    })
}

fn row_to_log(r: &Row<'_>) -> rusqlite::Result<LogEntry> {
    Ok(LogEntry { id: r.get(0)?, game_id: r.get(1)?, at: r.get(2)?, note: r.get(3)? })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::{Ownership, Priority};

    fn db() -> Db {
        Db::open(Path::new(":memory:")).unwrap()
    }

    fn imported(id: i64, status: Status, updated: &str) -> ImportedGame {
        ImportedGame {
            backloggery_id: id,
            parent_id: None,
            title: format!("Game {id}"),
            platform: "PC".into(),
            platform_abbr: Some("PC".into()),
            status,
            priority: Priority::Normal,
            ownership: Ownership::Owned,
            format: None,
            region: None,
            rating: None,
            achievements_done: 0,
            achievements_total: 0,
            notes: String::new(),
            updated: Some(updated.into()),
        }
    }

    fn input_from(g: &Game) -> GameInput {
        serde_json::from_value(serde_json::to_value(g).unwrap()).unwrap()
    }

    #[test]
    fn sync_adds_then_skips_unchanged() {
        let db = db();
        let r = db.apply_backloggery(&[imported(1, Status::Beaten, "t1")]).unwrap();
        assert_eq!(r.added, 1);
        let g = &db.list_games().unwrap()[0];
        assert_eq!(g.progress, 100);

        let r = db.apply_backloggery(&[imported(1, Status::Beaten, "t1")]).unwrap();
        assert_eq!((r.added, r.updated, r.unchanged), (0, 0, 1));
    }

    #[test]
    fn local_edits_survive_until_remote_changes() {
        let db = db();
        db.apply_backloggery(&[imported(1, Status::Unplayed, "t1")]).unwrap();
        let g = db.list_games().unwrap().remove(0);

        let mut input = input_from(&g);
        input.status = Status::Unfinished;
        input.hours_played = 3.5;
        let g = db.update_game(g.id, input).unwrap();
        assert!(g.local_changes);

        // Remote unchanged: the local edit stays.
        db.apply_backloggery(&[imported(1, Status::Unplayed, "t1")]).unwrap();
        assert_eq!(db.list_games().unwrap()[0].status, Status::Unfinished);

        // Remote changed: Backloggery wins, but local-only fields stay.
        let r = db.apply_backloggery(&[imported(1, Status::Completed, "t2")]).unwrap();
        assert_eq!(r.overwritten_local, 1);
        let g = &db.list_games().unwrap()[0];
        assert_eq!(g.status, Status::Completed);
        assert_eq!(g.hours_played, 3.5);
        assert!(!g.local_changes);
    }

    #[test]
    fn reports_games_removed_remotely() {
        let db = db();
        db.apply_backloggery(&[imported(1, Status::Unplayed, "t"), imported(2, Status::Unplayed, "t")])
            .unwrap();
        let r = db.apply_backloggery(&[imported(1, Status::Unplayed, "t")]).unwrap();
        assert_eq!(r.missing_remote, 1);
        assert_eq!(db.list_games().unwrap().len(), 2);
    }

    #[test]
    fn status_changes_are_logged() {
        let db = db();
        let g = db
            .create_game(GameInput {
                title: "  Hades ".into(),
                ..serde_json::from_str(r#"{"title":"x"}"#).unwrap()
            })
            .unwrap();
        assert_eq!(g.title, "Hades");
        let mut input = input_from(&g);
        input.status = Status::Beaten;
        db.update_game(g.id, input).unwrap();
        let log = db.list_log(g.id).unwrap();
        assert_eq!(log[0].note, "Unplayed → Beaten");
        assert_eq!(log.len(), 2);
    }
}
