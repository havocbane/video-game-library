//! PlayStation games you've played (PS4/PS5), with playtime, via the same
//! endpoints the PlayStation App uses. Authenticates with an NPSSO token the
//! user copies from their own signed-in browser session.

use serde::Deserialize;

use super::{BoxFuture, FieldKind, Source, SourceContext, SourceField, SourceGame, SourceInfo, iso_duration_minutes};
use crate::error::{Error, Result};

pub struct PlayStation;

// Public client credentials of the PlayStation App (as used by psnawp and similar tools).
const CLIENT_ID: &str = "09515159-7237-4370-9b40-3806e67c0891";
const CLIENT_BASIC_AUTH: &str = "Basic MDk1MTUxNTktNzIzNy00MzcwLTliNDAtMzgwNmU2N2MwODkxOnVjUGprYTV0bnRCMktxc1A=";
const REDIRECT_URI: &str = "com.scee.psxandroid.scecompcall://redirect";
const AUTH_BASE: &str = "https://ca.account.sony.com/api/authz/v3/oauth";
const GAMELIST: &str = "https://m.np.playstation.com/api/gamelist/v2/users/me/titles";
const PAGE: usize = 200;

#[derive(Deserialize)]
struct Token {
    access_token: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Page {
    #[serde(default)]
    titles: Vec<Title>,
    total_item_count: Option<usize>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Title {
    title_id: String,
    name: String,
    image_url: Option<String>,
    category: Option<String>,
    play_duration: Option<String>,
    last_played_date_time: Option<String>,
}

impl Source for PlayStation {
    fn info(&self) -> SourceInfo {
        SourceInfo {
            id: "psn".into(),
            name: "PlayStation".into(),
            description: "PS4 and PS5 games you've played, with playtime. Uses PlayStation's unofficial app API."
                .into(),
            fields: vec![SourceField::new("npsso", "NPSSO token", FieldKind::Secret).help(
                "Sign in at https://www.playstation.com, then in the same browser open \
                 https://ca.account.sony.com/api/v1/ssocookie (\"ca\" is Sony's account server, not Canada; it \
                 works in every region) and copy the value after \"npsso\", without the quotes. It expires after \
                 about two months.",
            )],
            origin: "built-in".into(),
        }
    }

    fn fetch<'a>(&'a self, ctx: &'a SourceContext) -> BoxFuture<'a, Result<Vec<SourceGame>>> {
        Box::pin(async move {
            let npsso = ctx.require("npsso", "NPSSO token")?;
            let token = access_token(npsso).await?;

            let mut games = Vec::new();
            let mut offset = 0;
            loop {
                let page: Page = ctx
                    .http
                    .get(GAMELIST)
                    .bearer_auth(&token)
                    .query(&[
                        ("categories", "ps4_game,ps5_native_game"),
                        ("limit", &PAGE.to_string()),
                        ("offset", &offset.to_string()),
                    ])
                    .send()
                    .await?
                    .error_for_status()?
                    .json()
                    .await?;
                let count = page.titles.len();
                games.extend(page.titles.into_iter().map(to_source_game));
                offset += count;
                if count < PAGE || page.total_item_count.is_some_and(|total| offset >= total) {
                    break;
                }
            }
            Ok(games)
        })
    }
}

async fn access_token(npsso: &str) -> Result<String> {
    // The authorize step answers with a redirect carrying the code; don't follow it.
    let http = reqwest::Client::builder().redirect(reqwest::redirect::Policy::none()).build()?;
    let resp = http
        .get(format!("{AUTH_BASE}/authorize"))
        .query(&[
            ("access_type", "offline"),
            ("client_id", CLIENT_ID),
            ("redirect_uri", REDIRECT_URI),
            ("response_type", "code"),
            ("scope", "psn:mobile.v2.core psn:clientapp"),
        ])
        .header("Cookie", format!("npsso={npsso}"))
        .send()
        .await?;

    let code = resp
        .headers()
        .get(reqwest::header::LOCATION)
        .and_then(|l| l.to_str().ok())
        .and_then(|l| reqwest::Url::parse(l).ok())
        .and_then(|url| url.query_pairs().find(|(k, _)| k == "code").map(|(_, v)| v.into_owned()))
        .ok_or_else(|| Error::msg("PlayStation rejected the NPSSO token. Copy a fresh one and try again."))?;

    let token: Token = http
        .post(format!("{AUTH_BASE}/token"))
        .header("Authorization", CLIENT_BASIC_AUTH)
        .form(&[
            ("code", code.as_str()),
            ("redirect_uri", REDIRECT_URI),
            ("grant_type", "authorization_code"),
            ("token_format", "jwt"),
        ])
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?;
    Ok(token.access_token)
}

fn to_source_game(t: Title) -> SourceGame {
    let platform = match t.category.as_deref() {
        Some("ps5_native_game") => "PlayStation 5",
        _ => "PlayStation 4",
    };
    SourceGame {
        external_id: t.title_id,
        title: t.name,
        platform: platform.into(),
        store: Some("PlayStation".into()),
        playtime_minutes: t.play_duration.as_deref().and_then(iso_duration_minutes),
        last_played: t.last_played_date_time.map(|d| d.replace('T', " ").trim_end_matches('Z').chars().take(19).collect()),
        cover_url: t.image_url,
        ownership: None,
        format: None,
        status: None,
    }
}
