use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

use serde::Serialize;
use tauri::{AppHandle, Emitter, State};

use crate::backloggery;
use crate::cover_cache::CoverCache;
use crate::covers;
use crate::db::Db;
use crate::error::{Error, Result};
use crate::models::{CoverReport, Game, GameInput, GameSourceLink, ImportReport, LogEntry, Settings, SyncReport};
use crate::sources::{self, Source, SourceContext, SourceInfo};

pub struct AppState {
    pub db: Db,
    pub http: reqwest::Client,
    pub builtin_sources: Vec<Arc<dyn Source>>,
    pub plugins_dir: PathBuf,
    pub covers: Arc<CoverCache>,
}

impl AppState {
    fn sources(&self) -> Vec<Arc<dyn Source>> {
        sources::all_sources(&self.builtin_sources, &self.plugins_dir)
    }
}

impl AppState {
    /// Brings the cover cache in line with the library in the background:
    /// downloads new covers and deletes ones no game uses any more.
    pub fn refresh_cover_cache(&self) {
        let Ok(urls) = self.db.cover_urls() else { return };
        let covers = Arc::clone(&self.covers);
        tauri::async_runtime::spawn(async move {
            covers.prune(&urls);
            covers.warm(urls).await;
        });
    }

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
    let game = state.db.create_game(input)?;
    state.refresh_cover_cache();
    Ok(game)
}

#[tauri::command]
pub fn update_game(state: State<'_, AppState>, id: i64, input: GameInput) -> Result<Game> {
    let game = state.db.update_game(id, input)?;
    state.refresh_cover_cache();
    Ok(game)
}

#[tauri::command]
pub fn delete_game(state: State<'_, AppState>, id: i64) -> Result<()> {
    state.db.delete_game(id)?;
    state.refresh_cover_cache();
    Ok(())
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
pub fn save_settings(state: State<'_, AppState>, mut settings: Settings) -> Result<()> {
    settings.backloggery_username = settings.backloggery_username.as_deref().map(backloggery::normalize_username);
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
    let report = state.db.apply_backloggery(&games)?;
    state.refresh_cover_cache();
    Ok(report)
}

#[tauri::command]
pub async fn find_cover(state: State<'_, AppState>, id: i64) -> Result<Option<String>> {
    let key = state.steamgriddb_key()?;
    let title = state.db.game_title(id)?;
    let url = covers::find_cover(&state.http, &key, &title).await?;
    if let Some(url) = &url {
        state.db.set_cover(id, url)?;
        state.refresh_cover_cache();
    }
    Ok(url)
}

#[tauri::command]
pub fn merge_games(state: State<'_, AppState>, target_id: i64, other_id: i64) -> Result<Game> {
    let game = state.db.merge_games(target_id, other_id)?;
    state.refresh_cover_cache();
    Ok(game)
}

#[tauri::command]
pub fn split_source(state: State<'_, AppState>, game_id: i64, source: String, external_id: String) -> Result<Game> {
    state.db.split_source(game_id, &source, &external_id)
}

#[tauri::command]
pub fn list_game_sources(state: State<'_, AppState>, game_id: i64) -> Result<Vec<GameSourceLink>> {
    state.db.list_game_sources(game_id)
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SourceView {
    #[serde(flatten)]
    info: SourceInfo,
    /// Saved settings.
    config: HashMap<String, String>,
    /// Values found on this machine, used when a setting is left empty.
    detected: HashMap<String, String>,
    last_import: Option<String>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SourcesView {
    sources: Vec<SourceView>,
    plugins_dir: String,
}

#[tauri::command]
pub fn list_sources(state: State<'_, AppState>) -> Result<SourcesView> {
    let sources = state
        .sources()
        .iter()
        .map(|s| {
            let info = s.info();
            Ok(SourceView {
                config: state.db.source_config(&info.id)?,
                detected: s.detect(),
                last_import: state.db.last_import(&info.id)?,
                info,
            })
        })
        .collect::<Result<_>>()?;
    Ok(SourcesView { sources, plugins_dir: state.plugins_dir.display().to_string() })
}

#[tauri::command]
pub fn save_source_config(state: State<'_, AppState>, id: String, config: HashMap<String, String>) -> Result<()> {
    state.db.save_source_config(&id, &config)
}

#[tauri::command]
pub async fn run_import(state: State<'_, AppState>, id: String) -> Result<ImportReport> {
    let source = state
        .sources()
        .into_iter()
        .find(|s| s.info().id == id)
        .ok_or_else(|| Error::msg(format!("No import source named \"{id}\".")))?;

    // Saved settings win; detected values fill the gaps.
    let mut config = source.detect();
    config.extend(state.db.source_config(&id)?.into_iter().filter(|(_, v)| !v.trim().is_empty()));

    let ctx = SourceContext { http: state.http.clone(), config };
    let games = source.fetch(&ctx).await?;
    let report = state.db.apply_import(&id, &source.info().name, &games)?;
    state.refresh_cover_cache();
    Ok(report)
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
    state.refresh_cover_cache();
    Ok(report)
}
