// Hide the console window in release builds on Windows.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod backloggery;
mod commands;
mod covers;
mod db;
mod error;
mod models;

use tauri::Manager;

use commands::AppState;
use db::Db;

fn main() {
    tauri::Builder::default()
        .setup(|app| {
            let data_dir = app.path().app_data_dir()?;
            std::fs::create_dir_all(&data_dir)?;
            let db = Db::open(&data_dir.join("library.db"))?;
            let http = reqwest::Client::builder()
                .user_agent(concat!("VideoGameLibrary/", env!("CARGO_PKG_VERSION")))
                .timeout(std::time::Duration::from_secs(30))
                .build()?;
            app.manage(AppState { db, http });
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
        ])
        .run(tauri::generate_context!())
        .expect("error while running the app");
}
