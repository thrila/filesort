//! The classifier's training data, kept in one compact binary file (`training.bin`).
//!
//! Layout: the 8-byte magic `MSTRAIN\x01`, then a deflate-compressed body:
//!
//! ```text
//! strings:  varint count, then each string as varint byte length + UTF-8
//! labels:   varint count, then per label:
//!             kind, label, categories-config  (string indices)
//!             status (1 byte: 0 partial, 1 complete, 2 exhausted)
//!             varint article count, then per article:
//!               title, description (string indices)
//!               varint category count, category indices
//! ```
//!
//! Every distinct string (titles, descriptions, category names) is stored once, so the
//! category names that thousands of articles share ("American non-fiction books") cost a
//! few bytes per use.

use crate::wiki::Article;
use anyhow::{bail, Context, Result};
use flate2::{read::DeflateDecoder, write::DeflateEncoder, Compression};
use std::collections::{BTreeMap, HashMap};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

const MAGIC: &[u8; 8] = b"FSTRAIN\x01";
/// Files written before the project was renamed to filesort.
const OLD_MAGIC: &[u8; 8] = b"MSTRAIN\x01";

/// The training data committed in `data/training.bin`, built into the program.
#[cfg(bundled_training)]
const BUNDLED: Option<&[u8]> = Some(include_bytes!("../data/training.bin"));
#[cfg(not(bundled_training))]
const BUNDLED: Option<&[u8]> = None;

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Status {
    /// Interrupted (network error, rate limit); finish next time.
    Partial,
    Complete,
    /// The categories have fewer articles than asked for; don't ask again.
    Exhausted,
}

#[derive(Debug, Clone)]
pub struct LabelData {
    /// The configured categories this data was pulled from, "|"-joined.
    pub categories: String,
    pub status: Status,
    pub articles: Vec<Article>,
}

pub struct Store {
    path: PathBuf,
    labels: BTreeMap<(String, String), LabelData>,
}

impl Store {
    /// Open the store at `path`. When that file is missing or unreadable it starts from,
    /// in order: per-label `.tsv` files from an older version (in
    /// `<file without extension>/<kind>/<label>.tsv`), or the copy built into the program.
    /// Whatever it starts from is saved to `path`, which is the copy that gets updated.
    pub fn open(path: &Path) -> Self {
        let mut store = Self {
            path: path.to_path_buf(),
            labels: BTreeMap::new(),
        };
        match std::fs::read(path).map(|b| decode(&b)) {
            Ok(Ok(labels)) => {
                store.labels = labels;
                return store;
            }
            Ok(Err(e)) => crate::log(&format!(
                "training data {} is unreadable ({e:#})",
                path.display()
            )),
            Err(_) => {}
        }
        let legacy = path.with_extension("");
        let n = store.import_tsv(&legacy);
        if n > 0 {
            crate::log(&format!(
                "imported {n} labels of training data from {}",
                legacy.display()
            ));
        } else if let Some(bytes) = BUNDLED {
            match decode(bytes) {
                Ok(labels) => {
                    crate::log(&format!(
                        "using the built-in training data ({} labels)",
                        labels.len()
                    ));
                    store.labels = labels;
                }
                Err(e) => crate::log(&format!("built-in training data is unreadable ({e:#})")),
            }
        }
        if !store.labels.is_empty() {
            if let Err(e) = store.save() {
                crate::log(&format!("could not save {}: {e:#}", path.display()));
            }
        }
        store
    }

    pub fn get(&self, kind: &str, label: &str) -> Option<&LabelData> {
        self.labels.get(&(kind.to_string(), label.to_string()))
    }

    /// Replace one label's data and write the file.
    pub fn put(&mut self, kind: &str, label: &str, data: LabelData) -> Result<()> {
        self.labels
            .insert((kind.to_string(), label.to_string()), data);
        self.save()
    }

    fn save(&self) -> Result<()> {
        if let Some(dir) = self.path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let bytes = encode(&self.labels)?;
        // Temp file + rename, so an interrupted save never leaves a broken file.
        let tmp = self.path.with_extension("bin.tmp");
        std::fs::write(&tmp, bytes)?;
        std::fs::rename(&tmp, &self.path)?;
        Ok(())
    }

    fn import_tsv(&mut self, dir: &Path) -> usize {
        let mut n = 0;
        for kind in crate::models::KINDS {
            let Ok(it) = std::fs::read_dir(dir.join(kind)) else {
                continue;
            };
            for e in it.flatten() {
                let p = e.path();
                if p.extension().is_some_and(|x| x == "tsv") {
                    if let (Some(label), Some(data)) = (
                        p.file_stem().map(|s| s.to_string_lossy().to_string()),
                        read_tsv(&p),
                    ) {
                        self.labels.insert((kind.to_string(), label), data);
                        n += 1;
                    }
                }
            }
        }
        n
    }
}

fn read_tsv(path: &Path) -> Option<LabelData> {
    let text = std::fs::read_to_string(path).ok()?;
    let mut data = LabelData {
        categories: String::new(),
        status: Status::Partial,
        articles: Vec::new(),
    };
    for line in text.lines() {
        if let Some(rest) = line.strip_prefix("#categories\t") {
            data.categories = rest.to_string();
        } else if let Some(rest) = line.strip_prefix("#status\t") {
            data.status = match rest.split('\t').next() {
                Some("complete") => Status::Complete,
                Some("exhausted") => Status::Exhausted,
                _ => Status::Partial,
            };
        } else if !line.starts_with('#') && !line.trim().is_empty() {
            let mut f = line.splitn(3, '\t');
            data.articles.push(Article {
                title: f.next().unwrap_or("").to_string(),
                description: f.next().unwrap_or("").to_string(),
                categories: f.next().unwrap_or("").to_string(),
            });
        }
    }
    Some(data)
}

// ---- encoding ----

#[derive(Default)]
struct Interner {
    index: HashMap<String, u64>,
    strings: Vec<String>,
}

impl Interner {
    fn id(&mut self, s: &str) -> u64 {
        if let Some(&i) = self.index.get(s) {
            return i;
        }
        let i = self.strings.len() as u64;
        self.strings.push(s.to_string());
        self.index.insert(s.to_string(), i);
        i
    }
}

fn put_varint(out: &mut Vec<u8>, mut v: u64) {
    loop {
        let b = (v & 0x7f) as u8;
        v >>= 7;
        if v == 0 {
            out.push(b);
            return;
        }
        out.push(b | 0x80);
    }
}

fn split_categories(s: &str) -> impl Iterator<Item = &str> {
    s.split(" | ").map(str::trim).filter(|c| !c.is_empty())
}

fn encode(labels: &BTreeMap<(String, String), LabelData>) -> Result<Vec<u8>> {
    let mut strings = Interner::default();
    let mut body = Vec::new();
    put_varint(&mut body, labels.len() as u64);
    for ((kind, label), d) in labels {
        for s in [kind.as_str(), label.as_str(), d.categories.as_str()] {
            put_varint(&mut body, strings.id(s));
        }
        body.push(match d.status {
            Status::Partial => 0,
            Status::Complete => 1,
            Status::Exhausted => 2,
        });
        put_varint(&mut body, d.articles.len() as u64);
        for a in &d.articles {
            put_varint(&mut body, strings.id(&a.title));
            put_varint(&mut body, strings.id(&a.description));
            let cats: Vec<&str> = split_categories(&a.categories).collect();
            put_varint(&mut body, cats.len() as u64);
            for c in cats {
                put_varint(&mut body, strings.id(c));
            }
        }
    }
    let mut raw = Vec::new();
    put_varint(&mut raw, strings.strings.len() as u64);
    for s in &strings.strings {
        put_varint(&mut raw, s.len() as u64);
        raw.extend_from_slice(s.as_bytes());
    }
    raw.extend_from_slice(&body);

    let mut out = MAGIC.to_vec();
    let mut z = DeflateEncoder::new(&mut out, Compression::new(6));
    z.write_all(&raw)?;
    z.finish()?;
    Ok(out)
}

struct Reader<'a> {
    buf: &'a [u8],
    pos: usize,
}

impl Reader<'_> {
    fn varint(&mut self) -> Result<u64> {
        let mut v = 0u64;
        for shift in (0..64).step_by(7) {
            let b = *self.buf.get(self.pos).context("truncated")?;
            self.pos += 1;
            v |= ((b & 0x7f) as u64) << shift;
            if b & 0x80 == 0 {
                return Ok(v);
            }
        }
        bail!("bad varint")
    }
    fn byte(&mut self) -> Result<u8> {
        let b = *self.buf.get(self.pos).context("truncated")?;
        self.pos += 1;
        Ok(b)
    }
    fn bytes(&mut self, n: usize) -> Result<&[u8]> {
        let end = self
            .pos
            .checked_add(n)
            .filter(|&e| e <= self.buf.len())
            .context("truncated")?;
        let s = &self.buf[self.pos..end];
        self.pos = end;
        Ok(s)
    }
}

fn decode(bytes: &[u8]) -> Result<BTreeMap<(String, String), LabelData>> {
    if bytes.len() < MAGIC.len()
        || (&bytes[..MAGIC.len()] != MAGIC && &bytes[..MAGIC.len()] != OLD_MAGIC)
    {
        bail!("not a filesort training file");
    }
    let mut raw = Vec::new();
    DeflateDecoder::new(&bytes[MAGIC.len()..])
        .read_to_end(&mut raw)
        .context("decompressing")?;
    let mut r = Reader { buf: &raw, pos: 0 };
    let n = r.varint()? as usize;
    let mut strings = Vec::with_capacity(n.min(1 << 20));
    for _ in 0..n {
        let len = r.varint()? as usize;
        strings.push(String::from_utf8(r.bytes(len)?.to_vec()).context("bad text")?);
    }
    let s = |i: u64| strings.get(i as usize).cloned().context("bad string index");
    let mut labels = BTreeMap::new();
    for _ in 0..r.varint()? {
        let (kind, label, categories) = (s(r.varint()?)?, s(r.varint()?)?, s(r.varint()?)?);
        let status = match r.byte()? {
            1 => Status::Complete,
            2 => Status::Exhausted,
            _ => Status::Partial,
        };
        let count = r.varint()? as usize;
        let mut articles = Vec::with_capacity(count.min(1 << 16));
        for _ in 0..count {
            let (title, description) = (s(r.varint()?)?, s(r.varint()?)?);
            let mut cats = Vec::new();
            for _ in 0..r.varint()? {
                cats.push(s(r.varint()?)?);
            }
            articles.push(Article {
                title,
                description,
                categories: cats.join(" | "),
            });
        }
        labels.insert(
            (kind, label),
            LabelData {
                categories,
                status,
                articles,
            },
        );
    }
    Ok(labels)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn round_trip() {
        let mut labels = BTreeMap::new();
        labels.insert(
            ("movies".to_string(), "Horror".to_string()),
            LabelData {
                categories: "American horror films|Horror films".into(),
                status: Status::Complete,
                articles: vec![
                    Article {
                        title: "Halloween".into(),
                        description: "1978 film".into(),
                        categories: "1978 films | American slasher films".into(),
                    },
                    Article {
                        title: "Scream".into(),
                        description: "1996 film by Wes Craven".into(),
                        categories: "1996 films | American slasher films".into(),
                    },
                ],
            },
        );
        let bytes = encode(&labels).unwrap();
        let back = decode(&bytes).unwrap();
        let d = &back[&("movies".to_string(), "Horror".to_string())];
        assert_eq!(d.status, Status::Complete);
        assert_eq!(d.articles[1].title, "Scream");
        assert_eq!(
            d.articles[1].categories,
            "1996 films | American slasher films"
        );
        assert!(decode(b"garbage!").is_err());
    }
}
