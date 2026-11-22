//! Box art lookup through SteamGridDB (free API key: steamgriddb.com → Preferences → API).

use reqwest::Url;
use serde::Deserialize;

use crate::error::Result;

const API: &str = "https://www.steamgriddb.com/api/v2";

#[derive(Deserialize)]
struct Response<T> {
    #[serde(default = "Vec::new")]
    data: Vec<T>,
}

#[derive(Deserialize)]
struct SearchHit {
    id: i64,
}

#[derive(Deserialize)]
struct Grid {
    url: String,
}

/// Returns a portrait (2:3) cover URL for the best title match, if any.
pub async fn find_cover(http: &reqwest::Client, api_key: &str, title: &str) -> Result<Option<String>> {
    let mut search = Url::parse(API).expect("valid SteamGridDB base URL");
    search.path_segments_mut().expect("base URL has a path").extend(["search", "autocomplete", title]);

    let hits: Response<SearchHit> =
        http.get(search).bearer_auth(api_key).send().await?.error_for_status()?.json().await?;
    let Some(hit) = hits.data.first() else {
        return Ok(None);
    };

    let grids: Response<Grid> = http
        .get(format!("{API}/grids/game/{}", hit.id))
        .query(&[("dimensions", "600x900,342x482,660x930"), ("types", "static")])
        .bearer_auth(api_key)
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?;
    Ok(grids.data.into_iter().next().map(|g| g.url))
}
