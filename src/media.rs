//! File type detection and filename parsing.

use regex::Regex;
use std::path::Path;
use std::sync::LazyLock;
use unicode_normalization::UnicodeNormalization;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Video,
    Audio,
    Image,
    Book,
    Document,
    Archive,
    Program,
    Sidecar,
    /// Source code, build output and config files. Never moved.
    Code,
    Other,
}

pub fn kind_of(path: &Path) -> Kind {
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    match ext.as_str() {
        "mkv" | "mp4" | "m4v" | "avi" | "mov" | "wmv" | "flv" | "webm" | "mpg" | "mpeg"
        | "m2ts" | "vob" | "3gp" | "ogv" | "divx" => Kind::Video,
        "mp3" | "flac" | "m4a" | "aac" | "ogg" | "opus" | "wav" | "wma" | "alac" | "aiff"
        | "aif" | "ape" | "wv" | "m4b" => Kind::Audio,
        "jpg" | "jpeg" | "png" | "gif" | "bmp" | "tif" | "tiff" | "webp" | "heic" | "heif"
        | "avif" | "raw" | "cr2" | "cr3" | "nef" | "arw" | "dng" | "orf" | "rw2" | "svg" => {
            Kind::Image
        }
        "pdf" | "epub" | "mobi" | "azw" | "azw3" | "djvu" | "fb2" | "cbz" | "cbr" | "chm" => {
            Kind::Book
        }
        "doc" | "docx" | "odt" | "rtf" | "txt" | "md" | "xls" | "xlsx" | "ods" | "csv" | "ppt"
        | "pptx" | "odp" => Kind::Document,
        "zip" | "rar" | "7z" | "tar" | "gz" | "bz2" | "xz" | "zst" | "iso" => Kind::Archive,
        "exe" | "msi" | "msix" | "appx" | "dmg" | "pkg" | "deb" | "rpm" | "apk" | "appimage" => {
            Kind::Program
        }
        "srt" | "sub" | "idx" | "ass" | "ssa" | "vtt" | "smi" | "sup" | "nfo" => Kind::Sidecar,
        // .ts is both TypeScript and MPEG transport stream video: look at the bytes.
        "ts" => {
            if is_mpeg_ts(path) {
                Kind::Video
            } else {
                Kind::Code
            }
        }
        e if CODE_EXTENSIONS.contains(&e) => Kind::Code,
        _ => Kind::Other,
    }
}

const CODE_EXTENSIONS: &[&str] = &[
    // source
    "rs",
    "c",
    "h",
    "cc",
    "cpp",
    "cxx",
    "hh",
    "hpp",
    "hxx",
    "cs",
    "java",
    "kt",
    "kts",
    "go",
    "py",
    "pyi",
    "pyw",
    "rb",
    "php",
    "js",
    "mjs",
    "cjs",
    "jsx",
    "tsx",
    "mts",
    "cts",
    "swift",
    "m",
    "mm",
    "scala",
    "lua",
    "pl",
    "pm",
    "r",
    "dart",
    "zig",
    "nim",
    "hs",
    "ex",
    "exs",
    "erl",
    "clj",
    "fs",
    "vb",
    "asm",
    "s",
    "sh",
    "bash",
    "zsh",
    "fish",
    "ps1",
    "psm1",
    "psd1",
    "bat",
    "cmd",
    "sql",
    "vue",
    "svelte",
    "astro",
    "ipynb",
    "gd",
    // web / config / data used by programs
    "html",
    "htm",
    "css",
    "scss",
    "sass",
    "less",
    "json",
    "jsonc",
    "json5",
    "yaml",
    "yml",
    "toml",
    "xml",
    "ini",
    "cfg",
    "conf",
    "env",
    "lock",
    "map",
    "proto",
    "graphql",
    "gradle",
    "cmake",
    "mk",
    "sln",
    "csproj",
    "vcxproj",
    "props",
    "targets",
    "editorconfig",
    "gitignore",
    "dockerfile",
    // build output
    "o",
    "obj",
    "a",
    "lib",
    "dll",
    "so",
    "dylib",
    "pdb",
    "class",
    "pyc",
    "pyo",
    "wasm",
    "rlib",
    "rmeta",
    "d",
    "ilk",
    "exp",
    "node",
];

/// MPEG transport stream: 188-byte packets that each start with the 0x47 sync byte.
fn is_mpeg_ts(path: &Path) -> bool {
    use std::io::Read;
    let mut buf = [0u8; 377];
    let Ok(mut f) = std::fs::File::open(path) else {
        return false;
    };
    let mut read = 0;
    while read < buf.len() {
        match f.read(&mut buf[read..]) {
            Ok(0) | Err(_) => break,
            Ok(n) => read += n,
        }
    }
    read == buf.len() && buf[0] == 0x47 && buf[188] == 0x47 && buf[376] == 0x47
}

/// Folder names / marker files that mean "this is a software project": nothing inside is
/// ever touched, so assets of a project (images, PDFs, videos) are never pulled out of it.
const PROJECT_DIRS: &[&str] = &[
    "node_modules",
    "__pycache__",
    "venv",
    ".venv",
    "site-packages",
    "bower_components",
    "vendor",
    "target",
    "build",
    "dist",
    "out",
    "bin",
    "obj",
];
const PROJECT_MARKERS: &[&str] = &[
    ".git",
    ".hg",
    ".svn",
    "package.json",
    "Cargo.toml",
    "pyproject.toml",
    "setup.py",
    "requirements.txt",
    "go.mod",
    "pom.xml",
    "build.gradle",
    "build.gradle.kts",
    "CMakeLists.txt",
    "Makefile",
    "meson.build",
    "composer.json",
    "Gemfile",
    "deno.json",
    "tsconfig.json",
    "pubspec.yaml",
    "mix.exs",
    ".vscode",
    ".idea",
];

pub fn is_code_project(dir: &Path) -> bool {
    let name = dir
        .file_name()
        .map(|n| n.to_string_lossy().to_lowercase())
        .unwrap_or_default();
    if PROJECT_DIRS.contains(&name.as_str()) {
        return true;
    }
    if PROJECT_MARKERS.iter().any(|m| dir.join(m).exists()) {
        return true;
    }
    // A saved web page: "Page_files/" next to "Page.html" holds that page's images.
    if let (Some(page), Some(parent)) = (name.strip_suffix("_files"), dir.parent()) {
        let orig = dir
            .file_name()
            .unwrap_or_default()
            .to_string_lossy()
            .to_string();
        let page_orig = &orig[..page.len()];
        if ["html", "htm", "mhtml"]
            .iter()
            .any(|e| parent.join(format!("{page_orig}.{e}")).exists())
        {
            return true;
        }
    }
    // Source files directly inside (a WordPress theme has functions.php and no other marker),
    // or Visual Studio / .NET solutions and projects. Data files like .json or .html alone
    // don't count, so photo exports with .json metadata are still sorted.
    std::fs::read_dir(dir).is_ok_and(|it| {
        it.flatten().any(|e| {
            let n = e.file_name().to_string_lossy().to_lowercase();
            let ext = n.rsplit_once('.').map(|(_, x)| x).unwrap_or("");
            n.ends_with(".sln")
                || n.ends_with(".csproj")
                || n.ends_with(".vcxproj")
                || SOURCE_EXTENSIONS.contains(&ext)
        })
    })
}

/// Programming-language source files: one of these in a folder makes it a project.
const SOURCE_EXTENSIONS: &[&str] = &[
    "rs", "c", "h", "cc", "cpp", "cxx", "hh", "hpp", "cs", "java", "kt", "go", "py", "rb", "php",
    "js", "mjs", "cjs", "jsx", "tsx", "swift", "scala", "lua", "pl", "dart", "zig", "hs", "ex",
    "erl", "clj", "vb", "vue", "svelte", "ipynb", "gd", "sh", "ps1", "bat",
];

/// Files that are never touched: partial downloads, temp files, OS clutter, code.
pub fn is_ignored(path: &Path) -> bool {
    if kind_of(path) == Kind::Code {
        return true;
    }
    let name = path
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    if name.starts_with('.') || name.starts_with("~$") {
        return true;
    }
    if matches!(
        name.as_str(),
        "desktop.ini" | "thumbs.db" | "wikipedia.json"
    ) {
        return true;
    }
    [
        ".part",
        ".crdownload",
        ".tmp",
        ".download",
        ".!qb",
        ".!ut",
        ".partial",
        ".opdownload",
    ]
    .iter()
    .any(|s| name.ends_with(s))
}

/// Lowercase ASCII tokens, accents folded.
pub fn tokens(text: &str) -> Vec<String> {
    let folded: String = text.nfkd().filter(char::is_ascii).collect();
    let spaced = CAMEL.replace_all(&folded, "$1 $2");
    spaced
        .to_lowercase()
        .split(|c: char| !c.is_ascii_alphanumeric())
        .filter(|t| !t.is_empty())
        .map(str::to_string)
        .collect()
}

fn title_words(s: &str) -> std::collections::HashSet<String> {
    tokens(s)
        .into_iter()
        .filter(|t| !matches!(t.as_str(), "the" | "a" | "an" | "and" | "of"))
        .collect()
}

/// True when every word of `article` appears in `query` ("The Office (US)" covers "The Office").
pub fn title_covers(query: &str, article: &str) -> bool {
    let (q, a) = (title_words(query), title_words(article));
    !a.is_empty() && a.is_subset(&q)
}

/// Overlap between two titles: |A∩B| / max(|A|,|B|), ignoring "the/a/an".
pub fn title_similarity(a: &str, b: &str) -> f64 {
    let (x, y) = (title_words(a), title_words(b));
    if x.is_empty() || y.is_empty() {
        return 0.0;
    }
    x.intersection(&y).count() as f64 / x.len().max(y.len()) as f64
}

static CAMEL: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"([a-z])([A-Z])").unwrap());
static BRACKETS: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\[[^\]]*\]|\{[^}]*\}").unwrap());
/// Season/episode markers, most specific first. Groups: `s` = season, `e` = episode.
static SEASON_EPISODE: LazyLock<Vec<Regex>> = LazyLock::new(|| {
    [
        r"(?i)\bS(?P<s>\d{1,2})\s*E(?P<e>\d{1,3})(?:\s*-?\s*E\d{1,3})*\b",
        r"(?i)\b(?P<s>\d{1,2})x(?P<e>\d{2,3})\b",
        r"(?i)\b(?:Season|Series)\s*(?P<s>\d{1,2})\W*(?:Episode|Ep)\.?\s*(?P<e>\d{1,3})\b",
        r"(?i)\b(?:Season|Series)\s*(?P<s>\d{1,2})\b",
        r"(?i)\bS(?P<s>\d{1,2})\b",
    ]
    .iter()
    .map(|r| Regex::new(r).unwrap())
    .collect()
});
/// Episode markers without a season: "Episode 3", "Ep 3", "(9 of 13)".
static EPISODE_ONLY: LazyLock<Vec<Regex>> = LazyLock::new(|| {
    [
        r"(?i)\b(?:Episode|Ep)\.?\s*(?P<e>\d{1,3})\b",
        r"(?i)\(\s*(?P<e>\d{1,3})\s+of\s+\d{1,3}\s*\)",
    ]
    .iter()
    .map(|r| Regex::new(r).unwrap())
    .collect()
});
static TRAILING_NUMBER: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)(?:\s*[-#]?\s*(?:part|pt|vol|no)?\.?\s*\d{1,3})+\s*$").unwrap()
});
static YEAR: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\b(19[0-9]{2}|20[0-9]{2})\b").unwrap());
static JUNK: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"(?i)\b(2160p|1080[pi]|720p|576p|480p|4k|uhd|bluray|blu ray|brrip|bdrip|bdremux|webrip|web dl|webdl|web|hdtv|pdtv|dvdrip|dvdscr|dvd|hdrip|hdcam|camrip|x264|x265|h 264|h 265|h264|h265|hevc|avc|xvid|divx|aac\d?|ac3|eac3|dd5|ddp5|dts|truehd|atmos|remux|proper|repack|extended|unrated|uncut|remastered|imax|10bit|8bit|hdr|hdr10|dv|sdr|multi|subbed|dubbed|internal|limited|complete|amzn|nf|dsnp|hmax|atvp)\b",
    )
    .unwrap()
});
static BOOK_JUNK: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)\((?:[^()]*(?:z-?lib|1lib|library\.|libgen|anna|\.sk|\.org|\.com)[^()]*)\)|\(\s*etc\.?\s*\)").unwrap()
});
static PARENS: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\(([^()]*)\)").unwrap());
static TRACKNUM: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^\s*(?:\d{1,3}|[A-D]\d{1,2})\s*[-.)_ ]\s*").unwrap());
static CAMERA: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)^(img|vid|dsc|dscn|dscf|dcim|pxl|mvimg|wp|photo|picture|screenshot|screen shot|screen recording|snapchat|signal|whatsapp|gopr|gh\d|dji|mov|p\d{7}|sam|image|camera|burst|pano|portrait)[\s_\-]?\d|^\d{8}[\s_\-]?\d{4,9}|^[\d\s_\-]+$|^(screenshot|screen shot|whatsapp image|whatsapp video)").unwrap()
});

/// Turn "The.Matrix.1999.1080p.BluRay.x264-GRP" into "The Matrix 1999 1080p BluRay x264 GRP".
fn spaced(stem: &str) -> String {
    let s = BRACKETS.replace_all(stem, " ");
    let s = if s.matches(' ').count() < 2 {
        s.replace(['.', '_'], " ")
    } else {
        s.replace('_', " ")
    };
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn tidy(s: &str) -> String {
    s.trim_matches(|c: char| c.is_whitespace() || "-–—_.,:(".contains(c))
        .to_string()
}

#[derive(Debug, Clone, Default)]
pub struct VideoName {
    pub title: String,
    pub year: Option<u16>,
    pub season: Option<u32>,
    pub episode: Option<u32>,
    /// Starts with "1. " / "02 - ": a lesson or list item, not a movie title.
    pub numbered: bool,
}

static LESSON_PREFIX: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^\s*\d{1,3}(?:\.\d{1,3})?\s*(?:[.)]|\s-)\s+").unwrap());

pub fn parse_video(stem: &str) -> VideoName {
    let numbered = LESSON_PREFIX.is_match(stem);
    let s = spaced(&LESSON_PREFIX.replace(stem, ""));
    let mut cut = s.len();
    let mut out = VideoName {
        numbered,
        ..VideoName::default()
    };

    let found = SEASON_EPISODE
        .iter()
        .filter_map(|r| r.captures(&s))
        .find(|c| c.get(0).unwrap().start() > 0)
        .or_else(|| {
            EPISODE_ONLY
                .iter()
                .filter_map(|r| r.captures(&s))
                .find(|c| c.get(0).unwrap().start() > 0)
        });
    if let Some(c) = found {
        cut = c.get(0).unwrap().start();
        out.season = c.name("s").and_then(|x| x.as_str().parse().ok());
        out.episode = c.name("e").and_then(|x| x.as_str().parse().ok());
    }
    // Year: ignore one at the very start ("2001 A Space Odyssey"), take the last before the cut.
    for m in YEAR.find_iter(&s) {
        if m.start() == 0 || m.start() > cut {
            continue;
        }
        out.year = m.as_str().parse().ok();
        if out.season.is_none() {
            cut = m.start();
        } else {
            cut = cut.min(m.start());
        }
    }
    if let Some(m) = JUNK.find(&s) {
        if m.start() > 0 {
            cut = cut.min(m.start());
        }
    }
    out.title = tidy(&s[..cut]);
    if out.title.is_empty() {
        out.title = tidy(&s);
    }
    // "ARP: Mapping … | Networking tutorial (9 of 13)": the show is the last "|" segment.
    if out.episode.is_some() && out.season.is_none() {
        if let Some((_, show)) = out.title.rsplit_once(['|', '｜']) {
            if !tidy(show).is_empty() {
                out.title = tidy(show);
            }
        }
    }
    out
}

/// Name shared by numbered videos ("Lecture 1", "Lecture 2" -> "Lecture"), used to keep
/// look-alike unidentified videos together.
pub fn group_key(title: &str) -> String {
    let t = TRAILING_NUMBER.replace(title, "");
    let t = tidy(&t);
    if t.is_empty() {
        tidy(title)
    } else {
        t
    }
}

#[derive(Debug, Clone, Default)]
pub struct BookName {
    pub title: String,
    pub author: Option<String>,
    /// Series name when the filename has a volume marker ("… Volume-13").
    pub series: Option<String>,
    pub volume: Option<u32>,
}

static VOLUME: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)(?:^|[\s,_(\[–-])(?:volume|vol|book|tome|part|pt|no|#)\.?\s*[-_.:#]?\s*(\d{1,3})(?:[^\d]|$)").unwrap()
});

/// "Algorithm Design (Jon Kleinberg, Éva Tardos) (z-library.sk, 1lib.sk).pdf"
pub fn parse_book(stem: &str) -> BookName {
    let s = BOOK_JUNK.replace_all(stem, " ");
    let s = BRACKETS.replace_all(&s, " ");
    let s = s.replace('_', " ");
    let mut author = PARENS
        .captures(&s)
        .map(|c| tidy(&c[1]))
        .filter(|a| !a.is_empty());
    let mut title = tidy(&PARENS.replace_all(&s, " "));
    let title_ws = title.split_whitespace().collect::<Vec<_>>().join(" ");
    // "Mushoku Tensei - Jobless Reincarnation Volume-13": a volume marker means the " - " part
    // is a subtitle, not "title - author".
    if let Some(c) = VOLUME.captures(&title_ws) {
        let start = c.get(0).unwrap().start();
        let series = tidy(&title_ws[..start]);
        if !series.is_empty() {
            return BookName {
                title: title_ws.clone(),
                author,
                series: Some(series),
                volume: c[1].parse().ok(),
            };
        }
    }
    if author.is_none() {
        if let Some((t, a)) = title.rsplit_once(" - ") {
            author = Some(tidy(a));
            title = tidy(t);
        }
    }
    BookName {
        title: title.split_whitespace().collect::<Vec<_>>().join(" "),
        author,
        series: None,
        volume: None,
    }
}

#[derive(Debug, Clone, Default)]
pub struct SongName {
    pub artist: Option<String>,
    pub title: String,
}

/// "03 - Queen - Bohemian Rhapsody" / "Queen - Bohemian Rhapsody"
pub fn parse_song(stem: &str) -> SongName {
    let s = spaced(stem);
    let s = TRACKNUM.replace(&s, "");
    match s.split_once(" - ") {
        Some((a, t)) => SongName {
            artist: Some(tidy(a)),
            title: tidy(&TRACKNUM.replace(t, "")),
        },
        None => SongName {
            artist: None,
            title: tidy(&s),
        },
    }
}

/// Names produced by phones and cameras ("IMG_20230101_120000", "DSC0042").
pub fn is_camera_name(stem: &str) -> bool {
    CAMERA.is_match(stem.trim())
}

/// Strip characters Windows does not allow in folder names.
pub fn safe_dir_name(s: &str) -> String {
    let cleaned: String = s
        .chars()
        .map(|c| {
            if "<>:\"/\\|?*".contains(c) || c.is_control() {
                ' '
            } else {
                c
            }
        })
        .collect();
    let out = cleaned.split_whitespace().collect::<Vec<_>>().join(" ");
    let out = out.trim_end_matches(['.', ' ']).to_string();
    if out.is_empty() {
        "Unknown".into()
    } else {
        out
    }
}

/// "breaking bad" -> "Breaking Bad" (only when the name is all lower or all upper case).
pub fn title_case(s: &str) -> String {
    if s.chars().any(char::is_lowercase) && s.chars().any(char::is_uppercase) {
        return s.to_string();
    }
    s.split(' ')
        .map(|w| {
            let mut c = w.chars();
            match c.next() {
                Some(f) => f.to_uppercase().collect::<String>() + &c.as_str().to_lowercase(),
                None => String::new(),
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn movie() {
        let v = parse_video("The.Matrix.1999.1080p.BluRay.x264-GRP");
        assert_eq!(v.title, "The Matrix");
        assert_eq!(v.year, Some(1999));
        assert!(v.season.is_none());
        let v = parse_video("2001 A Space Odyssey (1968) [1080p]");
        assert_eq!(v.title, "2001 A Space Odyssey");
        assert_eq!(v.year, Some(1968));
        let v = parse_video("Inception");
        assert_eq!(v.title, "Inception");
    }

    #[test]
    fn lessons() {
        let v = parse_video("1. Introduction");
        assert!(v.numbered);
        assert_eq!(v.title, "Introduction");
        assert!(parse_video("02 - Setting up").numbered);
        assert!(!parse_video("1917.2019.1080p").numbered);
        assert!(!parse_video("9 (2009)").numbered);
    }

    #[test]
    fn episode() {
        let v = parse_video("Breaking.Bad.S02E05.720p.HDTV.x264");
        assert_eq!(
            (v.title.as_str(), v.season, v.episode),
            ("Breaking Bad", Some(2), Some(5))
        );
        let v = parse_video("The Office (US) 3x07 Branch Wars");
        assert_eq!((v.season, v.episode), (Some(3), Some(7)));
        assert_eq!(v.title, "The Office (US)");
        let v = parse_video("Doctor.Who.2005.S01E01.Rose");
        assert_eq!(
            (v.title.as_str(), v.year, v.season),
            ("Doctor Who", Some(2005), Some(1))
        );
    }

    #[test]
    fn season_episode_words() {
        let v = parse_video("Euphoria_Season_1_episode_3");
        assert_eq!(
            (v.title.as_str(), v.season, v.episode),
            ("Euphoria", Some(1), Some(3))
        );
        let v = parse_video("Euphoria_Season_1_episode_");
        assert_eq!(
            (v.title.as_str(), v.season, v.episode),
            ("Euphoria", Some(1), None)
        );
        let v = parse_video("Euphoria Season 2 Ep. 4 720p");
        assert_eq!(
            (v.title.as_str(), v.season, v.episode),
            ("Euphoria", Some(2), Some(4))
        );
        let v = parse_video("Sherlock Series 2 Episode 1");
        assert_eq!(
            (v.title.as_str(), v.season, v.episode),
            ("Sherlock", Some(2), Some(1))
        );
        let v = parse_video("Cosmos Episode 7");
        assert_eq!(
            (v.title.as_str(), v.season, v.episode),
            ("Cosmos", None, Some(7))
        );
        let v =
            parse_video("ARP： Mapping between IP and Ethernet ｜ Networking tutorial (9 of 13)");
        assert_eq!(
            (v.title.as_str(), v.episode),
            ("Networking tutorial", Some(9))
        );
    }

    #[test]
    fn similarity() {
        assert!(title_covers("The Office (US)", "The Office"));
        assert!(!title_covers("Cosmos", "Ultraman Cosmos"));
        assert!(!title_covers("Lecture 1", "Lecture 21"));
    }

    #[test]
    fn groups() {
        assert_eq!(group_key("Lecture 1"), "Lecture");
        assert_eq!(group_key("Rust course - Part 12"), "Rust course");
        assert_eq!(group_key("My Trip"), "My Trip");
    }

    #[test]
    fn code_is_never_media() {
        let dir = std::env::temp_dir().join(format!("filesort_test_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let ts = dir.join("asar.d.ts");
        std::fs::write(
            &ts,
            "export declare function x(): void;
",
        )
        .unwrap();
        assert_eq!(kind_of(&ts), Kind::Code);
        assert!(is_ignored(&ts));
        let video = dir.join("Show.S01E01.ts");
        let mut bytes = vec![0u8; 188 * 3];
        for i in 0..3 {
            bytes[i * 188] = 0x47;
        }
        std::fs::write(&video, bytes).unwrap();
        assert_eq!(kind_of(&video), Kind::Video);
        for f in [
            "main.rs",
            "x.c",
            "a.h",
            "index.tsx",
            "package.json",
            "lib.dll",
        ] {
            assert_eq!(kind_of(Path::new(f)), Kind::Code, "{f}");
        }
        std::fs::write(dir.join("package.json"), "{}").unwrap();
        assert!(is_code_project(&dir));
        assert!(is_code_project(Path::new("C:/x/node_modules")));
        let theme = dir.join("theme");
        std::fs::create_dir_all(&theme).unwrap();
        std::fs::write(theme.join("photo.jpg.json"), "{}").unwrap();
        assert!(
            !is_code_project(&theme),
            "json metadata alone is not a project"
        );
        std::fs::write(theme.join("functions.php"), "<?php").unwrap();
        assert!(is_code_project(&theme), "a WordPress theme is a project");
        let page = dir.join("Saved Page_files");
        std::fs::create_dir_all(&page).unwrap();
        assert!(!is_code_project(&page));
        std::fs::write(dir.join("Saved Page.html"), "<html>").unwrap();
        assert!(is_code_project(&page), "saved web page assets");
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn book() {
        let b = parse_book(
            "Algorithm Design (Jon Kleinberg, Éva Tardos) (z-library.sk, 1lib.sk, z-lib.sk)",
        );
        assert_eq!(b.title, "Algorithm Design");
        assert_eq!(b.author.as_deref(), Some("Jon Kleinberg, Éva Tardos"));
        let b = parse_book("Basic Mathematics - Serge Lang");
        assert_eq!(
            (b.title.as_str(), b.author.as_deref()),
            ("Basic Mathematics", Some("Serge Lang"))
        );
        assert!(b.series.is_none());
        let b = parse_book("Mushoku Tensei - Jobless Reincarnation Volume-13");
        assert_eq!(
            b.series.as_deref(),
            Some("Mushoku Tensei - Jobless Reincarnation")
        );
        assert_eq!(b.volume, Some(13));
        assert!(b.author.is_none());
        for (name, series, vol) in [
            (
                "The Wheel of Time Book 3 - The Dragon Reborn",
                "The Wheel of Time",
                3,
            ),
            ("One Piece, Vol. 102", "One Piece", 102),
            ("Berserk #7", "Berserk", 7),
            ("Harry Potter (J. K. Rowling) Part 2", "Harry Potter", 2),
        ] {
            let b = parse_book(name);
            assert_eq!(
                (b.series.as_deref(), b.volume),
                (Some(series), Some(vol)),
                "{name}"
            );
        }
        assert!(parse_book("Notebook 2020").series.is_none());
    }

    #[test]
    fn song() {
        let s = parse_song("03 - Queen - Bohemian Rhapsody");
        assert_eq!(
            (s.artist.as_deref(), s.title.as_str()),
            (Some("Queen"), "Bohemian Rhapsody")
        );
    }

    #[test]
    fn camera() {
        assert!(is_camera_name("IMG_20230101_120000"));
        assert!(is_camera_name("PXL_20240502_101010123"));
        assert!(is_camera_name("20230101_120000"));
        assert!(is_camera_name("DSC0042"));
        assert!(!is_camera_name("Mona Lisa"));
    }
}
