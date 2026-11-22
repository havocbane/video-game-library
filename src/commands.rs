use serde::Serialize;
use tauri::{AppHandle, Emitter, State};

use crate::backloggery;
use crate::covers;
use crate::db::Db;
use crate::error::{Error, Result};
use crate::models::{CoverReport, Game, GameInput, LogEntry, Settings, SyncReport};

pub struct AppState {
    pub db: Db,
    pub http: reqwest::Client,
}

impl AppState {
    fn steamgriddb_key(&self) -> Result<String> {
        self.db.settings()?.steamgriddb_key.ok_or_else(|| {
            Error::msg("Add a SteamGridDB API key in Settings to look up cover art.")
        })
    }
}

#[tauri::command]
pub fn list_games(state: State<'_, AppState>) -> Result<Vec<Game>> {
    state.db.list_games()
}

#[tauri::command]
pub fn create_game(state: State<'_, AppState>, input: GameInput) -> Result<Game> {
    state.db.create_game(input)
}

#[tauri::command]
pub fn update_game(state: State<'_, AppState>, id: i64, input: GameInput) -> Result<Game> {
    state.db.update_game(id, input)
}

#[tauri::command]
pub fn delete_game(state: State<'_, AppState>, id: i64) -> Result<()> {
    state.db.delete_game(id)
}

#[tauri::command]
pub fn list_log(state: State<'_, AppState>, game_id: i64) -> Result<Vec<LogEntry>> {
    state.db.list_log(game_id)
}

#[tauri::command]
pub fn add_note(state: State<'_, AppState>, game_id: i64, note: String) -> Result<LogEntry> {
    state.db.add_note(game_id, &note)
}

#[tauri::command]
pub fn get_settings(state: State<'_, AppState>) -> Result<Settings> {
    state.db.settings()
}

#[tauri::command]
pub fn save_settings(state: State<'_, AppState>, settings: Settings) -> Result<()> {
    state.db.save_settings(&settings)
}

#[tauri::command]
pub async fn sync_backloggery(state: State<'_, AppState>) -> Result<SyncReport> {
    let username = state
        .db
        .settings()?
        .backloggery_username
        .ok_or_else(|| Error::msg("Set your Backloggery username in Settings first."))?;
    let games = backloggery::fetch_library(&state.http, &username).await?;
    state.db.apply_backloggery(&games)
}

#[tauri::command]
pub async fn find_cover(state: State<'_, AppState>, id: i64) -> Result<Option<String>> {
    let key = state.steamgriddb_key()?;
    let title = state.db.game_title(id)?;
    let url = covers::find_cover(&state.http, &key, &title).await?;
    if let Some(url) = &url {
        state.db.set_cover(id, url)?;
    }
    Ok(url)
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct CoverProgress {
    done: usize,
    total: usize,
    id: i64,
    url: Option<String>,
}

/// Looks up covers for every game without one, emitting `covers-progress` as it goes.
#[tauri::command]
pub async fn fetch_missing_covers(app: AppHandle, state: State<'_, AppState>) -> Result<CoverReport> {
    let key = state.steamgriddb_key()?;
    let missing = state.db.games_missing_covers()?;
    let total = missing.len();
    let mut report = CoverReport::default();

    for (i, (id, title)) in missing.into_iter().enumerate() {
        let url = match covers::find_cover(&state.http, &key, &title).await {
            Ok(Some(url)) => {
                state.db.set_cover(id, &url)?;
                report.found += 1;
                Some(url)
            }
            Ok(None) => {
                report.not_found += 1;
                None
            }
            // A bad key fails every request; stop rather than hammering the API.
            Err(Error::Http(e)) if e.status().is_some_and(|s| s.as_u16() == 401) => {
                return Err(Error::msg("SteamGridDB rejected the API key."));
            }
            Err(_) => {
                report.failed += 1;
                None
            }
        };
        // The UI only uses this for progress; a dropped event is harmless.
        let _ = app.emit("covers-progress", CoverProgress { done: i + 1, total, id, url });
    }
    Ok(report)
}
