//! Wikipedia lookups (MediaWiki Action API) with an on-disk JSON cache.

use crate::media::{title_covers, title_similarity};
use anyhow::{Context, Result};
use regex::Regex;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::LazyLock;
use std::time::{Duration, Instant};
use ureq::tls::{RootCerts, TlsConfig, TlsProvider};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Hit {
    pub title: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub extract: String,
    #[serde(default)]
    pub url: String,
    #[serde(default)]
    pub categories: Vec<String>,
}

/// What a Wikipedia article is about, judged from its short description / first sentence.
#[derive(Debug, Clone, PartialEq)]
pub enum Cat {
    Film,
    Series,
    Music { artist: Option<String> },
    Book,
    Art,
    Game,
    Other,
}

static SERIES_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)\b(television series|tv series|television show|tv show|television program|miniseries|mini-series|web series|sitcom|soap opera|anime series|animated series|docuseries|reality series|streaming series|drama series|comedy series)\b").unwrap()
});
static FILM_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)\b(film|movie|documentary)\b").unwrap());
static MUSIC_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)\b(song|single|album|ep|mixtape|soundtrack|band|singer|rapper|musician|composer|recording artist|symphony|concerto)\b").unwrap()
});
static BOOK_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)\b(novel|novella|book|textbook|memoir|autobiography|manga|comic|graphic novel|short story|essay|treatise|poem|monograph|non-fiction|nonfiction|handbook)\b").unwrap()
});
static ART_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)\b(painting|sculpture|photograph|artwork|mural|fresco|portrait by)\b").unwrap()
});
static GAME_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?i)\bvideo game\b").unwrap());
static BY_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)\b(?:song|single|album|ep|recording)\b[^,;]*?\bby\s+(?:the\s+)?(?:american|british|english|canadian|australian|irish|scottish|swedish|korean|south korean|japanese|french|german|nigerian|jamaican)?\s*(?:singer|rapper|band|musician|group|duo|artist|singer-songwriter)?\s*([^,;()]+)").unwrap()
});
static LEAD_PARENS: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\s*\([^)]*\)").unwrap());
static TITLE_PAREN: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\s*\([^)]*\)\s*$").unwrap());
static YEAR: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\b(1[89]\d{2}|20\d{2})\b").unwrap());

impl Hit {
    /// Description, intro and categories: the text a file is classified from.
    pub fn classifier_text(&self) -> String {
        format!(
            "{0} {0} {1} {2}",
            self.description,
            self.extract,
            self.categories.join(" ")
        )
    }

    pub fn category(&self) -> Cat {
        let first_sentence: String = self.extract.chars().take(250).collect();
        for text in [self.description.as_str(), first_sentence.as_str()] {
            if text.is_empty() {
                continue;
            }
            let lower = text.to_lowercase();
            if lower.contains("television film") || lower.contains("film series") {
                return Cat::Film;
            }
            if SERIES_RE.is_match(text) {
                return Cat::Series;
            }
            if GAME_RE.is_match(text) {
                return Cat::Game;
            }
            if FILM_RE.is_match(text) {
                return Cat::Film;
            }
            if MUSIC_RE.is_match(text) {
                let artist = BY_RE
                    .captures(text)
                    .map(|c| c[1].trim().trim_end_matches('.').to_string())
                    .filter(|a| !a.is_empty());
                return Cat::Music { artist };
            }
            if BOOK_RE.is_match(text) {
                return Cat::Book;
            }
            if ART_RE.is_match(text) {
                return Cat::Art;
            }
        }
        Cat::Other
    }

    /// Title without the "(1999 film)" / "(TV series)" disambiguator.
    pub fn clean_title(&self) -> String {
        TITLE_PAREN.replace(&self.title, "").trim().to_string()
    }

    /// The subject of the first sentence: "X is a ..." -> "X" (parentheses removed).
    pub fn lead_name(&self) -> String {
        let head: String = self.extract.chars().take(200).collect();
        let cut = [
            " is a ", " is an ", " is the ", " was a ", " was an ", " are a ", " were a ",
        ]
        .iter()
        .filter_map(|m| head.find(m))
        .min();
        match cut {
            Some(i) => LEAD_PARENS.replace_all(&head[..i], "").trim().to_string(),
            None => String::new(),
        }
    }

    /// First 4-digit year in the description, e.g. "1999 film by ...".
    pub fn year(&self) -> Option<u16> {
        YEAR.find(&self.description)
            .or_else(|| YEAR.find(&self.title))
            .and_then(|m| m.as_str().parse().ok())
    }

    fn mentions_year(&self, year: u16) -> bool {
        let y = year.to_string();
        self.title.contains(&y)
            || self.description.contains(&y)
            || self
                .extract
                .chars()
                .take(300)
                .collect::<String>()
                .contains(&y)
    }

    fn is_disambiguation(&self) -> bool {
        let d = self.description.to_lowercase();
        d.contains("disambiguation")
            || d.contains("topics referred to")
            || self.extract.contains("may refer to")
    }
}

pub fn is_film(c: &Cat) -> bool {
    *c == Cat::Film
}
pub fn is_series(c: &Cat) -> bool {
    *c == Cat::Series
}
pub fn is_book(c: &Cat) -> bool {
    *c == Cat::Book
}
pub fn is_art(c: &Cat) -> bool {
    *c == Cat::Art
}
pub fn is_music(c: &Cat) -> bool {
    matches!(c, Cat::Music { .. })
}

pub struct Wiki {
    agent: Option<ureq::Agent>,
    lang: String,
    cache_path: PathBuf,
    cache: HashMap<String, Vec<Hit>>,
    dirty: bool,
    last_request: Option<Instant>,
    failures: usize,
    paused_until: Option<Instant>,
}

/// Wikimedia asks for serial requests; two per second, one at a time, is polite.
const MIN_GAP: Duration = Duration::from_millis(500);
const PAUSE_AFTER_FAILURE: Duration = Duration::from_secs(5 * 60);

impl Wiki {
    pub fn new(enabled: bool, lang: &str, contact: &str, cache_dir: PathBuf) -> Self {
        let cache_path = cache_dir.join(format!("wikipedia_{lang}.json"));
        let cache = std::fs::read_to_string(&cache_path)
            .ok()
            .and_then(|t| serde_json::from_str(&t).ok())
            .unwrap_or_default();
        let agent = enabled.then(|| {
            let ua = if contact.trim().is_empty() {
                format!(
                    "filesort/{} (personal media organizer)",
                    env!("CARGO_PKG_VERSION")
                )
            } else {
                format!(
                    "filesort/{} ({})",
                    env!("CARGO_PKG_VERSION"),
                    contact.trim()
                )
            };
            ureq::Agent::config_builder()
                .tls_config(
                    TlsConfig::builder()
                        .provider(TlsProvider::NativeTls)
                        .root_certs(RootCerts::PlatformVerifier)
                        .build(),
                )
                .timeout_global(Some(Duration::from_secs(20)))
                .user_agent(ua)
                .http_status_as_error(false)
                .build()
                .into()
        });
        Self {
            agent,
            lang: lang.to_string(),
            cache_path,
            cache,
            dirty: false,
            last_request: None,
            failures: 0,
            paused_until: None,
        }
    }

    pub fn enabled(&self) -> bool {
        self.agent.is_some()
    }

    /// Lookups that failed (network error, rate limit) since the sorter started.
    /// Callers compare this before/after a lookup to decide whether to defer a file.
    pub fn failures(&self) -> usize {
        self.failures
    }

    /// Full-text search; returns up to 5 articles with their short description and intro.
    pub fn search(&mut self, query: &str) -> Vec<Hit> {
        let key = query.trim().to_lowercase();
        if key.is_empty() {
            return Vec::new();
        }
        if let Some(h) = self.cache.get(&key) {
            return h.clone();
        }
        let Some(agent) = self.agent.clone() else {
            return Vec::new();
        };
        if self.paused_until.is_some_and(|t| Instant::now() < t) {
            self.failures += 1;
            return Vec::new();
        }
        match self.fetch(&agent, query) {
            Ok(hits) => {
                self.cache.insert(key, hits.clone());
                self.dirty = true;
                hits
            }
            Err(e) => {
                self.failures += 1;
                self.paused_until = Some(Instant::now() + PAUSE_AFTER_FAILURE);
                crate::log(&format!(
                    "  wikipedia unavailable ({e:#}); pausing lookups for {} min, affected files will be retried next run",
                    PAUSE_AFTER_FAILURE.as_secs() / 60
                ));
                Vec::new()
            }
        }
    }

    fn fetch(&mut self, agent: &ureq::Agent, query: &str) -> Result<Vec<Hit>> {
        let v = self.api(
            agent,
            &[
                ("generator", "search"),
                ("gsrsearch", query),
                ("gsrlimit", "5"),
                ("gsrnamespace", "0"),
                ("prop", "description|extracts|info|categories"),
                ("clshow", "!hidden"),
                ("cllimit", "max"),
                ("inprop", "url"),
                ("exintro", "1"),
                ("explaintext", "1"),
                ("exsentences", "3"),
                ("exlimit", "5"),
                ("redirects", "1"),
            ],
        )?;
        let resp: Resp = serde_json::from_value(v).context("decoding response")?;
        let mut pages = resp.query.map(|q| q.pages).unwrap_or_default();
        pages.sort_by_key(|p| p.index.unwrap_or(99));
        Ok(pages
            .into_iter()
            .map(|p| Hit {
                title: p.title,
                description: p.description.unwrap_or_default(),
                extract: p.extract.unwrap_or_default(),
                url: p.fullurl.unwrap_or_default(),
                categories: p
                    .categories
                    .unwrap_or_default()
                    .into_iter()
                    .map(|c| c.title.trim_start_matches("Category:").to_string())
                    .collect(),
            })
            .collect())
    }

    /// One `action=query` call: polite spacing, Retry-After on 429/503, back-off.
    fn api(&mut self, agent: &ureq::Agent, params: &[(&str, &str)]) -> Result<serde_json::Value> {
        let url = format!("https://{}.wikipedia.org/w/api.php", self.lang);
        for attempt in 0..4u32 {
            if let Some(t) = self.last_request {
                let gap = t.elapsed();
                if gap < MIN_GAP {
                    std::thread::sleep(MIN_GAP - gap);
                }
            }
            self.last_request = Some(Instant::now());
            let mut req = agent
                .get(&url)
                .query("action", "query")
                .query("format", "json")
                .query("formatversion", "2")
                .query("maxlag", "5");
            for (k, v) in params {
                req = req.query(*k, *v);
            }
            let mut resp = req.call().context("request")?;
            let status = resp.status().as_u16();
            if status == 429 || status == 503 {
                // Respect Retry-After; otherwise back off 5s, 10s, 20s.
                let wait = resp
                    .headers()
                    .get("retry-after")
                    .and_then(|v| v.to_str().ok())
                    .and_then(|v| v.trim().parse::<u64>().ok())
                    .unwrap_or(5 << attempt);
                if wait > 60 || attempt == 3 {
                    anyhow::bail!("HTTP {status} (rate limited, retry after {wait}s)");
                }
                std::thread::sleep(Duration::from_secs(wait));
                continue;
            }
            if status != 200 {
                anyhow::bail!("HTTP {status}");
            }
            let v: serde_json::Value = resp.body_mut().read_json().context("decoding response")?;
            if let Some(err) = v.get("error") {
                if err.get("code").and_then(|c| c.as_str()) == Some("maxlag") && attempt < 3 {
                    std::thread::sleep(Duration::from_secs(5));
                    continue;
                }
                anyhow::bail!("API error: {err}");
            }
            return Ok(v);
        }
        anyhow::bail!("gave up after retries")
    }

    /// Collect up to `want` articles from Wikipedia categories (title, short description,
    /// categories), walking subcategories breadth-first up to `max_depth` levels.
    ///
    /// The quota is shared evenly between the listed categories, so one big category can't
    /// crowd out the others. Entries starting with `-` are exclusions: subcategories and
    /// articles whose name / description / categories contain that word are skipped
    /// (whole words, so "-fiction" does not exclude "non-fiction").
    ///
    /// Returns what it got plus the error that stopped it, if any.
    pub fn harvest(
        &mut self,
        categories: &[String],
        want: usize,
        max_depth: usize,
        mut progress: impl FnMut(usize),
    ) -> (Vec<Article>, Option<anyhow::Error>) {
        let mut out: Vec<Article> = Vec::new();
        let Some(agent) = self.agent.clone() else {
            return (out, Some(anyhow::anyhow!("Wikipedia lookups are disabled")));
        };
        let (excl, seeds): (Vec<&String>, Vec<&String>) =
            categories.iter().partition(|c| c.trim().starts_with('-'));
        let excludes: Vec<Regex> = excl
            .iter()
            .map(|e| {
                let w = regex::escape(e.trim().trim_start_matches('-').trim());
                Regex::new(&format!(r"(?i)(?:^|[^\w-]){w}(?:$|[^\w-])")).unwrap()
            })
            .collect();
        let excluded = |text: &str| excludes.iter().any(|r| r.is_match(text));
        let mut seen_pages = std::collections::HashSet::new();
        let mut seen_cats = std::collections::HashSet::new();
        let mut last_report = 0;
        for (i, seed) in seeds.iter().enumerate() {
            if out.len() >= want {
                break;
            }
            // This seed's share of what's still missing; unused share rolls over to the next.
            let target = out.len() + (want - out.len()).div_ceil(seeds.len() - i);
            let mut queue: std::collections::VecDeque<(String, usize)> =
                std::collections::VecDeque::from([(
                    seed.trim().trim_start_matches("Category:").to_string(),
                    0,
                )]);
            while let Some((cat, depth)) = queue.pop_front() {
                if out.len() >= target {
                    break;
                }
                if !seen_cats.insert(cat.to_lowercase()) {
                    continue;
                }
                let title = format!("Category:{cat}");
                // Articles in this category with their short description and (visible)
                // categories, up to 500 per batch. A batch's category lists can span several
                // responses (clcontinue); the batch is only used once it is complete.
                let mut cont: Vec<(String, String)> = Vec::new();
                let mut order: Vec<String> = Vec::new();
                let mut batch: HashMap<String, (String, Vec<String>)> = HashMap::new();
                loop {
                    let mut params: Vec<(String, String)> = [
                        ("generator", "categorymembers"),
                        ("gcmtitle", title.as_str()),
                        ("gcmtype", "page"),
                        ("gcmnamespace", "0"),
                        ("gcmlimit", "max"),
                        ("prop", "description|categories"),
                        ("clshow", "!hidden"),
                        ("cllimit", "max"),
                    ]
                    .iter()
                    .map(|(k, v)| (k.to_string(), v.to_string()))
                    .collect();
                    params.extend(cont.iter().cloned());
                    let refs: Vec<(&str, &str)> = params
                        .iter()
                        .map(|(k, v)| (k.as_str(), v.as_str()))
                        .collect();
                    let v = match self.api(&agent, &refs) {
                        Ok(v) => v,
                        Err(e) => return (out, Some(e)),
                    };
                    for p in v
                        .pointer("/query/pages")
                        .and_then(|p| p.as_array())
                        .into_iter()
                        .flatten()
                    {
                        let t = p
                            .get("title")
                            .and_then(|t| t.as_str())
                            .unwrap_or("")
                            .to_string();
                        if t.is_empty() {
                            continue;
                        }
                        let e = batch.entry(t.clone()).or_insert_with(|| {
                            order.push(t.clone());
                            (String::new(), Vec::new())
                        });
                        if let Some(d) = p.get("description").and_then(|d| d.as_str()) {
                            e.0 = d.to_string();
                        }
                        for c in p
                            .get("categories")
                            .and_then(|c| c.as_array())
                            .into_iter()
                            .flatten()
                        {
                            if let Some(ct) = c.get("title").and_then(|t| t.as_str()) {
                                e.1.push(ct.trim_start_matches("Category:").to_string());
                            }
                        }
                    }
                    let next = v.get("continue").and_then(|c| c.as_object()).cloned();
                    let batch_done = next.as_ref().is_none_or(|n| !n.contains_key("clcontinue"));
                    if batch_done {
                        for t in order.drain(..) {
                            let (description, cats) = batch.remove(&t).unwrap_or_default();
                            let cats = cats.join(" | ");
                            if out.len() >= target
                                || t.starts_with("List of")
                                || t.starts_with("Lists of")
                                || excluded(&t)
                                || excluded(&description)
                                || excluded(&cats)
                                || !seen_pages.insert(t.clone())
                            {
                                continue;
                            }
                            let clean = |s: &str| s.replace(['\t', '\n', '\r'], " ");
                            out.push(Article {
                                title: clean(&t),
                                description: clean(&description),
                                categories: clean(&cats),
                            });
                        }
                        batch.clear();
                        if out.len() >= last_report + 100 || out.len() >= want {
                            last_report = out.len();
                            progress(out.len());
                        }
                    }
                    match next {
                        Some(n) if out.len() < target || !batch_done => {
                            cont = n
                                .iter()
                                .map(|(k, v)| (k.clone(), v.as_str().unwrap_or("").to_string()))
                                .collect();
                        }
                        _ => break,
                    }
                }
                if out.len() >= target || depth >= max_depth {
                    continue;
                }
                let subs = match self.api(
                    &agent,
                    &[
                        ("list", "categorymembers"),
                        ("cmtitle", title.as_str()),
                        ("cmtype", "subcat"),
                        ("cmlimit", "500"),
                    ],
                ) {
                    Ok(v) => v,
                    Err(e) => return (out, Some(e)),
                };
                for c in subs
                    .pointer("/query/categorymembers")
                    .and_then(|c| c.as_array())
                    .into_iter()
                    .flatten()
                {
                    if let Some(t) = c.get("title").and_then(|t| t.as_str()) {
                        let name = t.trim_start_matches("Category:").to_string();
                        let lower = name.to_lowercase();
                        // Lists, stubs and maintenance categories add noise, not genre words.
                        if lower.contains("lists of")
                            || lower.contains("stubs")
                            || lower.contains("wikipedia")
                            || lower.contains("templates")
                            || excluded(&name)
                        {
                            continue;
                        }
                        queue.push_back((name, depth + 1));
                    }
                }
            }
        }
        progress(out.len());
        (out, None)
    }

    /// Try each query in turn and return the best article in one of the wanted categories
    /// whose title matches `title`. Score is in 0..1.
    pub fn find(
        &mut self,
        queries: &[String],
        title: &str,
        year: Option<u16>,
        wanted: &[fn(&Cat) -> bool],
    ) -> Option<(Hit, Cat, f64)> {
        let mut best: Option<(Hit, Cat, f64)> = None;
        for q in queries {
            for (rank, hit) in self.search(q).into_iter().enumerate() {
                if hit.is_disambiguation()
                    || hit.title.starts_with("List of ")
                    || hit.title.starts_with("Lists of ")
                {
                    continue;
                }
                let cat = hit.category();
                let Some(pref) = wanted.iter().position(|w| w(&cat)) else {
                    continue;
                };
                // Compare against the article title and the name its first sentence uses
                // ("Mushoku Tensei" vs "Mushoku Tensei: Jobless Reincarnation is a ...").
                let (sim, article) = [hit.clean_title(), hit.lead_name()]
                    .into_iter()
                    .filter(|n| !n.is_empty())
                    .map(|n| (title_similarity(title, &n), n))
                    .fold(
                        (0.0, String::new()),
                        |best, x| if x.0 > best.0 { x } else { best },
                    );
                // A half match only counts when the article title is contained in the filename
                // ("The Office (US)" -> "The Office"), not the other way ("Cosmos" -> "Ultraman Cosmos").
                if sim < 0.5 || (sim < 0.67 && !title_covers(title, &article)) {
                    continue;
                }
                let mut score = sim * 0.75 + 0.15 - rank as f64 * 0.03 - pref as f64 * 0.05;
                if let Some(y) = year {
                    if hit.mentions_year(y) {
                        score += 0.15;
                    } else {
                        score -= 0.1;
                    }
                }
                let score = score.clamp(0.0, 1.0);
                if best.as_ref().is_none_or(|b| score > b.2) {
                    best = Some((hit, cat, score));
                }
            }
            if best.as_ref().is_some_and(|b| b.2 >= 0.8) {
                break;
            }
        }
        best
    }

    pub fn save(&mut self) {
        if !self.dirty {
            return;
        }
        if let Some(parent) = self.cache_path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        match serde_json::to_string(&self.cache) {
            Ok(t) => {
                if std::fs::write(&self.cache_path, t).is_ok() {
                    self.dirty = false;
                }
            }
            Err(e) => crate::log(&format!("could not save wikipedia cache: {e}")),
        }
    }
}

/// One training example harvested from a Wikipedia category.
#[derive(Debug, Clone)]
pub struct Article {
    pub title: String,
    pub description: String,
    /// The article's visible Wikipedia categories, " | " separated.
    pub categories: String,
}

#[derive(Deserialize)]
struct Resp {
    query: Option<Query>,
}
#[derive(Deserialize)]
struct Query {
    #[serde(default)]
    pages: Vec<Page>,
}
#[derive(Deserialize)]
struct Page {
    title: String,
    index: Option<u32>,
    description: Option<String>,
    extract: Option<String>,
    fullurl: Option<String>,
    categories: Option<Vec<PageCategory>>,
}
#[derive(Deserialize)]
struct PageCategory {
    title: String,
}

#[cfg(test)]
mod tests {
    use super::*;
    fn hit(t: &str, d: &str) -> Hit {
        Hit {
            title: t.into(),
            description: d.into(),
            extract: String::new(),
            url: String::new(),
            categories: Vec::new(),
        }
    }
    #[test]
    fn categories() {
        assert_eq!(
            hit("The Matrix", "1999 film by the Wachowskis").category(),
            Cat::Film
        );
        assert_eq!(
            hit("Breaking Bad", "American crime drama television series").category(),
            Cat::Series
        );
        assert_eq!(
            hit("Dune (novel)", "1965 novel by Frank Herbert").category(),
            Cat::Book
        );
        assert_eq!(
            hit("Hello (Adele song)", "2015 single by Adele").category(),
            Cat::Music {
                artist: Some("Adele".into())
            }
        );
        assert_eq!(
            hit("Bohemian Rhapsody", "1975 song by Queen").category(),
            Cat::Music {
                artist: Some("Queen".into())
            }
        );
        assert_eq!(
            hit("Mona Lisa", "Painting by Leonardo da Vinci").category(),
            Cat::Art
        );
        assert_eq!(
            hit("Doctor Who", "British science fiction TV series").category(),
            Cat::Series
        );
        assert_eq!(hit("X", "Video game by Nintendo").category(), Cat::Game);
        assert_eq!(
            hit("The Matrix (1999 film)", "").clean_title(),
            "The Matrix"
        );
        let mut h = hit("Mushoku Tensei", "Japanese light novel series");
        h.extract =
            "Mushoku Tensei: Jobless Reincarnation (Japanese: x) is a Japanese light novel series"
                .into();
        assert_eq!(h.lead_name(), "Mushoku Tensei: Jobless Reincarnation");
    }
}
