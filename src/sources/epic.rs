//! Epic Games library from the Epic Games Launcher's local data. Epic has no
//! public library API, but the launcher caches the catalog entries for your
//! library (`Catalog\catcache.bin`, base64-encoded JSON, including box art)
//! and writes a manifest per installed game (`Manifests\*.item`).

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use base64::Engine;
use serde::Deserialize;

use super::{BoxFuture, FieldKind, Source, SourceContext, SourceField, SourceGame, SourceInfo};
use crate::error::{Error, Result};

pub struct Epic;

const DEFAULT_DIR: &str = r"C:\ProgramData\Epic\EpicGamesLauncher\Data";

#[derive(Deserialize)]
#[serde(rename_all = "PascalCase")]
struct Manifest {
    display_name: String,
    app_name: String,
    catalog_item_id: Option<String>,
    #[serde(default)]
    app_categories: Vec<String>,
    #[serde(default)]
    b_is_incomplete_install: bool,
    main_game_app_name: Option<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct CatalogItem {
    id: String,
    title: String,
    #[serde(default)]
    categories: Vec<Category>,
    main_game_item: Option<ItemRef>,
    #[serde(default)]
    key_images: Vec<KeyImage>,
}

#[derive(Deserialize)]
struct Category {
    path: String,
}

#[derive(Deserialize)]
struct ItemRef {
    #[serde(default)]
    id: String,
}

#[derive(Deserialize)]
struct KeyImage {
    #[serde(rename = "type")]
    kind: String,
    url: String,
}

impl CatalogItem {
    fn is_base_game(&self) -> bool {
        self.categories.iter().any(|c| c.path == "games")
            // DLC and add-ons point at their parent game.
            && self.main_game_item.as_ref().is_none_or(|m| m.id.is_empty())
    }

    /// Portrait box art, falling back to the thumbnail.
    fn cover(&self) -> Option<String> {
        ["DieselGameBoxTall", "OfferImageTall", "Thumbnail"]
            .iter()
            .find_map(|kind| self.key_images.iter().find(|i| i.kind == *kind))
            .map(|i| i.url.clone())
    }
}

impl Source for Epic {
    fn info(&self) -> SourceInfo {
        SourceInfo {
            // Kept from when this only read installed games, so earlier imports stay linked.
            id: "epic-installed".into(),
            name: "Epic Games".into(),
            description: "Your Epic library, installed or not, with box art. Read from the Epic Games \
                Launcher's local cache; open the launcher's Library once so the cache is current."
                .into(),
            fields: vec![
                SourceField::new("dataDir", "Launcher data folder", FieldKind::Path).placeholder(DEFAULT_DIR),
            ],
            origin: "built-in".into(),
        }
    }

    fn detect(&self) -> HashMap<String, String> {
        if Path::new(DEFAULT_DIR).is_dir() {
            HashMap::from([("dataDir".to_string(), DEFAULT_DIR.to_string())])
        } else {
            HashMap::new()
        }
    }

    fn fetch<'a>(&'a self, ctx: &'a SourceContext) -> BoxFuture<'a, Result<Vec<SourceGame>>> {
        // Older settings pointed at the Manifests folder itself.
        let dir = match (ctx.get("dataDir"), ctx.get("manifestDir")) {
            (Some(dir), _) => PathBuf::from(dir),
            (None, Some(manifests)) => Path::new(manifests).parent().map_or_else(|| DEFAULT_DIR.into(), Path::to_path_buf),
            (None, None) => PathBuf::from(DEFAULT_DIR),
        };
        Box::pin(async move { read_library(&dir) })
    }
}

fn read_library(data_dir: &Path) -> Result<Vec<SourceGame>> {
    let catalog = read_catalog(&data_dir.join("Catalog").join("catcache.bin"));
    let installed = read_manifests(&data_dir.join("Manifests"));
    if catalog.is_none() && installed.is_none() {
        return Err(Error::msg(format!(
            "No Epic Games Launcher data found in {}. Is the launcher installed?",
            data_dir.display()
        )));
    }
    let catalog = catalog.unwrap_or_default();
    let covers: HashMap<&str, String> =
        catalog.iter().filter_map(|i| Some((i.id.as_str(), i.cover()?))).collect();

    let mut games: Vec<SourceGame> = catalog
        .iter()
        .filter(|i| i.is_base_game())
        .map(|i| epic_game(i.id.clone(), i.title.trim().to_string(), i.cover()))
        .collect();

    // Installed games the cache doesn't know about yet.
    for m in installed.unwrap_or_default() {
        let id = m.catalog_item_id.clone().unwrap_or_else(|| m.app_name.clone());
        if !games.iter().any(|g| g.external_id == id) {
            let cover = covers.get(id.as_str()).cloned();
            games.push(epic_game(id, m.display_name, cover));
        }
    }
    Ok(games)
}

fn epic_game(id: String, title: String, cover_url: Option<String>) -> SourceGame {
    SourceGame {
        external_id: id,
        title,
        platform: "PC".into(),
        store: Some("Epic".into()),
        playtime_minutes: None,
        last_played: None,
        cover_url,
        ownership: None,
        format: None,
        status: None,
    }
}

fn read_catalog(path: &Path) -> Option<Vec<CatalogItem>> {
    let encoded = std::fs::read(path).ok()?;
    let encoded: Vec<u8> = encoded.into_iter().filter(|b| !b.is_ascii_whitespace()).collect();
    let json = base64::engine::general_purpose::STANDARD.decode(encoded).ok()?;
    // Skip entries this parser doesn't understand rather than failing the import.
    let items: Vec<serde_json::Value> = serde_json::from_slice(&json).ok()?;
    Some(items.into_iter().filter_map(|v| serde_json::from_value(v).ok()).collect())
}

/// Base-game manifests of completely installed games.
fn read_manifests(dir: &Path) -> Option<Vec<Manifest>> {
    let entries = std::fs::read_dir(dir).ok()?;
    let manifests = entries
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|ext| ext == "item"))
        .filter_map(|p| serde_json::from_str::<Manifest>(&std::fs::read_to_string(p).ok()?).ok())
        .filter(|m| {
            m.app_categories.iter().any(|c| c == "games")
                && !m.app_categories.iter().any(|c| c == "hidden")
                && !m.b_is_incomplete_install
                && m.main_game_app_name.as_deref().is_none_or(|main| main.is_empty() || main == m.app_name)
        })
        .collect();
    Some(manifests)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn catalog_items() {
        let json = r#"[
            {"id":"a","title":"Trine","categories":[{"path":"games"}],"mainGameItem":{"namespace":"","id":""},
             "keyImages":[{"type":"DieselGameBox","url":"wide"},{"type":"DieselGameBoxTall","url":"tall"}]},
            {"id":"b","title":"Trine DLC","categories":[{"path":"games"}],"mainGameItem":{"namespace":"x","id":"a"},"keyImages":[]},
            {"id":"c","title":"Unreal Engine","categories":[{"path":"engines"}],"keyImages":[]},
            {"unexpected":true}
        ]"#;
        let items: Vec<CatalogItem> = serde_json::from_str::<Vec<serde_json::Value>>(json)
            .unwrap()
            .into_iter()
            .filter_map(|v| serde_json::from_value(v).ok())
            .collect();
        let base: Vec<_> = items.iter().filter(|i| i.is_base_game()).collect();
        assert_eq!(base.len(), 1);
        assert_eq!(base[0].cover().as_deref(), Some("tall"));
    }

    /// Reads this machine's Epic launcher data: `cargo test -- --ignored local_epic --nocapture`
    #[test]
    #[ignore]
    fn local_epic() {
        let games = read_library(Path::new(DEFAULT_DIR)).unwrap();
        let covers = games.iter().filter(|g| g.cover_url.is_some()).count();
        println!("epic: {} games, {covers} with covers", games.len());
        assert!(!games.is_empty());
    }
}
