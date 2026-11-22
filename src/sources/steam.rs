//! Steam owned games via the Steam Web API (needs a free API key and a profile
//! whose "Game details" are public).

use std::collections::HashMap;

use serde::Deserialize;

use super::{BoxFuture, FieldKind, Source, SourceContext, SourceField, SourceGame, SourceInfo};
use crate::error::{Error, Result};

pub struct Steam;

const API: &str = "https://api.steampowered.com";

#[derive(Deserialize)]
struct Envelope<T> {
    response: T,
}

#[derive(Deserialize)]
struct Vanity {
    success: i64,
    steamid: Option<String>,
}

#[derive(Deserialize)]
struct Owned {
    game_count: Option<i64>,
    #[serde(default)]
    games: Vec<OwnedGame>,
}

#[derive(Deserialize)]
struct OwnedGame {
    appid: i64,
    name: Option<String>,
    #[serde(default)]
    playtime_forever: i64,
    #[serde(default)]
    rtime_last_played: i64,
}

impl Source for Steam {
    fn info(&self) -> SourceInfo {
        SourceInfo {
            id: "steam".into(),
            name: "Steam".into(),
            description: "Every game you own on Steam, with playtime and box art.".into(),
            fields: vec![
                SourceField::new("apiKey", "Web API key", FieldKind::Secret)
                    .help("Free at https://steamcommunity.com/dev/apikey (any domain name works)."),
                SourceField::new("steamId", "Steam profile", FieldKind::Text)
                    .placeholder("SteamID64, custom URL name, or profile link")
                    .help("\"Game details\" must be Public: https://steamcommunity.com/my/edit/settings"),
            ],
            origin: "built-in".into(),
        }
    }

    fn detect(&self) -> HashMap<String, String> {
        most_recent_login().map(|id| HashMap::from([("steamId".to_string(), id)])).unwrap_or_default()
    }

    fn fetch<'a>(&'a self, ctx: &'a SourceContext) -> BoxFuture<'a, Result<Vec<SourceGame>>> {
        Box::pin(async move {
            let key = ctx.require("apiKey", "Steam Web API key")?;
            let profile = ctx.require("steamId", "Steam profile")?;
            let steam_id = resolve_steam_id(&ctx.http, key, profile).await?;

            let owned: Envelope<Owned> = ctx
                .http
                .get(format!("{API}/IPlayerService/GetOwnedGames/v1/"))
                .query(&[
                    ("key", key),
                    ("steamid", steam_id.as_str()),
                    ("include_appinfo", "1"),
                    ("include_played_free_games", "1"),
                    ("format", "json"),
                ])
                .send()
                .await?
                .error_for_status()
                .map_err(|e| match e.status().map(|s| s.as_u16()) {
                    Some(401 | 403) => Error::msg("Steam rejected the API key."),
                    _ => e.into(),
                })?
                .json()
                .await?;

            if owned.response.game_count.is_none() {
                return Err(Error::msg(
                    "Steam returned no library. Set Profile → Privacy → Game details to Public.",
                ));
            }
            Ok(owned.response.games.into_iter().map(to_source_game).collect())
        })
    }
}

fn to_source_game(g: OwnedGame) -> SourceGame {
    SourceGame {
        external_id: g.appid.to_string(),
        title: g.name.unwrap_or_else(|| format!("Steam app {}", g.appid)),
        platform: "PC".into(),
        store: Some("Steam".into()),
        playtime_minutes: Some(g.playtime_forever),
        last_played: (g.rtime_last_played > 0).then(|| unix_to_sql(g.rtime_last_played)),
        cover_url: Some(format!(
            "https://shared.cloudflare.steamstatic.com/store_item_assets/steam/apps/{}/library_600x900.jpg",
            g.appid
        )),
        ownership: None,
        format: None,
        status: None,
    }
}

async fn resolve_steam_id(http: &reqwest::Client, key: &str, profile: &str) -> Result<String> {
    let profile = profile.trim().trim_end_matches('/');
    if let Some((_, id)) = profile.rsplit_once("/profiles/") {
        return Ok(id.to_string());
    }
    if profile.len() == 17 && profile.bytes().all(|b| b.is_ascii_digit()) {
        return Ok(profile.to_string());
    }
    let vanity = profile.rsplit_once("/id/").map_or(profile, |(_, name)| name);
    let resp: Envelope<Vanity> = http
        .get(format!("{API}/ISteamUser/ResolveVanityURL/v1/"))
        .query(&[("key", key), ("vanityurl", vanity)])
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?;
    match resp.response {
        Vanity { success: 1, steamid: Some(id) } => Ok(id),
        _ => Err(Error::msg(format!("Couldn't find a Steam profile named \"{vanity}\"."))),
    }
}

/// The SteamID64 of the account most recently signed in to the local Steam client.
fn most_recent_login() -> Option<String> {
    let path = r"C:\Program Files (x86)\Steam\config\loginusers.vdf";
    let text = std::fs::read_to_string(path).ok()?;
    let mut current: Option<&str> = None;
    let mut first = None;
    for line in text.lines().map(str::trim) {
        let unquoted = line.trim_matches('"');
        if unquoted.len() == 17 && unquoted.starts_with("7656") && unquoted.bytes().all(|b| b.is_ascii_digit()) {
            current = Some(unquoted);
            first.get_or_insert(unquoted);
        } else if line.starts_with("\"MostRecent\"") && line.ends_with("\"1\"") {
            if let Some(id) = current {
                return Some(id.to_string());
            }
        }
    }
    first.map(str::to_string)
}

/// Unix seconds → `YYYY-MM-DD HH:MM:SS` (UTC), without pulling in a date crate.
pub(super) fn unix_to_sql(secs: i64) -> String {
    let days = secs.div_euclid(86_400);
    let rem = secs.rem_euclid(86_400);
    // Civil-from-days (Howard Hinnant's algorithm).
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    format!("{year:04}-{month:02}-{day:02} {:02}:{:02}:{:02}", rem / 3600, rem % 3600 / 60, rem % 60)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unix_dates() {
        assert_eq!(unix_to_sql(0), "1970-01-01 00:00:00");
        assert_eq!(unix_to_sql(1_709_251_200), "2024-03-01 00:00:00");
        assert_eq!(unix_to_sql(951_782_400 + 3661), "2000-02-29 01:01:01");
    }
}
