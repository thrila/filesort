//! Metadata stored inside documents: EPUB package metadata (Dublin Core) and the PDF
//! document information dictionary. Fed to the classifier next to the file name, so a
//! book can be sorted by what its publisher says it is, not only by what it is called.

use regex::Regex;
use std::io::Read;
use std::path::Path;
use std::sync::LazyLock;

/// What a document says about itself. Empty fields are left empty.
#[derive(Debug, Default, Clone)]
pub struct DocMeta {
    pub title: String,
    pub authors: Vec<String>,
    /// EPUB `dc:subject` (publisher genre tags) or PDF Subject + Keywords.
    pub subjects: Vec<String>,
    pub description: String,
}

impl DocMeta {
    pub fn is_empty(&self) -> bool {
        self.title.is_empty()
            && self.authors.is_empty()
            && self.subjects.is_empty()
            && self.description.is_empty()
    }

    /// Text for the classifier. Subjects are repeated: a publisher's genre tag says more
    /// than any single word of the title.
    pub fn classifier_text(&self) -> String {
        let subjects = self.subjects.join(" ");
        let description: String = self.description.chars().take(1500).collect();
        format!(
            "{} {} {subjects} {subjects} {description}",
            self.title,
            self.authors.join(" ")
        )
    }
}

/// PDFs are read whole to find their metadata; skip giant ones.
const MAX_PDF_BYTES: u64 = 256 << 20;

/// Remembers metadata per file (keyed on size + modification time), so files that stay
/// in the watch folder aren't re-read on every scan.
#[derive(Default)]
pub struct Cache {
    entries: std::collections::HashMap<
        std::path::PathBuf,
        (u64, Option<std::time::SystemTime>, Option<DocMeta>),
    >,
}

impl Cache {
    pub fn get(&mut self, path: &Path) -> Option<DocMeta> {
        let md = std::fs::metadata(path).ok()?;
        let key = (md.len(), md.modified().ok());
        if let Some((len, modified, meta)) = self.entries.get(path) {
            if (*len, *modified) == key {
                return meta.clone();
            }
        }
        let meta = if md.len() > MAX_PDF_BYTES {
            None
        } else {
            read(path)
        };
        self.entries
            .insert(path.to_path_buf(), (key.0, key.1, meta.clone()));
        meta
    }
}

/// Read metadata from an `.epub` or `.pdf`; None for other files or when there's nothing.
pub fn read(path: &Path) -> Option<DocMeta> {
    let ext = path.extension()?.to_string_lossy().to_ascii_lowercase();
    let meta = match ext.as_str() {
        "epub" => read_epub(path),
        "pdf" => read_pdf(path),
        _ => None,
    }?;
    (!meta.is_empty()).then_some(meta)
}

// ---- EPUB ----

const MAX_XML: u64 = 4 << 20;

static ROOTFILE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r#"(?is)<rootfile\b[^>]*\bfull-path\s*=\s*["']([^"']+)["']"#).unwrap()
});

fn dc(tag: &str) -> Regex {
    Regex::new(&format!(
        r"(?is)<(?:dc:)?{tag}\b[^>]*>(.*?)</(?:dc:)?{tag}\s*>"
    ))
    .unwrap()
}
static DC_TITLE: LazyLock<Regex> = LazyLock::new(|| dc("title"));
static DC_CREATOR: LazyLock<Regex> = LazyLock::new(|| dc("creator"));
static DC_SUBJECT: LazyLock<Regex> = LazyLock::new(|| dc("subject"));
static DC_DESCRIPTION: LazyLock<Regex> = LazyLock::new(|| dc("description"));
static TAGS: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?s)<[^>]*>").unwrap());

fn zip_text(zip: &mut zip::ZipArchive<std::fs::File>, name: &str) -> Option<String> {
    let entry = zip.by_name(name).ok()?;
    let mut s = String::new();
    entry.take(MAX_XML).read_to_string(&mut s).ok()?;
    Some(s)
}

fn read_epub(path: &Path) -> Option<DocMeta> {
    let file = std::fs::File::open(path).ok()?;
    let mut zip = zip::ZipArchive::new(file).ok()?;
    let container = zip_text(&mut zip, "META-INF/container.xml")?;
    let opf_path = ROOTFILE.captures(&container)?.get(1)?.as_str().to_string();
    let opf = zip_text(&mut zip, &opf_path)?;
    // Only the <metadata> block; the manifest can be large and has no Dublin Core.
    let meta = match (opf.find("<metadata"), opf.find("</metadata>")) {
        (Some(a), Some(b)) if b > a => &opf[a..b],
        _ => opf.as_str(),
    };
    let all = |re: &Regex| -> Vec<String> {
        re.captures_iter(meta)
            .map(|c| clean_xml_text(&c[1]))
            .filter(|s| !s.is_empty())
            .collect()
    };
    Some(DocMeta {
        title: all(&DC_TITLE).into_iter().next().unwrap_or_default(),
        authors: all(&DC_CREATOR),
        subjects: all(&DC_SUBJECT),
        description: all(&DC_DESCRIPTION).join(" "),
    })
}

/// Decode entities and drop markup (descriptions are often escaped HTML).
fn clean_xml_text(s: &str) -> String {
    let mut t = unescape(s);
    // Escaped HTML inside the description becomes real tags after one unescape.
    t = TAGS.replace_all(&t, " ").to_string();
    t = unescape(&t);
    t.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn unescape(s: &str) -> String {
    let s = s.replace("<![CDATA[", "").replace("]]>", "");
    let mut out = String::with_capacity(s.len());
    let mut rest = s.as_str();
    while let Some(i) = rest.find('&') {
        out.push_str(&rest[..i]);
        rest = &rest[i..];
        // Entities are short; look for the ';' within the next 12 characters (not bytes:
        // the text after '&' can be multi-byte, e.g. "& – x").
        let Some(end) = rest
            .char_indices()
            .take(12)
            .find(|&(_, c)| c == ';')
            .map(|(i, _)| i)
        else {
            out.push('&');
            rest = &rest[1..];
            continue;
        };
        let ent = &rest[1..end];
        let ch = match ent {
            "amp" => Some('&'),
            "lt" => Some('<'),
            "gt" => Some('>'),
            "quot" => Some('"'),
            "apos" => Some('\''),
            "nbsp" => Some(' '),
            _ if ent.starts_with("#x") || ent.starts_with("#X") => {
                u32::from_str_radix(&ent[2..], 16)
                    .ok()
                    .and_then(char::from_u32)
            }
            _ if ent.starts_with('#') => ent[1..].parse().ok().and_then(char::from_u32),
            _ => None,
        };
        match ch {
            Some(c) => {
                out.push(c);
                rest = &rest[end + 1..];
            }
            None => {
                out.push('&');
                rest = &rest[1..];
            }
        }
    }
    out.push_str(rest);
    out
}

// ---- PDF ----

fn read_pdf(path: &Path) -> Option<DocMeta> {
    pdf::info(path)
}

mod pdf {
    use super::DocMeta;
    use std::path::Path;

    /// The document information dictionary (Title, Author, Subject, Keywords).
    pub fn info(path: &Path) -> Option<DocMeta> {
        let doc = lopdf::Document::load_metadata(path).ok()?;
        let split = |s: Option<String>| -> Vec<String> {
            s.map(|s| {
                s.split([';', ',', '\n'])
                    .map(|p| p.trim().to_string())
                    .filter(|p| !p.is_empty())
                    .collect()
            })
            .unwrap_or_default()
        };
        let mut subjects = split(doc.subject);
        subjects.extend(split(doc.keywords));
        Some(DocMeta {
            title: doc.title.unwrap_or_default().trim().to_string(),
            authors: split(doc.author),
            subjects,
            description: String::new(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    #[test]
    fn epub_metadata() {
        let dir = std::env::temp_dir().join(format!("docmeta_test_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("book.epub");
        let f = std::fs::File::create(&path).unwrap();
        let mut z = zip::ZipWriter::new(f);
        let opt = zip::write::SimpleFileOptions::default();
        z.start_file("mimetype", opt).unwrap();
        z.write_all(b"application/epub+zip").unwrap();
        z.start_file("META-INF/container.xml", opt).unwrap();
        z.write_all(br#"<container><rootfiles><rootfile full-path="OEBPS/content.opf" media-type="application/oebps-package+xml"/></rootfiles></container>"#).unwrap();
        z.start_file("OEBPS/content.opf", opt).unwrap();
        z.write_all(
            br#"<package><metadata xmlns:dc="http://purl.org/dc/elements/1.1/">
                <dc:title>Caraval</dc:title>
                <dc:creator opf:role="aut">Stephanie Garber</dc:creator>
                <dc:subject>Young Adult Fiction / Fantasy / General</dc:subject>
                <dc:subject>Romance</dc:subject>
                <dc:description>&lt;p&gt;Welcome to &lt;b&gt;Caraval&lt;/b&gt; &amp;mdash; a magical game.&lt;/p&gt;</dc:description>
            </metadata><manifest/></package>"#,
        )
        .unwrap();
        z.finish().unwrap();
        let m = read(&path).unwrap();
        assert_eq!(m.title, "Caraval");
        assert_eq!(m.authors, vec!["Stephanie Garber"]);
        assert_eq!(
            m.subjects,
            vec!["Young Adult Fiction / Fantasy / General", "Romance"]
        );
        assert!(m.description.starts_with("Welcome to Caraval"));
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn entities() {
        assert_eq!(
            unescape("Tom &amp; Jerry &#39;s &#x41;"),
            "Tom & Jerry 's A"
        );
        assert_eq!(unescape("R&D"), "R&D");
        assert_eq!(
            unescape("Tom & Jerry – the movie; part 2"),
            "Tom & Jerry – the movie; part 2"
        );
    }
}
