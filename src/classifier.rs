//! Multinomial Naive Bayes over bag-of-words, as described in book_sorter.md:
//!
//!   score(class) = Σ count(token) · log((count(token, class) + α) / (total(class) + α·|V|))
//!
//! then a softmax over the class scores gives a confidence.

use crate::media::tokens;
use std::collections::{HashMap, HashSet};

const ALPHA: f64 = 1.0;

const STOP: &[&str] = &[
    "a",
    "an",
    "and",
    "as",
    "at",
    "be",
    "but",
    "by",
    "for",
    "from",
    "have",
    "has",
    "he",
    "her",
    "his",
    "i",
    "if",
    "in",
    "into",
    "is",
    "it",
    "its",
    "me",
    "more",
    "most",
    "my",
    "no",
    "not",
    "of",
    "on",
    "or",
    "our",
    "over",
    "s",
    "she",
    "so",
    "than",
    "that",
    "the",
    "their",
    "them",
    "then",
    "there",
    "these",
    "they",
    "this",
    "those",
    "to",
    "under",
    "up",
    "via",
    "was",
    "we",
    "were",
    "what",
    "when",
    "where",
    "which",
    "who",
    "why",
    "with",
    "without",
    "would",
    "you",
    "your",
    "pdf",
    "epub",
    "mobi",
    "azw3",
    "djvu",
    "edition",
    "ed",
    "vol",
    "volume",
    "part",
    "complete",
    "final",
    "release",
    "etc",
    "lib",
    "library",
    "zlib",
    "sk",
    "org",
    "com",
    "wikipedia",
    "also",
    "known",
    "written",
    "published",
    "first",
    "mkv",
    "mp4",
    "avi",
    "mp3",
    "flac",
    "m4a",
];

#[derive(Default)]
struct Profile {
    counts: HashMap<String, u32>,
    total: u64,
}

pub struct NaiveBayes {
    labels: Vec<String>,
    profiles: Vec<Profile>,
    vocab: HashSet<String>,
    samples: Vec<usize>,
}

#[derive(Debug, Clone)]
pub struct Guess {
    pub label: String,
    pub confidence: f64,
}

/// Lowercase words, accents folded, camelCase split, stop words / numbers / 1-letter words removed.
pub fn words(text: &str) -> Vec<String> {
    tokens(text)
        .into_iter()
        .filter(|t| {
            t.len() > 1 && !t.chars().all(|c| c.is_ascii_digit()) && !STOP.contains(&t.as_str())
        })
        .collect()
}

/// Words describing the *shape* of a file name, so the model can learn that e.g. research
/// papers are named "biegler1983" (author + year) or "9404236v1" (arXiv id) even though
/// such names hold no topic words.
pub fn filename_features(stem: &str) -> String {
    use regex::Regex;
    use std::sync::LazyLock;
    static AUTHOR_YEAR: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(
            r"(?i)^[a-z][a-z'-]+(?:[ _-]?(?:et[ _]?al|and[ _][a-z]+))?[ _-]?(?:19|20)\d{2}[a-z]?$",
        )
        .unwrap()
    });
    static ARXIV: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(r"(?i)^(?:arxiv[ _.:-]?)?(?:\d{4}\.\d{4,5}|[a-z-]+/\d{7}|\d{7})(?:v\d+)?$")
            .unwrap()
    });
    static DOI: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?i)^10\.\d{4,9}[_/-]").unwrap());
    static UUIDISH: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r"(?i)^[0-9a-f]{8}-[0-9a-f]{4}-").unwrap());
    let stem = stem.trim();
    let mut f = Vec::new();
    if AUTHOR_YEAR.is_match(stem) {
        f.push("shapeauthoryear");
    }
    if ARXIV.is_match(stem) {
        f.push("shapearxiv");
    }
    if DOI.is_match(stem) {
        f.push("shapedoi");
    }
    if UUIDISH.is_match(stem) {
        f.push("shapeuuid");
    }
    if stem.chars().filter(char::is_ascii_digit).count() * 2 > stem.len() {
        f.push("shapemostlydigits");
    }
    f.join(" ")
}

impl NaiveBayes {
    pub fn new(labels: Vec<String>) -> Self {
        let n = labels.len();
        Self {
            labels,
            profiles: (0..n).map(|_| Profile::default()).collect(),
            vocab: HashSet::new(),
            samples: vec![0; n],
        }
    }

    /// Add one training document for `label` (case-insensitive), counted `weight` times.
    pub fn add(&mut self, label: &str, text: &str, weight: u32) {
        let Some(i) = self
            .labels
            .iter()
            .position(|l| l.eq_ignore_ascii_case(label))
        else {
            return;
        };
        let ws = words(text);
        if ws.is_empty() {
            return;
        }
        self.samples[i] += 1;
        let p = &mut self.profiles[i];
        for w in ws {
            *p.counts.entry(w.clone()).or_default() += weight;
            p.total += weight as u64;
            self.vocab.insert(w);
        }
    }

    /// Number of documents added per label.
    pub fn sample_counts(&self) -> Vec<(String, usize)> {
        self.labels
            .iter()
            .cloned()
            .zip(self.samples.iter().copied())
            .collect()
    }

    pub fn classify(&self, text: &str) -> Option<Guess> {
        let ws = words(text);
        let trained: Vec<usize> = (0..self.labels.len())
            .filter(|&i| self.profiles[i].total > 0)
            .collect();
        if ws.is_empty() || trained.is_empty() {
            return None;
        }
        let v = self.vocab.len().max(1) as f64;
        let scores: Vec<(usize, f64)> = trained
            .iter()
            .map(|&i| {
                let p = &self.profiles[i];
                let denom = p.total as f64 + ALPHA * v;
                let s = ws
                    .iter()
                    .map(|w| ((*p.counts.get(w).unwrap_or(&0) as f64 + ALPHA) / denom).ln())
                    .sum::<f64>();
                (i, s)
            })
            .collect();
        let &(best, top) = scores.iter().max_by(|a, b| a.1.total_cmp(&b.1))?;
        // Softmax. Log scores are scaled by 1/sqrt(word count) so that long inputs
        // (filename + a Wikipedia intro) don't all come out at 1.00 confidence.
        let scale = 1.0 / (ws.len() as f64).sqrt().max(1.0);
        let total: f64 = scores.iter().map(|(_, s)| ((s - top) * scale).exp()).sum();
        Some(Guess {
            label: self.labels[best].clone(),
            confidence: 1.0 / total,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn shapes() {
        assert_eq!(filename_features("biegler1983"), "shapeauthoryear");
        assert_eq!(filename_features("Smith_et_al_2020"), "shapeauthoryear");
        assert!(filename_features("9404236v1").contains("shapearxiv"));
        assert!(filename_features("2301.04567v2").contains("shapearxiv"));
        assert_eq!(filename_features("Algorithm Design"), "");
    }

    #[test]
    fn learns() {
        let mut nb = NaiveBayes::new(vec!["Horror".into(), "Comedy".into()]);
        nb.add("horror", "ghost haunted house terror slasher", 1);
        nb.add("Comedy", "funny laugh romantic comedy parody", 1);
        assert_eq!(
            nb.classify("a haunted ghost story").unwrap().label,
            "Horror"
        );
        assert_eq!(nb.classify("a funny parody").unwrap().label, "Comedy");
        assert!(nb.classify("").is_none());
    }
}
