use rusqlite::types::{FromSql, FromSqlError, FromSqlResult, ToSql, ToSqlOutput, ValueRef};
use serde::{Deserialize, Serialize};

/// A string-backed enum that round-trips through serde (to the UI) and SQLite.
macro_rules! str_enum {
    ($name:ident, default = $default:ident { $($variant:ident => $s:literal, $label:literal),+ $(,)? }) => {
        #[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
        pub enum $name {
            $(#[serde(rename = $s)] $variant),+
        }

        impl $name {
            pub fn as_str(self) -> &'static str {
                match self { $(Self::$variant => $s),+ }
            }
            #[allow(dead_code)] // not every enum needs display labels on the Rust side
            pub fn label(self) -> &'static str {
                match self { $(Self::$variant => $label),+ }
            }
            pub fn parse(s: &str) -> Option<Self> {
                match s { $($s => Some(Self::$variant),)+ _ => None }
            }
        }

        impl Default for $name {
            fn default() -> Self { Self::$default }
        }

        impl ToSql for $name {
            fn to_sql(&self) -> rusqlite::Result<ToSqlOutput<'_>> {
                Ok(self.as_str().into())
            }
        }

        impl FromSql for $name {
            fn column_result(v: ValueRef<'_>) -> FromSqlResult<Self> {
                let s = v.as_str()?;
                Self::parse(s).ok_or_else(|| {
                    FromSqlError::Other(format!("invalid {} value: {s}", stringify!($name)).into())
                })
            }
        }
    };
}

str_enum!(Status, default = Unplayed {
    Unplayed => "unplayed", "Unplayed",
    Unfinished => "unfinished", "Unfinished",
    Beaten => "beaten", "Beaten",
    Completed => "completed", "Completed",
    Endless => "endless", "Endless",
    NoStatus => "none", "None",
});

str_enum!(Priority, default = Normal {
    NowPlaying => "now_playing", "Now Playing",
    Ongoing => "ongoing", "Ongoing",
    Paused => "paused", "Paused",
    High => "high", "High",
    Normal => "normal", "Normal",
    Low => "low", "Low",
    Replay => "replay", "Replay",
    Shelved => "shelved", "Shelved",
});

str_enum!(Ownership, default = Owned {
    Owned => "owned", "Owned",
    Wishlist => "wishlist", "Wishlist",
    Household => "household", "Household",
    Subscription => "subscription", "Subscription",
    PlayedIt => "played_it", "Played It",
    FormerlyOwned => "formerly_owned", "Formerly Owned",
    Other => "other", "Other",
    Unknown => "unknown", "Unknown",
});

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Game {
    pub id: i64,
    pub title: String,
    /// Normalized title, equal for the same game across stores and platforms.
    pub title_key: String,
    pub platform: String,
    pub platform_abbr: Option<String>,
    pub status: Status,
    pub priority: Priority,
    pub ownership: Ownership,
    pub format: Option<String>,
    pub region: Option<String>,
    /// Half-stars, 1..=10 (same scale as Backloggery).
    pub rating: Option<u8>,
    /// Self-reported completion, 0..=100.
    pub progress: u8,
    pub hours_played: f64,
    pub notes: String,
    pub cover_url: Option<String>,
    pub achievements_done: i64,
    pub achievements_total: i64,
    pub backloggery_id: Option<i64>,
    /// Backloggery-owned fields were edited here since the last sync.
    pub local_changes: bool,
    pub created_at: String,
    pub updated_at: String,
    /// Most recent play time reported by any linked store.
    pub last_played: Option<String>,
    /// Stores this game was imported from, e.g. ["Steam", "GOG"].
    pub stores: Vec<String>,
}

/// The editable fields of a game. Unknown fields (id, timestamps, ...) are
/// ignored, so the UI can send back a whole `Game` object.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GameInput {
    pub title: String,
    #[serde(default)]
    pub platform: String,
    pub platform_abbr: Option<String>,
    #[serde(default)]
    pub status: Status,
    #[serde(default)]
    pub priority: Priority,
    #[serde(default)]
    pub ownership: Ownership,
    pub format: Option<String>,
    pub region: Option<String>,
    pub rating: Option<u8>,
    #[serde(default)]
    pub progress: u8,
    #[serde(default)]
    pub hours_played: f64,
    #[serde(default)]
    pub notes: String,
    pub cover_url: Option<String>,
    #[serde(default)]
    pub achievements_done: i64,
    #[serde(default)]
    pub achievements_total: i64,
}

impl GameInput {
    /// Trims text and clamps numbers into their valid ranges.
    pub fn normalized(mut self) -> Self {
        fn clean(s: Option<String>) -> Option<String> {
            s.map(|s| s.trim().to_string()).filter(|s| !s.is_empty())
        }
        self.title = self.title.trim().to_string();
        self.platform = self.platform.trim().to_string();
        self.platform_abbr = clean(self.platform_abbr);
        self.format = clean(self.format);
        self.region = clean(self.region);
        self.cover_url = clean(self.cover_url);
        self.rating = self.rating.filter(|r| (1..=10).contains(r));
        self.progress = self.progress.min(100);
        self.hours_played = if self.hours_played.is_finite() { self.hours_played.max(0.0) } else { 0.0 };
        self
    }
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LogEntry {
    pub id: i64,
    pub game_id: i64,
    pub at: String,
    pub note: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Settings {
    pub backloggery_username: Option<String>,
    pub steamgriddb_key: Option<String>,
    #[serde(default)]
    pub last_sync: Option<String>,
}

/// A game as imported from Backloggery, already mapped to local types.
#[derive(Debug, Clone)]
pub struct ImportedGame {
    pub backloggery_id: i64,
    pub parent_id: Option<i64>,
    pub title: String,
    pub platform: String,
    pub platform_abbr: Option<String>,
    pub status: Status,
    pub priority: Priority,
    pub ownership: Ownership,
    pub format: Option<String>,
    pub region: Option<String>,
    pub rating: Option<u8>,
    pub achievements_done: i64,
    pub achievements_total: i64,
    pub notes: String,
    pub updated: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SyncReport {
    pub added: u32,
    pub updated: u32,
    pub unchanged: u32,
    /// Updated from Backloggery even though they had local edits.
    pub overwritten_local: u32,
    /// Previously imported games that are no longer on Backloggery (kept locally).
    pub missing_remote: u32,
}

#[derive(Debug, Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CoverReport {
    pub found: u32,
    pub not_found: u32,
    pub failed: u32,
}

#[derive(Debug, Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ImportReport {
    /// New games created.
    pub added: u32,
    /// Linked to a game that was already in the library (same title and platform).
    pub matched: u32,
    /// Previously imported; playtime, cover and last-played refreshed.
    pub updated: u32,
    /// Games you deleted after an earlier import.
    pub skipped: u32,
}

/// Where a library entry came from.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GameSourceLink {
    pub source: String,
    pub external_id: String,
    pub store: Option<String>,
    pub title: String,
    pub playtime_minutes: Option<i64>,
    pub last_played: Option<String>,
    pub imported_at: String,
}
