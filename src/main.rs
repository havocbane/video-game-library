// Hide the console window in release builds on Windows.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod backloggery;
mod commands;
mod covers;
mod db;
mod error;
mod models;
mod sources;

use tauri::Manager;

use commands::AppState;
use db::Db;

fn main() {
    tauri::Builder::default()
        // Opens help links in the user's browser instead of inside the app window.
        .plugin(tauri_plugin_opener::init())
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
            app.manage(AppState { db, http, builtin_sources: sources::builtin_sources(), plugins_dir });
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
