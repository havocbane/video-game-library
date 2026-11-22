//! Library import sources ("plugins").
//!
//! Every store integration implements [`Source`] and returns a flat list of
//! [`SourceGame`]s; the database layer owns matching and merging. Sources come
//! in two flavours:
//!
//! - built-in Rust sources (Steam, GOG Galaxy, Epic, PlayStation, CSV), and
//! - external plugins: a folder with a `plugin.json` manifest and any
//!   executable that prints `SourceGame` JSON to stdout (see `external.rs`).

mod csv_file;
mod epic;
pub mod external;
mod galaxy;
mod psn;
mod steam;

use std::collections::HashMap;
use std::future::Future;
use std::path::Path;
use std::pin::Pin;
use std::sync::Arc;

use serde::{Deserialize, Serialize};

use crate::error::{Error, Result};
use crate::models::{Ownership, Status};

pub type BoxFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

/// One game as reported by a source. This is also the JSON contract for
/// external plugins, so field names are camelCase and most are optional.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SourceGame {
    /// Stable id within the source (app id, release key, ...).
    pub external_id: String,
    pub title: String,
    /// Defaults to "PC".
    #[serde(default)]
    pub platform: String,
    /// Storefront shown in the UI, e.g. "Steam" or "Epic".
    pub store: Option<String>,
    pub playtime_minutes: Option<i64>,
    /// `YYYY-MM-DD HH:MM:SS` (UTC) or any ISO-8601 prefix.
    pub last_played: Option<String>,
    pub cover_url: Option<String>,
    pub ownership: Option<Ownership>,
    /// Defaults to "Digital".
    pub format: Option<String>,
    /// Only used when the game is first added; never overwrites your status.
    pub status: Option<Status>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum FieldKind {
    Text,
    Secret,
    Path,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SourceField {
    pub key: String,
    pub label: String,
    #[serde(default = "default_kind")]
    pub kind: FieldKind,
    #[serde(default)]
    pub placeholder: Option<String>,
    #[serde(default)]
    pub help: Option<String>,
    #[serde(default)]
    pub optional: bool,
}

fn default_kind() -> FieldKind {
    FieldKind::Text
}

impl SourceField {
    fn new(key: &str, label: &str, kind: FieldKind) -> Self {
        Self { key: key.into(), label: label.into(), kind, placeholder: None, help: None, optional: false }
    }
    fn placeholder(mut self, s: &str) -> Self {
        self.placeholder = Some(s.into());
        self
    }
    fn help(mut self, s: &str) -> Self {
        self.help = Some(s.into());
        self
    }
    fn optional(mut self) -> Self {
        self.optional = true;
        self
    }
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SourceInfo {
    pub id: String,
    pub name: String,
    pub description: String,
    pub fields: Vec<SourceField>,
    /// "built-in" or the plugin folder path.
    pub origin: String,
}

pub struct SourceContext {
    pub http: reqwest::Client,
    pub config: HashMap<String, String>,
}

impl SourceContext {
    pub fn get(&self, key: &str) -> Option<&str> {
        self.config.get(key).map(|s| s.trim()).filter(|s| !s.is_empty())
    }

    pub fn require(&self, key: &str, label: &str) -> Result<&str> {
        self.get(key).ok_or_else(|| Error::msg(format!("{label} is required.")))
    }
}

pub trait Source: Send + Sync {
    fn info(&self) -> SourceInfo;

    /// Values found on this machine (install paths, account ids) used to
    /// pre-fill empty settings.
    fn detect(&self) -> HashMap<String, String> {
        HashMap::new()
    }

    fn fetch<'a>(&'a self, ctx: &'a SourceContext) -> BoxFuture<'a, Result<Vec<SourceGame>>>;
}

pub fn builtin_sources() -> Vec<Arc<dyn Source>> {
    vec![
        Arc::new(steam::Steam),
        Arc::new(galaxy::GogGalaxy),
        Arc::new(epic::Epic),
        Arc::new(psn::PlayStation),
        Arc::new(csv_file::CsvFile),
    ]
}

/// Built-in sources followed by external plugins found in `plugins_dir`.
/// Plugins are rescanned on every call so new ones show up without a restart.
pub fn all_sources(builtins: &[Arc<dyn Source>], plugins_dir: &Path) -> Vec<Arc<dyn Source>> {
    let mut all: Vec<Arc<dyn Source>> = builtins.to_vec();
    for plugin in external::scan(plugins_dir) {
        if all.iter().any(|s| s.info().id == plugin.info().id) {
            continue; // never let a plugin shadow a built-in
        }
        all.push(Arc::new(plugin));
    }
    all
}

// ---------- matching ----------

/// Trailing edition names that don't make a different game.
/// Longer phrases come first so they win over their own tails.
const EDITION_SUFFIXES: &[&str] = &[
    "game of the year edition", "game of the year", "goty edition", "goty", "definitive edition",
    "the complete edition", "complete edition", "digital deluxe edition", "deluxe edition",
    "ultimate edition", "gold edition", "enhanced edition", "directors cut", "special edition",
    "standard edition", "anniversary edition", "pc edition", "windows edition", "for windows",
];

/// Normalizes a title so the same game from different stores compares equal:
/// case, punctuation, trademark symbols and edition suffixes are ignored.
pub fn normalize_title(title: &str) -> String {
    let mut s = String::with_capacity(title.len());
    let mut last_space = true;
    for ch in title.chars() {
        let ch = match ch {
            '&' => {
                if !last_space {
                    s.push(' ');
                }
                s.push_str("and");
                last_space = false;
                continue;
            }
            '\'' | '’' | '™' | '®' | '©' => continue,
            c => c,
        };
        if ch.is_alphanumeric() {
            s.extend(ch.to_lowercase());
            last_space = false;
        } else if !last_space {
            s.push(' ');
            last_space = true;
        }
    }
    let mut s = s.trim_end().to_string();
    loop {
        let before = s.len();
        for suffix in EDITION_SUFFIXES {
            if let Some(rest) = s.strip_suffix(suffix) {
                if rest.ends_with(' ') && rest.trim().len() > 0 {
                    s = rest.trim_end().to_string();
                    break;
                }
            }
        }
        if s.len() == before {
            return s;
        }
    }
}

/// Groups platform names that are the same machine ("PC (Steam)", "Windows" → "pc").
pub fn platform_family(platform: &str) -> String {
    let p = platform.to_lowercase();
    let has = |needles: &[&str]| needles.iter().any(|n| p.contains(n));
    if p.is_empty() || p == "pc" || p.starts_with("pc ") || has(&["windows", "steam", "gog", "epic", "linux", "mac"]) {
        "pc".into()
    } else if has(&["playstation 5", "ps5"]) {
        "ps5".into()
    } else if has(&["playstation 4", "ps4"]) {
        "ps4".into()
    } else if has(&["series x", "series s", "xbox series"]) {
        "xbox-series".into()
    } else if has(&["xbox one"]) {
        "xbox-one".into()
    } else if has(&["switch 2"]) {
        "switch2".into()
    } else if has(&["switch"]) {
        "switch".into()
    } else {
        normalize_title(&p)
    }
}

/// Generic console names (as GOG Galaxy reports them) and the specific
/// generations they could mean.
const CONSOLE_LINES: &[(&str, &[&str])] = &[("playstation", &["ps5", "ps4"]), ("xbox", &["xbox-series", "xbox-one"])];

/// Families a game on `family` may match: itself, then the specific
/// generations of a generic console name, or the generic name of a specific one.
fn compatible_families(family: &str) -> Vec<&str> {
    let mut out = vec![family];
    for (generic, specific) in CONSOLE_LINES {
        if family == *generic {
            out.extend_from_slice(specific);
        } else if specific.contains(&family) {
            out.push(generic);
        }
    }
    out
}

/// Existing games keyed by normalized title and platform family.
#[derive(Default)]
pub struct MatchIndex {
    games: HashMap<(String, String), i64>,
}

impl MatchIndex {
    /// The first game inserted for a key wins.
    pub fn insert(&mut self, title: &str, platform: &str, id: i64) {
        self.games.entry((normalize_title(title), platform_family(platform))).or_insert(id);
    }

    pub fn find(&self, title: &str, platform: &str) -> Option<i64> {
        let title = normalize_title(title);
        let family = platform_family(platform);
        compatible_families(&family).into_iter().find_map(|f| self.games.get(&(title.clone(), f.to_string())).copied())
    }

    pub fn remove_game(&mut self, id: i64) {
        self.games.retain(|_, v| *v != id);
    }
}

/// Parses ISO-8601 durations like `PT12H3M4S` (PlayStation) into minutes.
pub fn iso_duration_minutes(s: &str) -> Option<i64> {
    let rest = s.strip_prefix("PT").or_else(|| s.strip_prefix('P'))?;
    let (mut total, mut num) = (0f64, String::new());
    for ch in rest.chars() {
        match ch {
            '0'..='9' | '.' => num.push(ch),
            'T' => {}
            unit => {
                let n: f64 = num.parse().ok()?;
                num.clear();
                total += match unit {
                    'D' => n * 1440.0,
                    'H' => n * 60.0,
                    'M' => n,
                    'S' => n / 60.0,
                    _ => return None,
                };
            }
        }
    }
    Some(total.round() as i64)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn titles_match_across_stores() {
        assert_eq!(normalize_title("DOOM® Eternal"), normalize_title("Doom Eternal"));
        assert_eq!(normalize_title("Dark Deity: Complete Edition"), "dark deity");
        assert_eq!(normalize_title("The Witcher 3: Wild Hunt - Game of the Year Edition"), "the witcher 3 wild hunt");
        assert_eq!(normalize_title("Ori & the Blind Forest"), "ori and the blind forest");
        assert_eq!(normalize_title("Assassin’s Creed"), "assassins creed");
        // An edition word alone is still a title.
        assert_eq!(normalize_title("Complete"), "complete");
    }

    #[test]
    fn platform_families() {
        assert_eq!(platform_family("PC (Steam)"), "pc");
        assert_eq!(platform_family("PC"), "pc");
        assert_eq!(platform_family("PlayStation 5"), "ps5");
        assert_eq!(platform_family("PS5"), "ps5");
        assert_eq!(platform_family("Xbox Series X|S"), "xbox-series");
        assert_eq!(platform_family("Nintendo Switch"), "switch");
        assert_eq!(platform_family("Game Boy Advance"), "game boy advance");
    }

    #[test]
    fn generic_consoles_match_specific_generations() {
        let mut index = MatchIndex::default();
        index.insert("Returnal", "PlayStation 5", 1);
        index.insert("Halo 5", "Xbox One", 2);
        index.insert("Hades", "PC", 3);
        assert_eq!(index.find("Returnal", "PlayStation"), Some(1));
        assert_eq!(index.find("Halo 5: Guardians", "Xbox"), None);
        assert_eq!(index.find("Halo 5", "Xbox"), Some(2));
        assert_eq!(index.find("Hades", "PlayStation"), None);
        // Specific generations don't match each other.
        assert_eq!(index.find("Returnal", "PlayStation 4"), None);

        let mut index = MatchIndex::default();
        index.insert("Returnal", "PlayStation", 4);
        assert_eq!(index.find("Returnal", "PS5"), Some(4));
        index.remove_game(4);
        assert_eq!(index.find("Returnal", "PS5"), None);
    }

    #[test]
    fn durations() {
        assert_eq!(iso_duration_minutes("PT12H3M30S"), Some(724));
        assert_eq!(iso_duration_minutes("PT45M"), Some(45));
        assert_eq!(iso_duration_minutes("P1DT1H"), Some(1500));
        assert_eq!(iso_duration_minutes("nope"), None);
    }
}
