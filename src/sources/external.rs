//! External plugins: any program, in any language, that prints a library as JSON.
//!
//! A plugin is a folder in the app's `plugins` directory containing `plugin.json`:
//!
//! ```json
//! {
//!   "id": "my-store",
//!   "name": "My Store",
//!   "description": "What it imports.",
//!   "command": ["powershell.exe", "-NoProfile", "-File", "import.ps1"],
//!   "fields": [{ "key": "token", "label": "API token", "kind": "secret" }]
//! }
//! ```
//!
//! The command runs with the plugin folder as its working directory and
//! receives `{"config": {<field key>: <value>, ...}}` on stdin. It must print a
//! JSON array of games (see `SourceGame`) to stdout and exit with status 0;
//! on failure, whatever it wrote to stderr is shown to the user.

use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;

use serde::Deserialize;
use tokio::io::AsyncWriteExt;

use super::{BoxFuture, Source, SourceContext, SourceField, SourceGame, SourceInfo};
use crate::error::{Error, Result};

const TIMEOUT: Duration = Duration::from_secs(300);

/// Plugins that ship with the app, installed into the plugins folder on first run.
const BUNDLED: &[(&str, &[(&str, &str)])] = &[(
    "xbox-pc",
    &[
        ("plugin.json", include_str!("../../plugins/xbox-pc/plugin.json")),
        ("import.ps1", include_str!("../../plugins/xbox-pc/import.ps1")),
    ],
)];

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Manifest {
    id: String,
    name: String,
    #[serde(default)]
    description: String,
    command: Vec<String>,
    #[serde(default)]
    fields: Vec<SourceField>,
}

pub struct ExternalPlugin {
    dir: PathBuf,
    manifest: Manifest,
}

/// Creates the plugins folder with the bundled plugins if it doesn't exist yet.
/// Deleting a bundled plugin afterwards is respected.
pub fn install_bundled(plugins_dir: &Path) -> std::io::Result<()> {
    if plugins_dir.exists() {
        return Ok(());
    }
    for (name, files) in BUNDLED {
        let dir = plugins_dir.join(name);
        std::fs::create_dir_all(&dir)?;
        for (file, contents) in *files {
            std::fs::write(dir.join(file), contents)?;
        }
    }
    Ok(())
}

pub fn scan(plugins_dir: &Path) -> Vec<ExternalPlugin> {
    let Ok(entries) = std::fs::read_dir(plugins_dir) else { return Vec::new() };
    let mut plugins: Vec<ExternalPlugin> = entries
        .flatten()
        .filter_map(|e| {
            let dir = e.path();
            let text = std::fs::read_to_string(dir.join("plugin.json")).ok()?;
            match serde_json::from_str::<Manifest>(&text) {
                Ok(manifest) if !manifest.command.is_empty() && !manifest.id.is_empty() => {
                    Some(ExternalPlugin { dir, manifest })
                }
                Ok(_) => None,
                Err(e) => {
                    eprintln!("skipping plugin {}: invalid plugin.json: {e}", dir.display());
                    None
                }
            }
        })
        .collect();
    plugins.sort_by(|a, b| a.manifest.name.cmp(&b.manifest.name));
    plugins
}

impl Source for ExternalPlugin {
    fn info(&self) -> SourceInfo {
        SourceInfo {
            id: self.manifest.id.clone(),
            name: self.manifest.name.clone(),
            description: self.manifest.description.clone(),
            fields: self.manifest.fields.clone(),
            origin: self.dir.display().to_string(),
        }
    }

    fn fetch<'a>(&'a self, ctx: &'a SourceContext) -> BoxFuture<'a, Result<Vec<SourceGame>>> {
        Box::pin(async move {
            let (program, args) = self.manifest.command.split_first().expect("checked in scan");
            // Resolve programs shipped inside the plugin folder; otherwise use PATH.
            let local = self.dir.join(program);
            let program = if local.is_file() { local.into_os_string() } else { program.into() };

            let mut cmd = tokio::process::Command::new(program);
            cmd.args(args)
                .current_dir(&self.dir)
                .stdin(Stdio::piped())
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .kill_on_drop(true);
            #[cfg(windows)]
            cmd.creation_flags(0x0800_0000); // CREATE_NO_WINDOW

            let name = &self.manifest.name;
            let mut child =
                cmd.spawn().map_err(|e| Error::msg(format!("Couldn't start the {name} plugin: {e}")))?;

            let input = serde_json::json!({ "config": ctx.config }).to_string();
            if let Some(mut stdin) = child.stdin.take() {
                // A plugin that ignores stdin may close it early; that's fine.
                let _ = stdin.write_all(input.as_bytes()).await;
            }

            let output = tokio::time::timeout(TIMEOUT, child.wait_with_output())
                .await
                .map_err(|_| Error::msg(format!("The {name} plugin took longer than 5 minutes and was stopped.")))?
                .map_err(|e| Error::msg(format!("The {name} plugin failed: {e}")))?;

            if !output.status.success() {
                let stderr = String::from_utf8_lossy(&output.stderr);
                let lines: Vec<&str> = stderr.trim().lines().collect();
                let tail = lines[lines.len().saturating_sub(5)..].join(" ");
                let detail = if tail.is_empty() { "no error output" } else { &tail };
                return Err(Error::msg(format!("The {name} plugin failed ({}): {detail}", output.status)));
            }
            parse_output(&output.stdout).map_err(|e| Error::msg(format!("The {name} plugin printed invalid JSON: {e}")))
        })
    }
}

/// Accepts either a bare array or `{"games": [...]}`.
fn parse_output(stdout: &[u8]) -> serde_json::Result<Vec<SourceGame>> {
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum Output {
        List(Vec<SourceGame>),
        Wrapped { games: Vec<SourceGame> },
    }
    let text = String::from_utf8_lossy(stdout);
    let text = text.trim_start_matches('\u{feff}').trim();
    if text.is_empty() {
        return Ok(Vec::new());
    }
    Ok(match serde_json::from_str(text)? {
        Output::List(games) | Output::Wrapped { games } => games,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn output_shapes() {
        let bare = br#"[{"externalId":"1","title":"Halo","platform":"PC","playtimeMinutes":12}]"#;
        assert_eq!(parse_output(bare).unwrap()[0].playtime_minutes, Some(12));
        let wrapped = b"\xEF\xBB\xBF{\"games\":[{\"externalId\":\"2\",\"title\":\"Gears\"}]}";
        assert_eq!(parse_output(wrapped).unwrap()[0].title, "Gears");
        assert!(parse_output(b"").unwrap().is_empty());
        assert!(parse_output(b"nope").is_err());
    }

    #[test]
    fn bundled_manifests_are_valid() {
        for (_, files) in BUNDLED {
            let manifest: Manifest = serde_json::from_str(files[0].1).unwrap();
            assert!(!manifest.command.is_empty());
        }
    }
}
