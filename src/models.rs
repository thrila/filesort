//! The trained models (books, movies, series, music) and where their training data
//! comes from.
//!
//! Wikipedia examples are kept in one compact file (see `store.rs`). A label is pulled
//! from Wikipedia again only when it's missing, its categories changed, it holds fewer than
//! `samples_per_label` articles (unless the categories ran out), or a previous pull was
//! interrupted. If Wikipedia can't be reached, whatever is saved is used.

use crate::books;
use crate::classifier::{filename_features, Guess, NaiveBayes};
use crate::config::ClassifierConfig;
use crate::log;
use crate::store::{LabelData, Status, Store};
use crate::wiki::{Article, Wiki};
use std::collections::HashMap;
use std::path::{Path, PathBuf};

pub const KINDS: &[&str] = &["books", "movies", "series", "music"];

pub struct Models {
    models: HashMap<&'static str, NaiveBayes>,
}

impl Models {
    pub fn load(cfg: &ClassifierConfig, books_dir: &Path, wiki: &mut Wiki) -> Self {
        let mut models = HashMap::new();
        let mut announced = false;
        let mut store = Store::open(&cfg.training_file);
        for &kind in KINDS {
            let labels: Vec<String> = if kind == "books" {
                cfg.book_labels()
            } else {
                cfg.model(kind).keys().cloned().collect()
            };
            if labels.is_empty() {
                continue;
            }
            let mut nb = NaiveBayes::new(labels.clone());
            for (label, cats) in cfg.model(kind) {
                let articles =
                    load_or_pull(&mut store, kind, label, cats, cfg, wiki, &mut announced);
                for a in &articles {
                    nb.add(
                        label,
                        &format!("{} {} {}", a.title, a.description, a.categories),
                        1,
                    );
                }
            }
            if kind == "books" {
                books::add_builtin(&mut nb, &labels);
                for (label, folders) in &cfg.local {
                    let n = add_local_documents(&mut nb, label, folders, cfg.samples_per_label);
                    log(&format!(
                        "  training books/{label}: {n} documents from {}",
                        folders
                            .iter()
                            .map(|f| f.display().to_string())
                            .collect::<Vec<_>>()
                            .join(", ")
                    ));
                }
                if cfg.learn_from_library {
                    for label in &labels {
                        add_local_documents(
                            &mut nb,
                            label,
                            &[books_dir.join(label)],
                            cfg.samples_per_label,
                        );
                    }
                }
            }
            let counts = nb.sample_counts();
            let total: usize = counts.iter().map(|c| c.1).sum();
            let empty: Vec<&str> = counts
                .iter()
                .filter(|c| c.1 == 0)
                .map(|c| c.0.as_str())
                .collect();
            if empty.is_empty() {
                log(&format!(
                    "classifier {kind}: {} labels, {total} training examples",
                    labels.len()
                ));
            } else {
                log(&format!(
                    "classifier {kind}: {} labels, {total} training examples; no data yet for {}",
                    labels.len(),
                    empty.join(", ")
                ));
            }
            models.insert(kind, nb);
        }
        if announced {
            log("training data ready");
        }
        Self { models }
    }

    pub fn classify(&self, kind: &str, text: &str) -> Option<Guess> {
        self.models.get(kind)?.classify(text)
    }
}

fn load_or_pull(
    store: &mut Store,
    kind: &str,
    label: &str,
    cats: &[String],
    cfg: &ClassifierConfig,
    wiki: &mut Wiki,
    announced: &mut bool,
) -> Vec<Article> {
    let wanted_cats = cats.join("|");
    let existing = store.get(kind, label).cloned();
    let fresh = existing.as_ref().is_some_and(|f| {
        f.categories == wanted_cats
            && match f.status {
                Status::Complete => f.articles.len() >= cfg.samples_per_label,
                Status::Exhausted => true,
                Status::Partial => false,
            }
    });
    if fresh || !wiki.enabled() {
        return existing.map(|f| f.articles).unwrap_or_default();
    }
    if !*announced {
        *announced = true;
        log(&format!(
            "pulling classifier training data from Wikipedia (up to {} articles per label; first run takes a while, later runs reuse {})",
            cfg.samples_per_label,
            cfg.training_file.display()
        ));
    }
    let shown: Vec<String> = cats
        .iter()
        .filter(|c| !c.starts_with('-'))
        .map(|c| format!("Category:{c}"))
        .collect();
    log(&format!(
        "  training {kind}/{label} from {}",
        shown.join(", ")
    ));
    let want = cfg.samples_per_label;
    let (articles, err) = wiki.harvest(cats, want, cfg.max_depth, |n| {
        log(&format!("  training {kind}/{label}: {n}/{want}"));
    });
    let status = match (&err, articles.len() >= want) {
        (Some(_), _) => Status::Partial,
        (None, true) => Status::Complete,
        (None, false) => Status::Exhausted,
    };
    if let Some(e) = &err {
        log(&format!(
            "  training {kind}/{label}: Wikipedia stopped us ({e:#}); will finish on a later start"
        ));
    }
    // Never replace a bigger saved set with a smaller interrupted one.
    if let Some(old) = existing {
        if status == Status::Partial && old.articles.len() >= articles.len() {
            return old.articles;
        }
    }
    let data = LabelData {
        categories: wanted_cats,
        status,
        articles,
    };
    if let Err(e) = store.put(kind, label, data.clone()) {
        log(&format!(
            "  could not save training data {}: {e:#}",
            cfg.training_file.display()
        ));
    }
    data.articles
}

/// Hold out every 5th saved Wikipedia example per label, train on the rest, and report how
/// often the held-out examples are classified correctly, from the title alone (like a bare
/// file name) and from title + description + categories (like a file whose Wikipedia
/// article was found). Uses only the saved training files; nothing is downloaded.
pub fn evaluate(cfg: &ClassifierConfig) {
    let store = Store::open(&cfg.training_file);
    let threshold = cfg.min_confidence;
    for &kind in KINDS {
        let labels: Vec<String> = cfg.model(kind).keys().cloned().collect();
        if labels.is_empty() {
            continue;
        }
        let mut nb = NaiveBayes::new(labels.clone());
        let mut test: Vec<(String, Article)> = Vec::new();
        for label in &labels {
            let saved = store
                .get(kind, label)
                .map(|d| d.articles.clone())
                .unwrap_or_default();
            for (i, a) in saved.into_iter().enumerate() {
                if i % 5 == 4 {
                    test.push((label.clone(), a));
                } else {
                    nb.add(
                        label,
                        &format!("{} {} {}", a.title, a.description, a.categories),
                        1,
                    );
                }
            }
        }
        if kind == "books" {
            books::add_builtin(&mut nb, &labels);
        }
        if test.is_empty() {
            println!(
                "{kind}: no saved training data in {}",
                cfg.training_file.display()
            );
            continue;
        }
        let mut per: HashMap<&str, (usize, usize, usize)> = HashMap::new();
        let (mut title_ok, mut full_ok) = (0, 0);
        // At the "leave it where it is" threshold: how many are sure enough, and how many of
        // those are right.
        let (mut t_sure, mut t_sure_ok, mut f_sure, mut f_sure_ok) = (0, 0, 0, 0);
        for (label, a) in &test {
            let tg = nb.classify(&a.title);
            let fg = nb.classify(&format!("{} {} {}", a.title, a.description, a.categories));
            let t = tg.as_ref().is_some_and(|g| g.label == *label);
            let f = fg.as_ref().is_some_and(|g| g.label == *label);
            if tg.as_ref().is_some_and(|g| g.confidence >= threshold) {
                t_sure += 1;
                t_sure_ok += t as usize;
            }
            if fg.as_ref().is_some_and(|g| g.confidence >= threshold) {
                f_sure += 1;
                f_sure_ok += f as usize;
            }
            title_ok += t as usize;
            full_ok += f as usize;
            let e = per.entry(label.as_str()).or_default();
            e.0 += 1;
            e.1 += t as usize;
            e.2 += f as usize;
        }
        let pct = |a: usize, b: usize| {
            if b == 0 {
                0.0
            } else {
                100.0 * a as f64 / b as f64
            }
        };
        println!(
            "\n{kind}: {} held-out examples, {} labels (chance = {:.0}%)",
            test.len(),
            labels.len(),
            100.0 / labels.len() as f64
        );
        println!(
            "  {:<22} {:>6} {:>12} {:>16}",
            "label", "tested", "title only", "title + wiki"
        );
        for label in &labels {
            if let Some(&(n, t, f)) = per.get(label.as_str()) {
                println!(
                    "  {:<22} {:>6} {:>11.0}% {:>15.0}%",
                    label,
                    n,
                    pct(t, n),
                    pct(f, n)
                );
            }
        }
        println!(
            "  {:<22} {:>6} {:>11.0}% {:>15.0}%",
            "ALL",
            test.len(),
            pct(title_ok, test.len()),
            pct(full_ok, test.len())
        );
        println!(
            "  at >= {:.0}% confidence: title only moves {:.0}% of files ({:.0}% of those right); title + wiki moves {:.0}% ({:.0}% right)",
            threshold * 100.0,
            pct(t_sure, test.len()),
            pct(t_sure_ok, t_sure),
            pct(f_sure, test.len()),
            pct(f_sure_ok, f_sure)
        );
    }
}

/// Use the names of the documents in `folders` (subfolders included, software projects
/// skipped) as training examples for `label`. Returns how many were added.
fn add_local_documents(
    nb: &mut NaiveBayes,
    label: &str,
    folders: &[PathBuf],
    limit: usize,
) -> usize {
    use crate::media::{is_code_project, kind_of, parse_book, Kind};
    let mut n = 0;
    for folder in folders {
        let walker = walkdir::WalkDir::new(folder)
            .max_depth(8)
            .into_iter()
            .filter_entry(|e| {
                !e.file_type().is_dir() || e.depth() == 0 || !is_code_project(e.path())
            });
        for e in walker.flatten() {
            if n >= limit {
                return n;
            }
            if !e.file_type().is_file() || !matches!(kind_of(e.path()), Kind::Book | Kind::Document)
            {
                continue;
            }
            let stem = e
                .path()
                .file_stem()
                .unwrap_or_default()
                .to_string_lossy()
                .to_string();
            let b = parse_book(&stem);
            let meta = crate::docmeta::read(e.path())
                .map(|m| m.classifier_text())
                .unwrap_or_default();
            let text = format!(
                "{} {} {} {meta}",
                b.title,
                b.author.unwrap_or_default(),
                filename_features(&stem)
            );
            nb.add(label, &text, 1);
            n += 1;
        }
    }
    n
}

/// Resolve a relative training_file against the folder holding the config file.
pub fn resolve_training_file(dir: &Path, config_path: &Path) -> PathBuf {
    if dir.is_absolute() {
        return dir.to_path_buf();
    }
    let base = config_path
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    base.join(dir)
}
