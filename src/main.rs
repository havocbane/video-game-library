// Hide the console window in release builds on Windows.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod backloggery;
mod commands;
mod cover_cache;
mod covers;
mod db;
mod error;
mod models;
mod sources;

use std::sync::Arc;

use tauri::Manager;
use tauri::http::{Response, StatusCode, header};

use commands::AppState;
use db::Db;

fn main() {
    tauri::Builder::default()
        // Opens help links in the user's browser instead of inside the app window.
        .plugin(tauri_plugin_opener::init())
        // Covers load as cover://<encoded image URL>, served from the local cache.
        .register_asynchronous_uri_scheme_protocol("cover", |ctx, request, responder| {
            let app = ctx.app_handle().clone();
            let url = cover_cache::decode_path(request.uri().path());
            tauri::async_runtime::spawn(async move {
                let covers = Arc::clone(&app.state::<AppState>().covers);
                let cover = match url {
                    Some(url) => covers.get(&url).await.ok(),
                    None => None,
                };
                let response = match cover {
                    Some(c) => Response::builder()
                        .header(header::CONTENT_TYPE, c.mime)
                        .header(header::CACHE_CONTROL, "max-age=86400")
                        .body(c.bytes),
                    // The UI shows its generated cover instead.
                    None => Response::builder().status(StatusCode::NOT_FOUND).body(Vec::new()),
                };
                responder.respond(response.expect("valid response"));
            });
        })
        .setup(|app| {
            // VGL_DATA_DIR points the app at another library (testing, portable installs).
            let data_dir = match std::env::var_os("VGL_DATA_DIR") {
                Some(dir) => dir.into(),
                None => app.path().app_data_dir()?,
            };
            std::fs::create_dir_all(&data_dir)?;
            let db = Db::open(&data_dir.join("library.db"))?;
            let http = reqwest::Client::builder()
                .user_agent(concat!("VideoGameLibrary/", env!("CARGO_PKG_VERSION")))
                .timeout(std::time::Duration::from_secs(30))
                .build()?;
            let plugins_dir = data_dir.join("plugins");
            if let Err(e) = sources::external::install_bundled(&plugins_dir) {
                eprintln!("couldn't install bundled plugins: {e}");
            }
            let covers = Arc::new(cover_cache::CoverCache::new(data_dir.join("covers"), http.clone())?);
            let state = AppState { db, http, builtin_sources: sources::builtin_sources(), plugins_dir, covers };
            // Catch up on covers added before the cache existed or while offline.
            state.refresh_cover_cache();
            app.manage(state);
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            commands::list_games,
            commands::create_game,
            commands::update_game,
            commands::delete_game,
            commands::list_log,
            commands::add_note,
            commands::get_settings,
            commands::save_settings,
            commands::sync_backloggery,
            commands::find_cover,
            commands::fetch_missing_covers,
            commands::list_game_sources,
            commands::merge_games,
            commands::split_source,
            commands::list_sources,
            commands::save_source_config,
            commands::run_import,
        ])
        .run(tauri::generate_context!())
        .expect("error while running the app");
}
