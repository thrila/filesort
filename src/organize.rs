//! One sorting pass over the watch folder.

use crate::classifier::Guess;
use crate::config::{Config, Dirs};
use crate::log;
use crate::media::{self, Kind};
use crate::models::Models;
use crate::wiki::{is_art, is_book, is_film, is_music, is_series, Cat, Hit, Wiki};
use anyhow::{Context, Result};
use regex::Regex;
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::LazyLock;
use std::time::{Duration, SystemTime};
use walkdir::WalkDir;

pub struct Sorter {
    cfg: Config,
    dirs: Dirs,
    wiki: Wiki,
    models: Models,
    /// How many videos in the current scan share each group_key (lowercased).
    video_groups: HashMap<String, usize>,
    /// Year for each photo album (top-level folder under the watch dir) in the current scan.
    album_years: HashMap<PathBuf, String>,
    /// How many books in the current scan share each group_key (lowercased).
    book_groups: HashMap<String, usize>,
    /// Files already reported as "left where it is", so each is logged once.
    left_reported: HashSet<PathBuf>,
    /// EPUB / PDF metadata, read once per file.
    doc_meta: crate::docmeta::Cache,
}

#[derive(Default, Debug)]
pub struct Stats {
    pub moved: usize,
    pub skipped: usize,
    pub deferred: usize,
    pub failed: usize,
}

/// What a planned move is, for the `skip` setting.
#[derive(Debug, Clone, Copy, PartialEq)]
enum Bucket {
    Movie,
    Series,
    UnsortedVideo,
    HomeVideo,
    Music,
    Picture,
    Book,
    UnsortedBook,
    Document,
    Archive,
    Program,
    Misc,
}

impl Bucket {
    /// `skip` names that cover this kind of move.
    fn names(self) -> &'static [&'static str] {
        match self {
            Bucket::Movie => &["movies", "videos"],
            Bucket::Series => &["series", "videos"],
            Bucket::UnsortedVideo => &["unsorted", "videos"],
            Bucket::HomeVideo => &["pictures", "videos"],
            Bucket::Music => &["music"],
            Bucket::Picture => &["pictures"],
            Bucket::Book => &["books"],
            Bucket::UnsortedBook => &["unsorted", "books"],
            Bucket::Document => &["other", "documents"],
            Bucket::Archive => &["other", "archives"],
            Bucket::Program => &["other", "programs"],
            Bucket::Misc => &["other"],
        }
    }
}

struct Plan {
    /// Genre / subject predicted by the classifier, for the log and wikipedia.json.
    genre: Option<Guess>,
    /// Set when the model wasn't sure enough to decide where the file goes: it stays put.
    leave: Option<String>,
    bucket: Bucket,
    dest: PathBuf,
    what: &'static str,
    note: String,
    /// Folder that gets a wikipedia.json, and the article it describes.
    info: Option<(PathBuf, Hit)>,
}

static FILENAME_DATE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"(?:^|\D)((?:19|20)\d{2})[-_]?(?:0[1-9]|1[0-2])[-_]?(?:0[1-9]|[12]\d|3[01])(?:\D|$)",
    )
    .unwrap()
});

impl Sorter {
    pub fn new(cfg: Config) -> Self {
        let dirs = cfg.dirs();
        let wiki = Wiki::new(
            cfg.wikipedia.enabled,
            &cfg.wikipedia.lang,
            &cfg.wikipedia.contact,
            dirs.cache.clone(),
        );
        let mut wiki = wiki;
        let models = Models::load(&cfg.classifier, &dirs.books, &mut wiki);
        Self {
            cfg,
            dirs,
            wiki,
            models,
            video_groups: HashMap::new(),
            album_years: HashMap::new(),
            book_groups: HashMap::new(),
            left_reported: HashSet::new(),
            doc_meta: crate::docmeta::Cache::default(),
        }
    }

    /// Library folders that must never be scanned as input (when the watch dir contains them).
    fn protected(&self) -> Vec<PathBuf> {
        let d = &self.dirs;
        let mut v = vec![
            d.movies.clone(),
            d.series.clone(),
            d.music.clone(),
            d.pictures.clone(),
            d.other.clone(),
            d.unsorted.clone(),
            d.cache.clone(),
        ];
        v.push(d.books.clone());
        for l in &self.cfg.classifier.book_labels() {
            v.push(d.books.join(l));
        }
        let watch = canon(&d.watch);
        v.into_iter()
            .map(|p| canon(&p))
            .filter(|p| *p != watch)
            .collect()
    }

    pub fn run_once(&mut self) -> Result<Stats> {
        let watch = self.dirs.watch.clone();
        if !watch.is_dir() {
            anyhow::bail!("watch_dir {} does not exist", watch.display());
        }
        let protected = self.protected();
        let settle = Duration::from_secs(self.cfg.settle_seconds);
        let now = SystemTime::now();

        let mut files: Vec<PathBuf> = Vec::new();
        let walker = WalkDir::new(&watch)
            .min_depth(1)
            .max_depth(if self.cfg.recursive { 32 } else { 1 })
            .sort_by_file_name()
            .into_iter()
            .filter_entry(|e| {
                if !e.file_type().is_dir() {
                    return true;
                }
                let name = e.file_name().to_string_lossy();
                if name.starts_with('.') || protected.contains(&canon(e.path())) {
                    return false;
                }
                // Software projects are left completely alone.
                if media::is_code_project(e.path()) {
                    log(&format!(
                        "  leaving  {} (software project, not scanned)",
                        e.path().strip_prefix(&watch).unwrap_or(e.path()).display()
                    ));
                    return false;
                }
                true
            });
        for entry in walker.flatten() {
            if entry.file_type().is_file() && !media::is_ignored(entry.path()) {
                files.push(entry.into_path());
            }
        }

        // Pair subtitles / .nfo files, and posters / fanart next to a movie or episode,
        // with their video. Phone/camera clips are not owners: they belong to photo albums.
        let mut videos_by_dir: HashMap<PathBuf, Vec<PathBuf>> = HashMap::new();
        files.retain(|f| !is_sample(f));
        for f in files
            .iter()
            .filter(|f| media::kind_of(f) == Kind::Video && !is_camera_file(f))
        {
            videos_by_dir
                .entry(f.parent().unwrap().to_path_buf())
                .or_default()
                .push(f.clone());
        }
        let mut sidecars: HashMap<PathBuf, Vec<PathBuf>> = HashMap::new();
        let mut attached: HashSet<PathBuf> = HashSet::new();
        for f in &files {
            let kind = media::kind_of(f);
            let image_by_video = kind == Kind::Image && !is_camera_file(f);
            if kind != Kind::Sidecar && !image_by_video {
                continue;
            }
            if let Some(owner) = sidecar_owner(f, &videos_by_dir, &watch) {
                sidecars.entry(owner).or_default().push(f.clone());
                attached.insert(f.clone());
            }
        }

        // Other images inside a movie / episode folder (release ads, screenshots) are left
        // where they are rather than treated as someone's photos.
        let in_video_folder = |f: &PathBuf| {
            media::kind_of(f) == Kind::Image
                && !attached.contains(f)
                && f.parent()
                    .is_some_and(|d| d != watch && videos_by_dir.contains_key(d))
        };
        let left_with_video: HashSet<PathBuf> = files
            .iter()
            .filter(|f| in_video_folder(f))
            .cloned()
            .collect();

        // Photos that arrive inside a folder stay together as an album, filed under the
        // year of the oldest photo in that folder: Pictures/<Year>/<Folder>/.
        self.album_years.clear();
        for f in files
            .iter()
            .filter(|f| !attached.contains(*f) && !left_with_video.contains(*f) && is_photo_like(f))
        {
            if let Some(top) = album_root(&watch, f) {
                let stem = f.file_stem().unwrap_or_default().to_string_lossy();
                let year = picture_year(f, &stem);
                let e = self.album_years.entry(top).or_insert_with(|| year.clone());
                if year < *e {
                    *e = year;
                }
            }
        }

        self.book_groups.clear();
        for f in files.iter().filter(|f| media::kind_of(f) == Kind::Book) {
            let stem = f.file_stem().unwrap_or_default().to_string_lossy();
            let b = media::parse_book(&stem);
            let key = media::group_key(&b.series.unwrap_or(b.title)).to_lowercase();
            *self.book_groups.entry(key).or_default() += 1;
        }
        self.video_groups.clear();
        for f in files.iter().filter(|f| media::kind_of(f) == Kind::Video) {
            let stem = f.file_stem().unwrap_or_default().to_string_lossy();
            let key = media::group_key(&media::parse_video(&stem).title).to_lowercase();
            *self.video_groups.entry(key).or_default() += 1;
        }

        let mut stats = Stats::default();
        for path in &files {
            let kind = media::kind_of(path);
            if kind == Kind::Sidecar || attached.contains(path) {
                continue;
            }
            if left_with_video.contains(path) {
                stats.skipped += 1;
                continue;
            }
            if self.skipped_up_front(path, kind) {
                stats.skipped += 1;
                continue;
            }
            let young = std::fs::metadata(path)
                .and_then(|m| m.modified())
                .ok()
                .and_then(|t| now.duration_since(t).ok())
                .is_some_and(|age| age < settle);
            if young {
                log(&format!(
                    "  waiting  {} (modified less than {}s ago)",
                    self.rel(path),
                    settle.as_secs()
                ));
                stats.skipped += 1;
                continue;
            }
            let failures_before = self.wiki.failures();
            let plan = self.plan(path, kind);
            if self.wiki.failures() > failures_before {
                log(&format!(
                    "  deferred {} (Wikipedia lookup failed, will retry next run)",
                    self.rel(path)
                ));
                stats.deferred += 1;
                continue;
            }
            let Some(plan) = plan else {
                stats.skipped += 1;
                continue;
            };
            if self.is_skipped(plan.bucket) {
                stats.skipped += 1;
                continue;
            }
            if let Some(why) = &plan.leave {
                if self.left_reported.insert(path.clone()) {
                    log(&format!(
                        "  leaving  {} where it is ({why})",
                        self.rel(path)
                    ));
                }
                stats.skipped += 1;
                continue;
            }
            let extras = sidecars.get(path).cloned().unwrap_or_default();
            match self.execute(path, &extras, &plan) {
                Ok(()) => stats.moved += 1,
                Err(e) => {
                    log(&format!("  FAILED   {}: {e:#}", self.rel(path)));
                    stats.failed += 1;
                }
            }
        }
        self.wiki.save();

        if self.cfg.remove_empty_dirs && !self.cfg.dry_run {
            remove_empty_dirs(&watch, &protected);
        }
        Ok(stats)
    }

    fn is_skipped(&self, bucket: Bucket) -> bool {
        bucket
            .names()
            .iter()
            .any(|n| self.cfg.skip.iter().any(|s| s == n))
    }

    /// Cheap checks before any Wikipedia lookup: skipped extensions, and file types whose
    /// every possible destination is skipped.
    fn skipped_up_front(&self, path: &Path, kind: Kind) -> bool {
        let ext = path
            .extension()
            .map(|e| e.to_string_lossy().to_lowercase())
            .unwrap_or_default();
        if self.cfg.skip_extensions.contains(&ext) {
            return true;
        }
        let skip = |n: &str| self.cfg.skip.iter().any(|s| s == n);
        match kind {
            Kind::Video => skip("videos"),
            Kind::Audio => skip("music"),
            Kind::Image => skip("pictures"),
            Kind::Book => skip("books"),
            Kind::Document | Kind::Archive | Kind::Program | Kind::Other => skip("other"),
            Kind::Sidecar | Kind::Code => false,
        }
    }

    fn plan(&mut self, path: &Path, kind: Kind) -> Option<Plan> {
        let stem = path.file_stem()?.to_string_lossy().to_string();
        match kind {
            Kind::Video => {
                let mut plan = self.plan_video(path, &stem)?;
                self.add_video_genre(&mut plan, &stem);
                Some(plan)
            }
            Kind::Audio => Some(self.plan_audio(path, &stem)),
            Kind::Image => Some(self.plan_image(path, &stem)),
            Kind::Book => Some(self.plan_book(path, &stem)),
            Kind::Document | Kind::Archive | Kind::Program | Kind::Other => {
                let (sub, bucket) = match kind {
                    Kind::Document => ("Documents", Bucket::Document),
                    Kind::Archive => ("Archives", Bucket::Archive),
                    Kind::Program => ("Programs", Bucket::Program),
                    _ => ("Misc", Bucket::Misc),
                };
                Some(Plan {
                    genre: None,
                    leave: None,
                    bucket,
                    dest: self.dirs.other.join(sub),
                    what: "Other",
                    note: String::new(),
                    info: None,
                })
            }
            Kind::Sidecar | Kind::Code => None,
        }
    }

    fn plan_video(&mut self, path: &Path, stem: &str) -> Option<Plan> {
        if media::is_camera_name(stem) {
            if let Some(dest) = self.album_dest(path) {
                return Some(Plan {
                    genre: None,
                    leave: None,
                    bucket: Bucket::HomeVideo,
                    dest,
                    what: "Home video",
                    note: "kept with its album".into(),
                    info: None,
                });
            }
            let year = picture_year(path, stem);
            return Some(Plan {
                genre: None,
                leave: None,
                bucket: Bucket::HomeVideo,
                dest: self.dirs.pictures.join(year),
                what: "Home video",
                note: String::new(),
                info: None,
            });
        }

        let v = media::parse_video(stem);
        let min = self.cfg.min_confidence;

        if let Some(season) = v.season {
            let queries = vec![format!("{} TV series", v.title), v.title.clone()];
            let hit = self
                .wiki
                .find(&queries, &v.title, v.year, &[is_series])
                .filter(|h| h.2 >= min);
            let show = match &hit {
                // Keep a qualifier the filename already has, e.g. "The Office (US)".
                Some(_) if v.title.contains('(') => media::title_case(&v.title),
                Some((h, _, _)) => h.clean_title(),
                None => match v.year {
                    Some(y) => format!("{} ({y})", media::title_case(&v.title)),
                    None => media::title_case(&v.title),
                },
            };
            let show_dir = self.dirs.series.join(media::safe_dir_name(&show));
            let note = match (&hit, v.episode) {
                (Some(h), Some(e)) => format!("S{season:02}E{e:02}, {}", describe(h)),
                (Some(h), None) => format!("season {season}, {}", describe(h)),
                (None, Some(e)) => format!("S{season:02}E{e:02}, not found on Wikipedia"),
                (None, None) => format!("season {season}, not found on Wikipedia"),
            };
            return Some(Plan {
                genre: None,
                leave: None,
                bucket: Bucket::Series,
                dest: show_dir.join(format!("Season {season:02}")),
                what: "Episode",
                note,
                info: hit.map(|(h, _, _)| (show_dir, h)),
            });
        }

        // "Cosmos Episode 7", "Networking tutorial (9 of 13)": episodes without a season.
        if let Some(ep) = v.episode {
            let queries = vec![format!("{} TV series", v.title), v.title.clone()];
            return Some(
                match self
                    .wiki
                    .find(&queries, &v.title, v.year, &[is_series])
                    .filter(|h| h.2 >= min)
                {
                    Some(found) => {
                        let show_dir = self
                            .dirs
                            .series
                            .join(media::safe_dir_name(&found.0.clean_title()));
                        Plan {
                            genre: None,
                            leave: None,
                            bucket: Bucket::Series,
                            dest: show_dir.clone(),
                            what: "Episode",
                            note: format!("episode {ep}, {}", describe(&found)),
                            info: Some((show_dir, found.0)),
                        }
                    }
                    None => match self.folder_dest(&self.dirs.unsorted, path) {
                        Some(dest) => Plan {
                            genre: None,
                            leave: None,
                            bucket: Bucket::UnsortedVideo,
                            dest,
                            what: "Episode",
                            note: format!(
                                "episode {ep}, not found on Wikipedia, kept in its folder"
                            ),
                            info: None,
                        },
                        None => Plan {
                            genre: None,
                            leave: None,
                            bucket: Bucket::UnsortedVideo,
                            dest: self
                                .dirs
                                .unsorted
                                .join(media::safe_dir_name(&media::title_case(&v.title))),
                            what: "Episode",
                            note: format!("episode {ep}, not found on Wikipedia, grouped by name"),
                            info: None,
                        },
                    },
                },
            );
        }

        let mut queries = Vec::new();
        if v.numbered {
            // "1. Introduction.mp4" is a lesson, not the film "Introduction": no movie lookup.
        } else if let Some(y) = v.year {
            queries.push(format!("{} {y} film", v.title));
        }
        if !v.numbered {
            queries.push(format!("{} film", v.title));
            queries.push(v.title.clone());
        }
        match self
            .wiki
            .find(&queries, &v.title, v.year, &[is_film, is_series])
            .filter(|h| h.2 >= min)
        {
            Some((h, Cat::Series, s)) => {
                let show_dir = self
                    .dirs
                    .series
                    .join(media::safe_dir_name(&h.clean_title()));
                Some(Plan {
                    genre: None,
                    leave: None,
                    bucket: Bucket::Series,
                    dest: show_dir.clone(),
                    what: "Series",
                    note: describe(&(h.clone(), Cat::Series, s)),
                    info: Some((show_dir, h)),
                })
            }
            Some((h, c, s)) => {
                let year = v.year.or(h.year());
                let name = match year {
                    Some(y) => format!("{} ({y})", h.clean_title()),
                    None => h.clean_title(),
                };
                let dir = self.dirs.movies.join(media::safe_dir_name(&name));
                Some(Plan {
                    genre: None,
                    leave: None,
                    bucket: Bucket::Movie,
                    dest: dir.clone(),
                    what: "Movie",
                    note: describe(&(h.clone(), c, s)),
                    info: Some((dir, h)),
                })
            }
            None if v.year.is_some() => {
                // "Title.2019.1080p..." is a strong movie pattern even without a Wikipedia article.
                let name = format!("{} ({})", media::title_case(&v.title), v.year.unwrap());
                Some(Plan {
                    genre: None,
                    leave: None,
                    bucket: Bucket::Movie,
                    dest: self.dirs.movies.join(media::safe_dir_name(&name)),
                    what: "Movie",
                    note: "not found on Wikipedia, using filename".into(),
                    info: None,
                })
            }
            None => {
                // Unidentified videos that came in a folder (a course, a channel download)
                // keep that folder structure.
                if let Some(dest) = self.folder_dest(&self.dirs.unsorted, path) {
                    return Some(Plan {
                        genre: None,
                        leave: None,
                        bucket: Bucket::UnsortedVideo,
                        dest,
                        what: "Video",
                        note: "could not identify, kept in its folder".into(),
                        info: None,
                    });
                }
                // Keep look-alike videos ("Lecture 1", "Lecture 2") together in one folder.
                let key = media::group_key(&v.title);
                let group_dir = self
                    .dirs
                    .unsorted
                    .join(media::safe_dir_name(&media::title_case(&key)));
                let shared = self
                    .video_groups
                    .get(&key.to_lowercase())
                    .copied()
                    .unwrap_or(0)
                    >= 2;
                if shared || group_dir.is_dir() {
                    Some(Plan {
                        genre: None,
                        leave: None,
                        bucket: Bucket::UnsortedVideo,
                        dest: group_dir,
                        what: "Video",
                        note: "could not identify, grouped by name".into(),
                        info: None,
                    })
                } else {
                    Some(Plan {
                        genre: None,
                        leave: None,
                        bucket: Bucket::UnsortedVideo,
                        dest: self.dirs.unsorted.clone(),
                        what: "Video",
                        note: "could not identify".into(),
                        info: None,
                    })
                }
            }
        }
    }

    /// Classify a movie or series by genre from its name and Wikipedia article. The genre is
    /// recorded; with `genre_folders` it also becomes a folder level.
    fn add_video_genre(&mut self, plan: &mut Plan, stem: &str) {
        let (kind, base) = match plan.bucket {
            Bucket::Movie => ("movies", self.dirs.movies.clone()),
            Bucket::Series => ("series", self.dirs.series.clone()),
            _ => return,
        };
        let name = media::parse_video(stem).title;
        let text = match &plan.info {
            Some((_, h)) => format!("{} {name} {}", h.clean_title(), h.classifier_text()),
            None => name,
        };
        let sure = self.cfg.classifier.min_confidence;
        let guess = self.models.classify(kind, &text);
        let Some(g) = guess.clone().filter(|g| g.confidence >= sure) else {
            if self.cfg.classifier.genre_folder(kind) {
                plan.leave = Some(match guess {
                    Some(g) => format!(
                        "genre guess {} at {:.0}% is below {:.0}%",
                        g.label,
                        g.confidence * 100.0,
                        sure * 100.0
                    ),
                    None => "no genre".into(),
                });
            }
            return;
        };
        if self.cfg.classifier.genre_folder(kind) {
            let genre_dir = base.join(media::safe_dir_name(&g.label));
            let regroup = |p: &Path| {
                p.strip_prefix(&base)
                    .map(|r| genre_dir.join(r))
                    .unwrap_or_else(|_| p.to_path_buf())
            };
            plan.dest = regroup(&plan.dest);
            if let Some((dir, _)) = plan.info.as_mut() {
                *dir = regroup(dir);
            }
        }
        plan.note = format!("{}, genre {} ({:.2})", plan.note, g.label, g.confidence);
        plan.genre = Some(g);
    }

    fn plan_audio(&mut self, path: &Path, stem: &str) -> Plan {
        let tags = read_tags(path);
        let parsed = media::parse_song(stem);
        let title = tags.title.clone().unwrap_or(parsed.title.clone());
        let mut artist = tags
            .album_artist
            .clone()
            .or(tags.artist.clone())
            .or(parsed.artist.clone());
        let mut note = if tags.artist.is_some() {
            "from tags".to_string()
        } else {
            "from filename".to_string()
        };
        if artist.is_none() {
            let queries = vec![format!("{title} song"), title.clone()];
            if let Some((h, Cat::Music { artist: Some(a) }, s)) =
                self.wiki.find(&queries, &title, None, &[is_music])
            {
                if s >= self.cfg.min_confidence {
                    note = describe(&(
                        h.clone(),
                        Cat::Music {
                            artist: Some(a.clone()),
                        },
                        s,
                    ));
                    artist = Some(a);
                }
            }
        }
        // Genre: the song's own Wikipedia article (cached), its genre tag and its names.
        let mut text = format!(
            "{title} {} {} {}",
            artist.clone().unwrap_or_default(),
            tags.album.clone().unwrap_or_default(),
            tags.genre.clone().unwrap_or_default()
        );
        if let Some(a) = &artist {
            let queries = vec![format!("{title} {a} song"), format!("{title} song")];
            if let Some((h, _, s)) = self.wiki.find(&queries, &title, None, &[is_music]) {
                if s >= self.cfg.min_confidence {
                    text = format!("{text} {}", h.classifier_text());
                }
            }
        }
        let sure = self.cfg.classifier.min_confidence;
        let guess = self.models.classify("music", &text);
        let genre = guess.clone().filter(|g| g.confidence >= sure);
        let leave = match (&genre, self.cfg.classifier.genre_folder("music")) {
            (None, true) => Some(match &guess {
                Some(g) => format!(
                    "genre guess {} at {:.0}% is below {:.0}%",
                    g.label,
                    g.confidence * 100.0,
                    sure * 100.0
                ),
                None => "no genre".into(),
            }),
            _ => None,
        };
        let artist = media::safe_dir_name(&artist.unwrap_or_else(|| "Unknown Artist".into()));
        let album = media::safe_dir_name(&tags.album.unwrap_or_else(|| "Singles".into()));
        let mut dest = self.dirs.music.clone();
        if let Some(g) = &genre {
            note = format!("{note}, genre {} ({:.2})", g.label, g.confidence);
            if self.cfg.classifier.genre_folder("music") {
                dest = dest.join(media::safe_dir_name(&g.label));
            }
        }
        Plan {
            genre,
            leave,
            bucket: Bucket::Music,
            dest: dest.join(artist).join(album),
            what: "Music",
            note,
            info: None,
        }
    }

    /// `base`/<folder>/<subfolder…> mirroring where the file sits under the watch dir;
    /// None for files directly in the watch dir.
    fn folder_dest(&self, base: &Path, path: &Path) -> Option<PathBuf> {
        let rel = path.parent()?.strip_prefix(&self.dirs.watch).ok()?;
        if rel.as_os_str().is_empty() {
            return None;
        }
        let mut dest = base.to_path_buf();
        for part in rel.components() {
            dest.push(media::safe_dir_name(&part.as_os_str().to_string_lossy()));
        }
        Some(dest)
    }

    /// Pictures/<album year>/<folder>/<subfolder…> for a photo inside a folder of the watch dir.
    fn album_dest(&self, path: &Path) -> Option<PathBuf> {
        let top = album_root(&self.dirs.watch, path)?;
        let year = self.album_years.get(&top)?;
        let rel = path.parent()?.strip_prefix(&self.dirs.watch).ok()?;
        let mut dest = self.dirs.pictures.join(year);
        for part in rel.components() {
            dest.push(media::safe_dir_name(&part.as_os_str().to_string_lossy()));
        }
        Some(dest)
    }

    fn plan_image(&mut self, path: &Path, stem: &str) -> Plan {
        if let Some(dest) = self.album_dest(path) {
            return Plan {
                genre: None,
                leave: None,
                bucket: Bucket::Picture,
                dest,
                what: "Picture",
                note: "kept with its album".into(),
                info: None,
            };
        }
        if !media::is_camera_name(stem) {
            let title = media::parse_book(stem).title;
            if let Some(found) =
                self.wiki
                    .find(std::slice::from_ref(&title), &title, None, &[is_art])
            {
                if found.2 >= self.cfg.min_confidence.max(0.6) {
                    return Plan {
                        genre: None,
                        leave: None,
                        bucket: Bucket::Picture,
                        dest: self.dirs.pictures.join("Art"),
                        what: "Artwork",
                        note: describe(&found),
                        info: None,
                    };
                }
            }
        }
        Plan {
            genre: None,
            leave: None,
            bucket: Bucket::Picture,
            dest: self.dirs.pictures.join(picture_year(path, stem)),
            what: "Picture",
            note: String::new(),
            info: None,
        }
    }

    /// The Wikipedia article about a book's author: the search result whose title is their
    /// full name ("Collins Suzanne" and "Suzanne Collins" both match).
    fn author_article(&mut self, author: &str) -> Option<Hit> {
        let name = author.split(['[', '(']).next()?.trim();
        if name.split_whitespace().count() < 2 {
            return None;
        }
        self.wiki.search(name).into_iter().find(|h| {
            media::title_similarity(name, &h.clean_title()) >= 0.99
                && !h.description.to_lowercase().contains("disambiguation")
        })
    }

    fn plan_book(&mut self, path: &Path, stem: &str) -> Plan {
        let b = media::parse_book(stem);
        // What the file says about itself (EPUB / PDF metadata).
        let meta = self.doc_meta.get(path);
        // A series is named by a volume marker, or by several numbered books in one scan
        // ("Dune 1", "Dune 2"). Every volume of a series is filed the same way, because the
        // lookup and the subject guess use only the series name.
        let series = b.series.clone().or_else(|| {
            let key = media::group_key(&b.title);
            let shared = self
                .book_groups
                .get(&key.to_lowercase())
                .copied()
                .unwrap_or(0)
                >= 2;
            (shared && key != b.title).then_some(key)
        });
        let name = series.clone().unwrap_or_else(|| b.title.clone());
        let first_author = b
            .author
            .as_deref()
            .and_then(|a| a.split(',').next())
            .unwrap_or("")
            .trim()
            .to_string();
        let mut queries = Vec::new();
        if !first_author.is_empty() {
            queries.push(format!("{name} {first_author}"));
        }
        if series.is_some() {
            queries.push(name.clone());
            queries.push(format!("{name} novel series"));
        } else {
            queries.push(format!("{name} book"));
        }
        // A real title from the metadata helps when the file name is junk ("main.pdf").
        let mut match_name = name.clone();
        if let Some(m) = &meta {
            let author = m.authors.first().map(String::as_str).unwrap_or("");
            if !m.title.is_empty() && !m.title.eq_ignore_ascii_case(&name) {
                queries.push(format!("{} {author}", m.title).trim().to_string());
                if name.split_whitespace().count() < 2 {
                    match_name = m.title.clone();
                }
            }
        }
        let hit = self
            .wiki
            .find(&queries, &match_name, None, &[is_book, is_series])
            .filter(|h| h.2 >= 0.6);

        let mut text = format!(
            "{0} {0} {1} {2}",
            name,
            b.author.clone().unwrap_or_default(),
            crate::classifier::filename_features(stem)
        );
        if let Some((h, _, _)) = &hit {
            text = format!("{text} {}", h.classifier_text());
        }
        if let Some(m) = &meta {
            text = format!("{text} {}", m.classifier_text());
        }
        // No article for the book itself: the author's article says what they write
        // ("Stephanie Garber … young adult fantasy novels").
        let author_name = meta
            .as_ref()
            .and_then(|m| m.authors.first().cloned())
            .filter(|a| !a.is_empty())
            .unwrap_or_else(|| first_author.clone());
        let author_hit = if hit.is_none() && !author_name.is_empty() {
            self.author_article(&author_name)
        } else {
            None
        };
        if let Some(a) = &author_hit {
            text = format!("{text} {}", a.classifier_text());
        }
        // Series folder: the article's name if Wikipedia knows it, else the filename's.
        let series_dir = series.as_ref().map(|s| {
            let n = match &hit {
                Some((h, _, _)) => h.clean_title(),
                None => media::title_case(s),
            };
            media::safe_dir_name(&n)
        });
        let vol = match (&series_dir, b.volume) {
            (Some(d), Some(v)) => format!("{d} vol. {v}, "),
            (Some(d), None) => format!("{d}, "),
            _ => String::new(),
        };
        let mut sources = vec!["filename"];
        if meta.is_some() {
            sources.push("metadata");
        }
        if hit.is_some() {
            sources.push("wikipedia");
        }
        let author_note = author_hit
            .as_ref()
            .map(|a| format!("author's wikipedia: {} — {}", a.title, a.description));
        if author_note.is_some() {
            sources.push("author's wikipedia");
        }
        let source = sources.join(" + ");
        let in_series = |base: PathBuf| match &series_dir {
            Some(d) => base.join(d),
            None => base,
        };
        let guess = self.models.classify("books", &text);
        let wiki_note = match (&hit, &author_note) {
            (Some(h), _) => format!(", {}", describe(h)),
            (None, Some(a)) => format!(", {a}"),
            (None, None) => String::new(),
        };
        let sure = self.cfg.classifier.min_confidence;
        match guess {
            Some(g) if g.confidence >= sure => Plan {
                genre: None,
                leave: None,
                bucket: Bucket::Book,
                dest: in_series(self.dirs.books.join(&g.label)),
                what: "Book",
                note: match &hit {
                    Some(h) => format!(
                        "{vol}{} ({:.2}) from {source}, {}",
                        g.label,
                        g.confidence,
                        describe(h)
                    ),
                    None => format!(
                        "{vol}{} ({:.2}) from {source}{wiki_note}",
                        g.label, g.confidence
                    ),
                },
                info: None,
            },
            g => Plan {
                genre: None,
                leave: Some(match g {
                    Some(g) => format!(
                        "{vol}best guess {} at {:.0}% is below {:.0}%{wiki_note}",
                        g.label,
                        g.confidence * 100.0,
                        sure * 100.0
                    ),
                    None => format!("{vol}no usable words in the title{wiki_note}"),
                }),
                bucket: Bucket::UnsortedBook,
                dest: in_series(self.dirs.books.join("Unsorted")),
                what: "Book",
                note: String::new(),
                info: None,
            },
        }
    }

    fn execute(&mut self, src: &Path, sidecars: &[PathBuf], plan: &Plan) -> Result<()> {
        let verb = if self.cfg.dry_run {
            "would move"
        } else {
            "moved"
        };
        let note = if plan.note.is_empty() {
            String::new()
        } else {
            format!("  [{}]", plan.note)
        };
        let extra = if sidecars.is_empty() {
            String::new()
        } else {
            format!(" (+{} extra files)", sidecars.len())
        };
        log(&format!(
            "  {:<10} {}{} -> {}  {verb}{note}",
            plan.what,
            self.rel(src),
            extra,
            self.rel(&plan.dest)
        ));
        if self.cfg.dry_run {
            return Ok(());
        }
        std::fs::create_dir_all(&plan.dest)
            .with_context(|| format!("creating {}", plan.dest.display()))?;
        move_file(src, &plan.dest)?;
        for s in sidecars {
            if let Err(e) = move_file(s, &plan.dest) {
                log(&format!("  FAILED   moving {}: {e:#}", self.rel(s)));
            }
        }
        if let (true, Some((dir, hit))) = (self.cfg.wikipedia.write_info, &plan.info) {
            let file = dir.join("wikipedia.json");
            if !file.exists() {
                std::fs::create_dir_all(dir)?;
                let mut info = serde_json::to_value(hit)?;
                if let (Some(g), Some(obj)) = (&plan.genre, info.as_object_mut()) {
                    obj.insert("genre".into(), g.label.clone().into());
                    obj.insert(
                        "genre_confidence".into(),
                        ((g.confidence * 100.0).round() / 100.0).into(),
                    );
                }
                std::fs::write(&file, serde_json::to_string_pretty(&info)?)?;
            }
        }
        Ok(())
    }

    fn rel(&self, p: &Path) -> String {
        for base in [&self.dirs.watch, &self.cfg.library_root] {
            if let Ok(r) = p.strip_prefix(base) {
                let s = r.display().to_string();
                return if s.is_empty() { ".".into() } else { s };
            }
        }
        p.display().to_string()
    }
}

fn is_camera_file(path: &Path) -> bool {
    media::is_camera_name(&path.file_stem().unwrap_or_default().to_string_lossy())
}

/// Pictures, and phone/camera video clips.
fn is_photo_like(path: &Path) -> bool {
    match media::kind_of(path) {
        Kind::Image => true,
        Kind::Video => is_camera_file(path),
        _ => false,
    }
}

/// The top-level folder under `watch` that contains `path`, or None for loose files.
fn album_root(watch: &Path, path: &Path) -> Option<PathBuf> {
    let rel = path.parent()?.strip_prefix(watch).ok()?;
    let first = rel.components().next()?;
    Some(watch.join(first))
}

/// Release "sample" clips (small videos named sample / *.sample / *-sample) are left alone.
fn is_sample(path: &Path) -> bool {
    if media::kind_of(path) != Kind::Video {
        return false;
    }
    let stem = path
        .file_stem()
        .unwrap_or_default()
        .to_string_lossy()
        .to_lowercase();
    let named = stem == "sample" || stem.ends_with(".sample") || stem.ends_with("-sample");
    named
        && std::fs::metadata(path)
            .map(|m| m.len() < 300 << 20)
            .unwrap_or(true)
}

fn describe((h, _, score): &(Hit, Cat, f64)) -> String {
    let d = if h.description.is_empty() {
        String::new()
    } else {
        format!(" — {}", h.description)
    };
    format!("wikipedia: {}{d}, {:.2}", h.title, score)
}

fn canon(p: &Path) -> PathBuf {
    std::fs::canonicalize(p).unwrap_or_else(|_| p.to_path_buf())
}

/// Which video a subtitle / .nfo / artwork image belongs to, if any.
/// - "Movie.en.srt", "Movie-poster.jpg" belong to "Movie.mkv" in the same folder.
/// - In a video's own folder (not the watch folder itself), a lone video also owns
///   unnamed subtitles and artwork like "poster.jpg"; so does a "Subs" subfolder of it.
/// - Other images are never attached: they are pictures in their own right.
fn sidecar_owner(
    side: &Path,
    videos: &HashMap<PathBuf, Vec<PathBuf>>,
    watch: &Path,
) -> Option<PathBuf> {
    let dir = side.parent()?;
    let name = side.file_name()?.to_string_lossy().to_lowercase();
    let stem = side.file_stem()?.to_string_lossy().to_lowercase();
    let is_image = media::kind_of(side) == Kind::Image;
    if is_image
        && !ARTWORK.is_match(&stem)
        && !videos
            .get(dir)
            .is_some_and(|vs| vs.iter().any(|v| name_matches(&name, v)))
    {
        return None;
    }
    let is_root = |d: &Path| d == watch;
    if let Some(vs) = videos.get(dir) {
        let best = vs
            .iter()
            .filter(|v| name_matches(&name, v))
            .max_by_key(|v| v.file_stem().map_or(0, std::ffi::OsStr::len));
        if best.is_some() {
            return best.cloned();
        }
        if vs.len() == 1 && !is_root(dir) {
            return Some(vs[0].clone());
        }
    }
    let dir_name = dir.file_name()?.to_string_lossy().to_lowercase();
    let parent = dir.parent()?;
    if !SIDECAR_DIRS.contains(&dir_name.as_str()) || is_root(parent) {
        return None;
    }
    match videos.get(parent) {
        Some(vs) if vs.len() == 1 => Some(vs[0].clone()),
        _ => None,
    }
}

/// "movie.en.srt" / "movie-poster.jpg" for video "Movie.mkv".
fn name_matches(name: &str, video: &Path) -> bool {
    let stem = video
        .file_stem()
        .unwrap_or_default()
        .to_string_lossy()
        .to_lowercase();
    name.strip_prefix(&stem)
        .is_some_and(|rest| rest.starts_with('.') || rest.starts_with('-'))
}

const SIDECAR_DIRS: &[&str] = &[
    "subs",
    "subtitles",
    "sub",
    "extrafanart",
    "extrathumbs",
    "artwork",
];

/// Kodi / Plex / Jellyfin artwork names.
static ARTWORK: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^(poster|fanart|folder|cover|banner|thumb|backdrop|background|clearlogo|clearart|logo|disc|discart|landscape|keyart|season\d*(-all)?-(poster|banner|fanart|landscape)|season-specials-poster)(\d*)$").unwrap()
});

#[derive(Default)]
struct Tags {
    artist: Option<String>,
    album_artist: Option<String>,
    album: Option<String>,
    title: Option<String>,
    genre: Option<String>,
}

fn read_tags(path: &Path) -> Tags {
    use lofty::prelude::*;
    let Ok(tagged) = lofty::read_from_path(path) else {
        return Tags::default();
    };
    let Some(tag) = tagged.primary_tag().or_else(|| tagged.first_tag()) else {
        return Tags::default();
    };
    let clean = |s: Option<String>| s.map(|s| s.trim().to_string()).filter(|s| !s.is_empty());
    Tags {
        artist: clean(tag.artist().map(|s| s.to_string())),
        album_artist: clean(
            tag.get_string(&lofty::tag::ItemKey::AlbumArtist)
                .map(str::to_string),
        ),
        album: clean(tag.album().map(|s| s.to_string())),
        title: clean(tag.title().map(|s| s.to_string())),
        genre: clean(tag.genre().map(|s| s.to_string())),
    }
}

/// Year a photo/video was taken: EXIF, then a date in the filename, then the file time.
fn picture_year(path: &Path, stem: &str) -> String {
    if let Some(y) = exif_year(path) {
        return y;
    }
    if let Some(c) = FILENAME_DATE.captures(stem) {
        return c[1].to_string();
    }
    let t = std::fs::metadata(path)
        .and_then(|m| m.modified())
        .unwrap_or_else(|_| SystemTime::now());
    chrono::DateTime::<chrono::Local>::from(t)
        .format("%Y")
        .to_string()
}

fn exif_year(path: &Path) -> Option<String> {
    let file = std::fs::File::open(path).ok()?;
    let exif = exif::Reader::new()
        .read_from_container(&mut std::io::BufReader::new(file))
        .ok()?;
    for tag in [
        exif::Tag::DateTimeOriginal,
        exif::Tag::DateTimeDigitized,
        exif::Tag::DateTime,
    ] {
        if let Some(f) = exif.get_field(tag, exif::In::PRIMARY) {
            if let exif::Value::Ascii(v) = &f.value {
                let s = String::from_utf8_lossy(v.first()?);
                let y = s.get(..4)?;
                if y.chars().all(|c| c.is_ascii_digit()) && y != "0000" {
                    return Some(y.to_string());
                }
            }
        }
    }
    None
}

fn unique_destination(dir: &Path, name: &std::ffi::OsStr) -> PathBuf {
    let candidate = dir.join(name);
    if !candidate.exists() {
        return candidate;
    }
    let p = Path::new(name);
    let stem = p.file_stem().unwrap_or_default().to_string_lossy();
    let ext = p
        .extension()
        .map(|e| format!(".{}", e.to_string_lossy()))
        .unwrap_or_default();
    (2..)
        .map(|i| dir.join(format!("{stem} ({i}){ext}")))
        .find(|c| !c.exists())
        .unwrap()
}

fn move_file(src: &Path, dest_dir: &Path) -> Result<()> {
    let dest = unique_destination(dest_dir, src.file_name().context("no file name")?);
    if std::fs::rename(src, &dest).is_ok() {
        return Ok(());
    }
    // Different drive: copy then delete.
    std::fs::copy(src, &dest).with_context(|| format!("copying to {}", dest.display()))?;
    std::fs::remove_file(src).with_context(|| format!("removing {}", src.display()))?;
    Ok(())
}

fn remove_empty_dirs(root: &Path, protected: &[PathBuf]) {
    let dirs: Vec<PathBuf> = WalkDir::new(root)
        .min_depth(1)
        .contents_first(true)
        .into_iter()
        .flatten()
        .filter(|e| e.file_type().is_dir())
        .map(walkdir::DirEntry::into_path)
        .collect();
    for d in dirs {
        if protected.contains(&canon(&d)) {
            continue;
        }
        let only_junk = std::fs::read_dir(&d).map(|it| {
            it.flatten().all(|e| {
                let n = e.file_name().to_string_lossy().to_lowercase();
                matches!(n.as_str(), "thumbs.db" | "desktop.ini" | ".ds_store")
            })
        });
        if only_junk.unwrap_or(false) {
            if let Ok(it) = std::fs::read_dir(&d) {
                for e in it.flatten() {
                    let _ = std::fs::remove_file(e.path());
                }
            }
            let _ = std::fs::remove_dir(&d);
        }
    }
}
