//! Configuration file (TOML). Every folder and the run schedule live here.

use anyhow::Result;
use serde::Deserialize;
use std::collections::BTreeMap;
use std::path::PathBuf;

/// Default `[classifier]` section; also what an omitted setting falls back to.
macro_rules! classifier_defaults {
    () => {
        r#"[classifier]
# The classifier learns genres / subjects from Wikipedia: for every label below it pulls
# up to `samples_per_label` articles from the listed Wikipedia categories (and their
# subcategories, up to `max_depth` levels), then trains Naive Bayes on their titles,
# short descriptions and first sentences. Files are classified from their name plus their
# own Wikipedia summary.
#
# The training data is saved in one compact file, `training_file` (relative to this config
# file), and only pulled again for labels whose categories you change. If it's missing,
# the copy built into the program is used, so filesort works even without Wikipedia.
samples_per_label = 1000
max_depth = 3
training_file = "training.bin"

# When the model decides where a file goes (book subjects, and genre folders below) and is
# less sure than this, the file is left where it is.
min_confidence = 0.70

# Which kinds get genre / subject folders. Books always use subject folders; for movies,
# series and music the genre is otherwise only recorded (log and wikipedia.json).
#   "movies" -> Movies/<Genre>/<Title (Year)>/
#   "series" -> Series/<Genre>/<Show>/Season 01/
#   "music"  -> Music/<Genre>/<Artist>/<Album>/
genre_folders = []

# Also learn book subjects from the file names already in your library's subject folders.
learn_from_library = true

# label (= folder name) = [Wikipedia categories]
[classifier.books]
# Each label's quota is split evenly between its categories. An entry starting with "-"
# excludes subcategories / articles containing that word (keeps novels out of Computers).
Art         = ["Art history books", "-novel", "-novels", "-fiction"]
Business    = ["Business books", "Economics books", "-novel", "-novels", "-fiction"]
Computers   = ["Computer books", "Computer science books", "Computer programming books", "-novel", "-novels", "-fiction"]
Engineering = ["Engineering books", "-novel", "-novels", "-fiction"]
Fiction     = ["American young adult novels", "American fantasy novels", "Young adult fantasy novels", "Dystopian novels", "Historical novels", "Thriller novels", "Romance novels", "Mystery novels", "Science fiction novels", "Light novels", "Manga series"]
History     = ["History books", "Memoirs", "-novel", "-novels", "-fiction"]
Math        = ["Mathematics books", "Mathematics textbooks", "-novel", "-novels", "-fiction"]
Philosophy  = ["Philosophy books", "-novel", "-novels", "-fiction"]

# Extra subjects learned from your own folders (file names of the documents in them,
# subfolders included). A label here that isn't in [classifier.books] becomes a new subject.
#   Papers = ['C:\Users\Me\Documents\Papers']
[classifier.local]

[classifier.movies]
Action            = ["American action films", "Action films"]
Animation         = ["American animated feature films", "Animated films", "Anime films"]
Comedy            = ["American comedy films", "Comedy films"]
Crime             = ["American crime films", "Crime films"]
Documentary       = ["American documentary films", "Documentary films"]
Drama             = ["American drama films", "Drama films"]
Fantasy           = ["American fantasy films", "Fantasy films"]
Horror            = ["American horror films", "Horror films"]
Romance           = ["American romance films", "Romance films"]
"Science Fiction" = ["American science fiction films", "Science fiction films"]
Thriller          = ["American thriller films", "Thriller films"]
War               = ["American war films", "War films"]
Western           = ["American Western (genre) films", "Western (genre) films"]

[classifier.series]
Animation         = ["American animated television series", "Anime series", "Animated television series"]
Comedy            = ["American comedy television series", "Comedy television series"]
Crime             = ["American crime television series", "Crime television series"]
Documentary       = ["American documentary television series", "Documentary television series"]
Drama             = ["American drama television series", "British drama television series", "Drama television series"]
Fantasy           = ["American fantasy television series", "Fantasy television series"]
Horror            = ["American horror fiction television series", "Supernatural television series"]
Reality           = ["American reality television series", "Reality television series"]
"Science Fiction" = ["American science fiction television series", "Science fiction television series"]

[classifier.music]
Afrobeats           = ["Afrobeats songs"]
Classical           = ["Compositions for symphony orchestra", "Classical compositions"]
Country             = ["Country music songs"]
"Dance & Electronic" = ["Electronic songs", "Dance-pop songs"]
Folk                = ["Folk songs"]
"Hip-Hop"           = ["Hip-hop songs", "American hip-hop songs"]
Jazz                = ["Jazz songs", "Jazz compositions"]
Metal               = ["Heavy metal songs"]
Pop                 = ["Pop songs"]
"R&B & Soul"        = ["Rhythm and blues songs", "Soul songs"]
Reggae              = ["Reggae songs"]
Rock                = ["Rock songs"]
"#
    };
}

const SAMPLE_CONFIG: &str = concat!(
    r#"# filesort configuration  (filesort --help for commands)
#
# Changes are applied while filesort runs; no restart needed.
# Relative folder paths are resolved against `library_root`.
# Paths in single quotes are taken literally, so Windows backslashes are fine.

# Folder that is scanned for new media on every run.
watch_dir = '{WATCH_DIR}'

# Where books and documents go: one folder per subject (Fiction, Math, Papers, ...).
# Videos, music and pictures go to your system's own folders, see [folders] below.
library_root = '{LIBRARY_ROOT}'

# Cron expression (minute hour day-of-month month day-of-week), local time.
# "*/15 * * * *"  = every 15 minutes
# "0 * * * *"     = every hour, on the hour
# "30 2 * * *"    = every night at 02:30
schedule = "*/15 * * * *"

# Run once immediately when the service starts, before waiting for the schedule.
run_on_start = true

# true = only log what would happen. Set to false to actually move files.
dry_run = true

# Walk into subfolders of watch_dir (season packs, movie folders, photo albums...).
# false = only look at files directly inside watch_dir.
# Software projects (folders with .git, package.json, Cargo.toml, node_modules, ...) are
# never walked into either way.
recursive = true

# Delete subfolders of watch_dir that are empty after their files were moved.
remove_empty_dirs = true

# Skip files modified less than this many seconds ago (still downloading / copying).
settle_seconds = 120

# Kinds of files to leave where they are (never moved). Any of:
#   movies, series, videos (= movies + series + unidentified videos + phone clips),
#   music, pictures, books,
#   unsorted (anything that could not be identified),
#   other (= documents + archives + programs + anything else), documents, archives, programs
# Example: skip = ["music", "archives", "programs"]
skip = ["other"]

# File extensions to leave where they are, e.g. ["iso", "mkv"] (no dot).
skip_extensions = []

# Matches below this confidence (0..1) go to folders.unsorted instead.
min_confidence = 0.35

# Optional log file (appended). Leave empty to log to the console only.
log_file = ""

[folders]
# Defaults are this computer's own Videos / Music / Pictures folders. Relative paths are
# resolved against library_root.
movies   = '{MOVIES}'     # <movies>/<Title (Year)>/
series   = '{SERIES}'     # <series>/<Show>/Season 01/
unsorted = '{UNSORTED}'   # videos that couldn't be identified (folder structure kept)
music    = '{MUSIC}'      # <music>/<Artist>/<Album>/
pictures = '{PICTURES}'   # <pictures>/<Year>/
books    = "."            # <library_root>/<Subject>/
other    = "Other"        # <library_root>/Other/<Documents|Archives|Programs>/ (see skip)
cache    = '{CACHE}'      # Wikipedia lookups already made

[wikipedia]
enabled = true
lang = "en"
# Wikimedia asks API clients to identify themselves. Put a way to reach you here
# (e.g. a URL or email). It is appended to the User-Agent header.
contact = ""
# Write a wikipedia.json with the fetched summary into each movie / series folder.
write_info = true

"#,
    classifier_defaults!()
);

/// The starter config, filled in with this computer's folders: Downloads as `watch_dir`,
/// `<Documents>/Library` for books, and the system Videos / Music / Pictures folders.
pub fn sample() -> String {
    let f = Folders::default();
    let library = system_dirs::documents().join("Library");
    let show = |p: &PathBuf| p.display().to_string();
    SAMPLE_CONFIG
        .replace("{WATCH_DIR}", &show(&system_dirs::downloads()))
        .replace("{LIBRARY_ROOT}", &show(&library))
        .replace("{MOVIES}", &show(&f.movies))
        .replace("{SERIES}", &show(&f.series))
        .replace("{UNSORTED}", &show(&f.unsorted))
        .replace("{MUSIC}", &show(&f.music))
        .replace("{PICTURES}", &show(&f.pictures))
        .replace("{CACHE}", &show(&f.cache))
}

/// Names accepted in `skip`.
pub const SKIP_NAMES: &[&str] = &[
    "movies",
    "series",
    "videos",
    "music",
    "pictures",
    "books",
    "unsorted",
    "other",
    "documents",
    "archives",
    "programs",
];

#[derive(Debug, Deserialize, Clone)]
#[serde(deny_unknown_fields)]
pub struct Config {
    pub watch_dir: PathBuf,
    pub library_root: PathBuf,
    #[serde(default = "default_schedule")]
    pub schedule: String,
    #[serde(default = "yes")]
    pub run_on_start: bool,
    #[serde(default = "yes")]
    pub dry_run: bool,
    #[serde(default = "yes")]
    pub recursive: bool,
    #[serde(default = "yes")]
    pub remove_empty_dirs: bool,
    #[serde(default = "default_settle")]
    pub settle_seconds: u64,
    #[serde(default = "default_skip")]
    pub skip: Vec<String>,
    #[serde(default)]
    pub skip_extensions: Vec<String>,
    #[serde(default = "default_min_conf")]
    pub min_confidence: f64,
    #[serde(default)]
    pub log_file: String,
    #[serde(default)]
    pub folders: Folders,
    #[serde(default)]
    pub wikipedia: WikiConfig,
    #[serde(default)]
    pub classifier: ClassifierConfig,
}

#[derive(Debug, Deserialize, Clone)]
#[serde(default, deny_unknown_fields)]
pub struct Folders {
    pub movies: PathBuf,
    pub series: PathBuf,
    pub music: PathBuf,
    pub pictures: PathBuf,
    pub books: PathBuf,
    pub other: PathBuf,
    pub unsorted: PathBuf,
    pub cache: PathBuf,
}

impl Default for Folders {
    /// This computer's own folders: videos in Videos (Movies on macOS), music in Music,
    /// pictures in Pictures; books and other documents under library_root.
    fn default() -> Self {
        let videos = system_dirs::videos();
        Self {
            movies: videos.join("Movies"),
            series: videos.join("Series"),
            unsorted: videos.join("Unsorted"),
            music: system_dirs::music(),
            pictures: system_dirs::pictures(),
            books: ".".into(),
            other: "Other".into(),
            cache: config_dir().join("cache"),
        }
    }
}

/// `~/.config/filesort` (or `$XDG_CONFIG_HOME/filesort`) on every OS.
pub fn config_dir() -> PathBuf {
    let base = match std::env::var_os("XDG_CONFIG_HOME") {
        Some(d) if !d.is_empty() => PathBuf::from(d),
        _ => system_dirs::home().join(".config"),
    };
    base.join("filesort")
}

/// The OS's own user folders (Windows known folders, XDG user dirs on Linux, macOS
/// standard folders), with sensible fallbacks when one isn't set.
mod system_dirs {
    use std::path::PathBuf;
    pub fn home() -> PathBuf {
        dirs::home_dir().unwrap_or_else(|| PathBuf::from("."))
    }
    pub fn videos() -> PathBuf {
        dirs::video_dir().unwrap_or_else(|| home().join("Videos"))
    }
    pub fn music() -> PathBuf {
        dirs::audio_dir().unwrap_or_else(|| home().join("Music"))
    }
    pub fn pictures() -> PathBuf {
        dirs::picture_dir().unwrap_or_else(|| home().join("Pictures"))
    }
    pub fn downloads() -> PathBuf {
        dirs::download_dir().unwrap_or_else(|| home().join("Downloads"))
    }
    pub fn documents() -> PathBuf {
        dirs::document_dir().unwrap_or_else(|| home().join("Documents"))
    }
}

#[derive(Debug, Deserialize, Clone)]
#[serde(default, deny_unknown_fields)]
pub struct WikiConfig {
    pub enabled: bool,
    pub lang: String,
    pub contact: String,
    pub write_info: bool,
}

impl Default for WikiConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            lang: "en".into(),
            contact: String::new(),
            write_info: true,
        }
    }
}

/// Wikipedia-trained genre / subject classifier. A setting left out of the config file falls
/// back to the built-in `[classifier]` section (parsed once; every field is present there, so
/// parsing it never needs a default itself).
#[derive(Debug, Deserialize, Clone)]
#[serde(deny_unknown_fields)]
pub struct ClassifierConfig {
    #[serde(default = "cd::samples_per_label")]
    pub samples_per_label: usize,
    #[serde(default = "cd::max_depth")]
    pub max_depth: usize,
    #[serde(default = "cd::training_file")]
    pub training_file: PathBuf,
    #[serde(default = "cd::min_confidence")]
    pub min_confidence: f64,
    #[serde(default = "cd::genre_folders")]
    pub genre_folders: Vec<String>,
    #[serde(default = "cd::learn_from_library")]
    pub learn_from_library: bool,
    #[serde(default = "cd::local")]
    pub local: BTreeMap<String, Vec<PathBuf>>,
    #[serde(default = "cd::books")]
    pub books: BTreeMap<String, Vec<String>>,
    #[serde(default = "cd::movies")]
    pub movies: BTreeMap<String, Vec<String>>,
    #[serde(default = "cd::series")]
    pub series: BTreeMap<String, Vec<String>>,
    #[serde(default = "cd::music")]
    pub music: BTreeMap<String, Vec<String>>,
}

static CLASSIFIER_DEFAULTS: std::sync::LazyLock<ClassifierConfig> =
    std::sync::LazyLock::new(|| {
        #[derive(Deserialize)]
        struct Wrap {
            classifier: ClassifierConfig,
        }
        toml::from_str::<Wrap>(classifier_defaults!())
            .expect("built-in classifier defaults")
            .classifier
    });

impl Default for ClassifierConfig {
    fn default() -> Self {
        CLASSIFIER_DEFAULTS.clone()
    }
}

/// Per-field defaults for ClassifierConfig.
mod cd {
    use super::*;
    pub fn samples_per_label() -> usize {
        CLASSIFIER_DEFAULTS.samples_per_label
    }
    pub fn max_depth() -> usize {
        CLASSIFIER_DEFAULTS.max_depth
    }
    pub fn training_file() -> PathBuf {
        CLASSIFIER_DEFAULTS.training_file.clone()
    }
    pub fn min_confidence() -> f64 {
        CLASSIFIER_DEFAULTS.min_confidence
    }
    pub fn genre_folders() -> Vec<String> {
        CLASSIFIER_DEFAULTS.genre_folders.clone()
    }
    pub fn learn_from_library() -> bool {
        CLASSIFIER_DEFAULTS.learn_from_library
    }
    pub fn local() -> BTreeMap<String, Vec<PathBuf>> {
        CLASSIFIER_DEFAULTS.local.clone()
    }
    pub fn books() -> BTreeMap<String, Vec<String>> {
        CLASSIFIER_DEFAULTS.books.clone()
    }
    pub fn movies() -> BTreeMap<String, Vec<String>> {
        CLASSIFIER_DEFAULTS.movies.clone()
    }
    pub fn series() -> BTreeMap<String, Vec<String>> {
        CLASSIFIER_DEFAULTS.series.clone()
    }
    pub fn music() -> BTreeMap<String, Vec<String>> {
        CLASSIFIER_DEFAULTS.music.clone()
    }
}

#[cfg(test)]
mod classifier_defaults_tests {
    #[test]
    fn defaults_parse_and_partial_config_fills_in() {
        let d = super::ClassifierConfig::default();
        assert_eq!(d.samples_per_label, 1000);
        assert!(d.movies.contains_key("Horror"));
        let cfg = super::Config::parse(
            "watch_dir='x'
library_root='y'
[classifier]
samples_per_label = 50
",
        )
        .unwrap();
        assert_eq!(cfg.classifier.samples_per_label, 50);
        assert!(cfg.classifier.books.contains_key("Fiction"));
    }
}

impl ClassifierConfig {
    /// Labels (folder names) and their Wikipedia categories for one kind of media.
    pub fn model(&self, kind: &str) -> &BTreeMap<String, Vec<String>> {
        match kind {
            "books" => &self.books,
            "movies" => &self.movies,
            "series" => &self.series,
            _ => &self.music,
        }
    }

    /// Book subjects: the Wikipedia-trained ones plus any only learned from local folders.
    pub fn book_labels(&self) -> Vec<String> {
        let mut labels: Vec<String> = self.books.keys().cloned().collect();
        for l in self.local.keys() {
            if !labels.iter().any(|x| x.eq_ignore_ascii_case(l)) {
                labels.push(l.clone());
            }
        }
        labels
    }

    pub fn genre_folder(&self, kind: &str) -> bool {
        self.genre_folders.iter().any(|k| k == kind)
    }
}

fn yes() -> bool {
    true
}
fn default_schedule() -> String {
    "*/15 * * * *".into()
}
fn default_skip() -> Vec<String> {
    vec!["other".into()]
}
fn default_settle() -> u64 {
    120
}
fn default_min_conf() -> f64 {
    0.35
}

/// Absolute destination folders, resolved against the library root.
#[derive(Debug, Clone)]
pub struct Dirs {
    pub watch: PathBuf,
    pub movies: PathBuf,
    pub series: PathBuf,
    pub music: PathBuf,
    pub pictures: PathBuf,
    pub books: PathBuf,
    pub other: PathBuf,
    pub unsorted: PathBuf,
    pub cache: PathBuf,
}

impl Config {
    pub fn parse(text: &str) -> Result<Self> {
        let mut cfg: Config = toml::from_str(text)?;
        if !(0.0..=1.0).contains(&cfg.min_confidence) {
            anyhow::bail!("min_confidence must be between 0 and 1");
        }
        for t in &mut cfg.skip {
            *t = t.trim().to_lowercase();
            if !SKIP_NAMES.contains(&t.as_str()) {
                anyhow::bail!(
                    "unknown type {t:?} in skip; use any of: {}",
                    SKIP_NAMES.join(", ")
                );
            }
        }
        for k in &mut cfg.classifier.genre_folders {
            *k = k.trim().to_lowercase();
            if !["movies", "series", "music"].contains(&k.as_str()) {
                anyhow::bail!(
                    "unknown kind {k:?} in classifier.genre_folders; use movies, series or music"
                );
            }
        }
        if !(0.0..=1.0).contains(&cfg.classifier.min_confidence) {
            anyhow::bail!("classifier.min_confidence must be between 0 and 1");
        }
        if cfg.classifier.books.is_empty() {
            anyhow::bail!("[classifier.books] needs at least one subject");
        }
        for e in &mut cfg.skip_extensions {
            *e = e.trim().trim_start_matches('.').to_lowercase();
        }
        Ok(cfg)
    }

    pub fn dirs(&self) -> Dirs {
        let root = &self.library_root;
        let r = |p: &PathBuf| {
            if p.is_absolute() {
                p.clone()
            } else if p.as_os_str() == "." || p.as_os_str().is_empty() {
                root.clone()
            } else {
                root.join(p)
            }
        };
        Dirs {
            watch: self.watch_dir.clone(),
            movies: r(&self.folders.movies),
            series: r(&self.folders.series),
            music: r(&self.folders.music),
            pictures: r(&self.folders.pictures),
            books: r(&self.folders.books),
            other: r(&self.folders.other),
            unsorted: r(&self.folders.unsorted),
            cache: r(&self.folders.cache),
        }
    }
}
