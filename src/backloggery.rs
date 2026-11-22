//! Read-only client for Backloggery's (undocumented) web API.
//!
//! The site's own front end loads a public library with
//! `POST /api/fetch_library.php {"type":"load_user_library","username":...}`,
//! which needs no login. The numeric codes below mirror the enums in the
//! site's JavaScript bundle.

use serde::Deserialize;

use crate::error::{Error, Result};
use crate::models::{ImportedGame, Ownership, Priority, Status};

const LIBRARY_URL: &str = "https://backloggery.com/api/fetch_library.php";

#[derive(Debug, Deserialize)]
struct RemoteGame {
    game_inst_id: i64,
    title: String,
    #[serde(default)]
    status: i64,
    #[serde(default)]
    priority: i64,
    #[serde(default)]
    region: i64,
    #[serde(default)]
    achieve_score: Option<i64>,
    #[serde(default)]
    achieve_total: Option<i64>,
    #[serde(default)]
    phys_digi: i64,
    #[serde(default)]
    own: i64,
    notes: Option<String>,
    last_update: Option<String>,
    abbr: Option<String>,
    platform_title: Option<String>,
    sub_platform_title: Option<String>,
    rating: Option<u8>,
    parent_inst_id: Option<i64>,
}

pub async fn fetch_library(http: &reqwest::Client, username: &str) -> Result<Vec<ImportedGame>> {
    let body = serde_json::json!({ "type": "load_user_library", "username": username.trim() });
    let resp = http.post(LIBRARY_URL).json(&body).send().await?.error_for_status()?;
    let value: serde_json::Value = resp.json().await?;

    // A missing or private profile comes back as something other than a list.
    if !value.is_array() {
        return Err(Error::msg(format!(
            "Backloggery didn't return a library for \"{username}\". Check the username and that the profile is public."
        )));
    }
    let games: Vec<RemoteGame> = serde_json::from_value(value)
        .map_err(|e| Error::msg(format!("Unexpected Backloggery response format: {e}")))?;
    Ok(games.into_iter().map(map_game).collect())
}

fn map_game(r: RemoteGame) -> ImportedGame {
    let platform = match (r.platform_title, r.sub_platform_title) {
        (Some(p), Some(sub)) if !sub.is_empty() => format!("{p} ({sub})"),
        (Some(p), _) => p,
        (None, _) => r.abbr.clone().unwrap_or_default(),
    };
    ImportedGame {
        backloggery_id: r.game_inst_id,
        parent_id: r.parent_inst_id,
        title: r.title.trim().to_string(),
        platform,
        platform_abbr: r.abbr,
        status: status(r.status),
        priority: priority(r.priority),
        ownership: ownership(r.own),
        format: format(r.phys_digi).map(str::to_string),
        region: region(r.region).map(str::to_string),
        rating: r.rating.filter(|r| (1..=10).contains(r)),
        achievements_done: r.achieve_score.unwrap_or(0),
        achievements_total: r.achieve_total.unwrap_or(0),
        notes: r.notes.unwrap_or_default(),
        updated: r.last_update,
    }
}

fn status(code: i64) -> Status {
    match code {
        10 => Status::Unplayed,
        20 => Status::Unfinished,
        30 => Status::Beaten,
        40 => Status::Completed,
        60 => Status::Endless,
        _ => Status::NoStatus,
    }
}

fn priority(code: i64) -> Priority {
    match code {
        80 => Priority::NowPlaying,
        70 => Priority::Ongoing,
        60 => Priority::Paused,
        50 => Priority::High,
        30 => Priority::Low,
        20 => Priority::Replay,
        10 => Priority::Shelved,
        _ => Priority::Normal,
    }
}

fn ownership(code: i64) -> Ownership {
    match code {
        1 => Ownership::Owned,
        2 => Ownership::FormerlyOwned,
        3 => Ownership::PlayedIt,
        4 => Ownership::Other,
        5 => Ownership::Household,
        6 => Ownership::Subscription,
        7 => Ownership::Wishlist,
        _ => Ownership::Unknown,
    }
}

fn format(code: i64) -> Option<&'static str> {
    Some(match code {
        1 => "Digital",
        21 => "Physical (Game Only)",
        22 => "Physical (Incomplete)",
        28 => "Physical (Complete In Box)",
        29 => "Physical (Sealed)",
        30 => "Physical (Licensed Repro)",
        31 => "Physical (Unlicensed Repro)",
        20..=39 => "Physical",
        _ => return None,
    })
}

fn region(code: i64) -> Option<&'static str> {
    Some(match code {
        1 => "Region Free",
        2 => "North America",
        3 => "Japan",
        4 => "PAL",
        5 => "China",
        6 => "Korea",
        7 => "Brazil",
        8 => "Asia",
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn maps_a_real_library_row() {
        let row = r#"{"game_inst_id":19733385,"title":"Zeno Clash","status":30,"priority":40,"region":0,
            "achieve_score":16,"achieve_total":22,"platform_id":123,"sub_platform_id":0,"phys_digi":1,"own":1,
            "notes":"[Steam] (2009)","last_update":"2010-12-25 08:22:57","abbr":"PC","sub_abbr":null,
            "platform_title":"PC","sub_platform_title":null,"has_review":0,"rating":null,
            "parent_inst_id":null,"is_parent":0}"#;
        let g = map_game(serde_json::from_str(row).unwrap());
        assert_eq!(g.status, Status::Beaten);
        assert_eq!(g.priority, Priority::Normal);
        assert_eq!(g.ownership, Ownership::Owned);
        assert_eq!(g.format.as_deref(), Some("Digital"));
        assert_eq!(g.region, None);
        assert_eq!((g.achievements_done, g.achievements_total), (16, 22));
        assert_eq!(g.platform, "PC");
    }

    /// Hits the real site: `cargo test -- --ignored live_fetch`
    #[test]
    #[ignore]
    fn live_fetch_public_library() {
        let http = reqwest::Client::new();
        let games = tauri::async_runtime::block_on(fetch_library(&http, "tor")).unwrap();
        assert!(!games.is_empty());
        assert!(games.iter().all(|g| !g.title.is_empty() && !g.platform.is_empty()));
    }
}
