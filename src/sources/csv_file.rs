//! Spreadsheet import for anything without an API: Nintendo eShop purchases,
//! a physical collection, Humble or itch.io exports, etc.
//!
//! Needs a header row with at least a `title` (or `name` / `game`) column.
//! Optional columns: platform, store, status, ownership, format, hours, last played.

use std::path::PathBuf;

use super::{BoxFuture, FieldKind, Source, SourceContext, SourceField, SourceGame, SourceInfo, normalize_title};
use crate::error::{Error, Result};
use crate::models::{Ownership, Status};

pub struct CsvFile;

impl Source for CsvFile {
    fn info(&self) -> SourceInfo {
        SourceInfo {
            id: "csv".into(),
            name: "CSV file".into(),
            description: "Import a spreadsheet: Nintendo purchases, a physical shelf, a Humble export… Columns: \
                title (required), platform, store, status, ownership, format, hours, last played."
                .into(),
            fields: vec![
                SourceField::new("path", "CSV file", FieldKind::Path).placeholder(r"C:\Users\you\Documents\games.csv"),
                SourceField::new("defaultPlatform", "Platform when the row has none", FieldKind::Text)
                    .placeholder("Nintendo Switch")
                    .optional(),
                SourceField::new("defaultFormat", "Format when the row has none", FieldKind::Text)
                    .placeholder("Digital or Physical")
                    .optional(),
            ],
            origin: "built-in".into(),
        }
    }

    fn fetch<'a>(&'a self, ctx: &'a SourceContext) -> BoxFuture<'a, Result<Vec<SourceGame>>> {
        Box::pin(async move {
            let path = PathBuf::from(ctx.require("path", "CSV file")?.trim_matches('"'));
            let text = std::fs::read_to_string(&path)
                .map_err(|e| Error::msg(format!("Can't read {}: {e}", path.display())))?;
            parse(&text, ctx.get("defaultPlatform").unwrap_or("PC"), ctx.get("defaultFormat"))
        })
    }
}

fn parse(text: &str, default_platform: &str, default_format: Option<&str>) -> Result<Vec<SourceGame>> {
    let mut reader = csv::ReaderBuilder::new().flexible(true).trim(csv::Trim::All).from_reader(text.as_bytes());
    let headers: Vec<String> = reader
        .headers()
        .map_err(csv_err)?
        .iter()
        .map(|h| h.trim_start_matches('\u{feff}').to_lowercase().replace(['_', '-'], " "))
        .collect();
    let col = |names: &[&str]| headers.iter().position(|h| names.contains(&h.as_str()));

    let title_col = col(&["title", "name", "game", "game title"])
        .ok_or_else(|| Error::msg("The CSV needs a header row with a \"title\" column."))?;
    let platform_col = col(&["platform", "system", "console"]);
    let store_col = col(&["store", "storefront", "source"]);
    let status_col = col(&["status"]);
    let ownership_col = col(&["ownership", "own"]);
    let format_col = col(&["format", "media"]);
    let hours_col = col(&["hours", "hours played", "playtime", "playtime hours"]);
    let last_col = col(&["last played", "lastplayed"]);

    let mut games = Vec::new();
    for (line, record) in reader.records().enumerate() {
        let record = record.map_err(csv_err)?;
        let field = |c: Option<usize>| c.and_then(|i| record.get(i)).map(str::trim).filter(|s| !s.is_empty());
        let Some(title) = field(Some(title_col)) else { continue };
        let platform = field(platform_col).unwrap_or(default_platform).to_string();
        let enum_key = |s: &str| s.to_lowercase().replace([' ', '-'], "_");

        let status = match field(status_col) {
            Some(s) => Some(Status::parse(&enum_key(s)).ok_or_else(|| {
                Error::msg(format!("Row {}: unknown status \"{s}\".", line + 2))
            })?),
            None => None,
        };
        let ownership = field(ownership_col).and_then(|s| Ownership::parse(&enum_key(s)));
        let hours: Option<f64> = field(hours_col).and_then(|h| h.parse().ok());

        games.push(SourceGame {
            // Rows have no ids; title + platform keeps re-imports idempotent.
            external_id: format!("{}|{}", normalize_title(title), platform.to_lowercase()),
            title: title.to_string(),
            platform,
            store: field(store_col).map(str::to_string),
            playtime_minutes: hours.map(|h| (h * 60.0).round() as i64),
            last_played: field(last_col).map(str::to_string),
            cover_url: None,
            ownership,
            format: field(format_col).or(default_format).map(str::to_string),
            status,
        });
    }
    Ok(games)
}

fn csv_err(e: csv::Error) -> Error {
    Error::msg(format!("Couldn't read the CSV: {e}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_loose_headers() {
        let csv = "\u{feff}Name,System,Status,Ownership,Hours,Format\n\
                   Zelda: Breath of the Wild,Nintendo Switch,Completed,Owned,120,Physical\n\
                   Metroid Prime,,Unplayed,wishlist,,\n\
                   ,PS5,,,,\n";
        let games = parse(csv, "GameCube", Some("Digital")).unwrap();
        assert_eq!(games.len(), 2);
        assert_eq!(games[0].status, Some(Status::Completed));
        assert_eq!(games[0].playtime_minutes, Some(7200));
        assert_eq!(games[0].format.as_deref(), Some("Physical"));
        assert_eq!(games[1].platform, "GameCube");
        assert_eq!(games[1].ownership, Some(Ownership::Wishlist));
        assert_eq!(games[1].format.as_deref(), Some("Digital"));
    }

    #[test]
    fn rejects_unknown_status() {
        assert!(parse("title,status\nX,Sorta done\n", "PC", None).is_err());
    }
}
