//! Watch the config file and report what changed when it is edited.

use anyhow::{Context, Result};
use notify::{EventKind, RecursiveMode, Watcher};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{channel, Receiver};

/// Sends `()` on the returned channel whenever the config file is created, written or
/// replaced. The parent folder is watched (not the file) because many editors save by
/// writing a temp file and renaming it over the original.
pub fn watch(config: &Path) -> Result<(notify::RecommendedWatcher, Receiver<()>)> {
    let config = std::fs::canonicalize(config).unwrap_or_else(|_| config.to_path_buf());
    let dir = config
        .parent()
        .map(Path::to_path_buf)
        .unwrap_or_else(|| PathBuf::from("."));
    let name = config
        .file_name()
        .map(|n| n.to_string_lossy().to_lowercase())
        .unwrap_or_default();
    let (tx, rx) = channel();
    let mut watcher = notify::recommended_watcher(move |res: notify::Result<notify::Event>| {
        let Ok(event) = res else { return };
        if !matches!(event.kind, EventKind::Create(_) | EventKind::Modify(_)) {
            return;
        }
        let ours = event.paths.iter().any(|p| {
            p.file_name()
                .is_some_and(|n| n.to_string_lossy().to_lowercase() == name)
        });
        if ours {
            let _ = tx.send(());
        }
    })
    .context("starting config watcher")?;
    watcher
        .watch(&dir, RecursiveMode::NonRecursive)
        .with_context(|| format!("watching {}", dir.display()))?;
    Ok((watcher, rx))
}

/// Human readable list of settings that differ between two config texts,
/// e.g. `schedule: "*/15 * * * *" -> "0 * * * *"`.
pub fn changes(old: &str, new: &str) -> Vec<String> {
    let (a, b) = (flatten(old), flatten(new));
    let mut keys: Vec<&String> = a.keys().chain(b.keys()).collect();
    keys.sort();
    keys.dedup();
    keys.into_iter()
        .filter_map(|k| match (a.get(k), b.get(k)) {
            (Some(x), Some(y)) if x == y => None,
            (Some(x), Some(y)) => Some(format!("{k}: {x} -> {y}")),
            (None, Some(y)) => Some(format!("{k}: (default) -> {y}")),
            (Some(x), None) => Some(format!("{k}: {x} -> (default)")),
            (None, None) => None,
        })
        .collect()
}

fn flatten(text: &str) -> BTreeMap<String, String> {
    fn walk(prefix: &str, v: &toml::Value, out: &mut BTreeMap<String, String>) {
        match v {
            toml::Value::Table(t) => {
                for (k, v) in t {
                    let key = if prefix.is_empty() {
                        k.clone()
                    } else {
                        format!("{prefix}.{k}")
                    };
                    walk(&key, v, out);
                }
            }
            other => {
                out.insert(prefix.to_string(), other.to_string());
            }
        }
    }
    let mut out = BTreeMap::new();
    if let Ok(v) = text.parse::<toml::Value>() {
        walk("", &v, &mut out);
    }
    out
}

#[cfg(test)]
mod tests {
    #[test]
    fn diff() {
        let old = "schedule = \"*/15 * * * *\"\ndry_run = true\n[folders]\nmovies = \"Movies\"\n";
        let new = "schedule = \"0 * * * *\"\ndry_run = true\n[folders]\nmovies = \"Films\"\nseries = \"TV\"\n";
        assert_eq!(
            super::changes(old, new),
            vec![
                "folders.movies: \"Movies\" -> \"Films\"",
                "folders.series: (default) -> \"TV\"",
                "schedule: \"*/15 * * * *\" -> \"0 * * * *\"",
            ]
        );
    }
}
