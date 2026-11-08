# media_sorter

Rust rewrite of the Python book sorter, extended to every kind of media. It watches a
folder (e.g. Downloads) on a cron schedule, identifies each file using Wikipedia and moves it
into your library:

| Media | Where it goes | How it is identified |
|---|---|---|
| Movies | `Movies/<Title (Year)>/` | filename → Wikipedia (`"1999 film by …"`) |
| TV episodes | `Series/<Show>/Season 01/` | `S01E02`, `1x02`, `Season 1 Episode 2`, `Season_1_episode_` in the name; canonical show name from Wikipedia (unknown shows still get their own folder) |
| Unidentified episodes / numbered videos | `Unsorted/Videos/<Name>/` | `Episode 3`, `(9 of 13)`, `Lecture 1` + `Lecture 2`: look-alike names are kept together |
| Whole series (no episode number) | `Series/<Show>/` | Wikipedia says "television series" |
| Music | `Music/<Artist>/<Album>/` | audio tags → `Artist - Title` filename → Wikipedia (`"song by Queen"`) |
| Photos & phone videos | `Pictures/<Year>/` | EXIF date → date in filename → file date |
| Photos that arrive in a folder | `Pictures/<Year>/<Folder>/` (subfolders kept) | the whole folder is filed under the year of its oldest photo, so an album is never split |
| Artwork images | `Pictures/Art/` | Wikipedia says "painting by …" |
| Book series (`Volume-13`, `Vol. 2`, `Book 3`, `#7`, or numbered titles like `Dune 1`, `Dune 2`) | `<books>/<Subject>/<Series>/` | every volume is filed under the series name, so they all land together |
| Books & papers (pdf, epub, mobi, djvu…) | `<books>/<Subject>/` | the trained classifier (see below) |
| Documents, archives, installers | `Other/<Documents\|Archives\|Programs>/` | extension (left in place by default, see `skip`) |
| Not identified | `Unsorted/<Videos\|Books>/` | |

Each episode of a series ends up in the same show folder. Subtitles, `.nfo` files and
artwork (`poster.jpg`, `fanart.jpg`) go with their video, including `Movie.en.srt` and
`Movie/Subs/English.srt`. Movie and series
folders get a `wikipedia.json` with the article title, short description, intro and link.

## The classifier

At its core this is a classifier: multinomial Naive Bayes, as described in `book_sorter.md`.
There is one model per kind of media, each trained from Wikipedia:

| Model | Labels (defaults, all configurable) | Used for |
|---|---|---|
| books | Art, Business, Computers, Engineering, Fiction, History, Math, Philosophy, plus any `[classifier.local]` label such as Papers | the subject folder |
| movies | Action, Animation, Comedy, Crime, Documentary, Drama, Fantasy, Horror, Romance, Science Fiction, Thriller, War, Western | genre, recorded in the log and `wikipedia.json` |
| series | Animation, Comedy, Crime, Documentary, Drama, Fantasy, Horror, Reality, Science Fiction | genre, recorded |
| music | Afrobeats, Classical, Country, Dance & Electronic, Folk, Hip-Hop, Jazz, Metal, Pop, R&B & Soul, Reggae, Rock | genre, recorded |

**Training data.** For every label, up to `samples_per_label` (1000) articles are pulled from
the Wikipedia categories configured for it, walking subcategories up to `max_depth` levels.
Each article contributes its title, short description and Wikipedia categories (e.g.
"1999 films | American science fiction action films | Cyberpunk films"). The book model also
learns from:

- the 8,000 Open Library titles of the old sorter,
- the file names already in your library's subject folders (`learn_from_library`),
- any folders you list in `[classifier.local]`:

```toml
[classifier.local]
Papers = ['C:\Users\Administrator\Documents\Papers']   # adds a "Papers" subject
```

File-name *shapes* are features too, so from those examples the model learns that research
papers are named like `biegler1983` (author + year) or `9404236v1` (arXiv id).

Each label's quota is split evenly between its categories, so one large category can't crowd
out the rest. Entries starting with `-` exclude subcategories and articles containing that
word. This keeps novels out of the non-fiction subjects:

```toml
Computers = ["Computer books", "Computer science books", "-novel", "-fiction"]
```

**First launch** logs `pulling classifier training data from Wikipedia…` and progress per
label (`training movies/Action: 400/1000`). Everything is saved in one compact binary file,
`data/training.bin` (about 2 MB for all four models), next to the config. Each distinct
string is stored once and the file is compressed. A label is only pulled again when its
categories change or a pull was cut short. Commit that file: if Wikipedia can't be reached,
the saved data is used. Training data from older versions (`data/training/*.tsv`) is imported
automatically.

**Classifying a file** uses its cleaned-up name plus its own Wikipedia article (description,
intro and categories) when one was found.

**When the model isn't sure, the file stays where it is.** If the model decides where a file
goes (the subject of a book, or a genre folder) and is less sure than
`[classifier] min_confidence` (default 0.70), the file is left in its current folder. The log
says why, once per file:

```text
leaving Her_books\Caraval.epub where it is (best guess Fiction at 55% is below 70%)
```

`media_sorter --evaluate` shows, per model, how accurate it is on held-out training
examples, and how many files would be moved or left at your threshold.

**Genre folders** are off by default for movies, series and music. Turn them on per kind:

```toml
[classifier]
genre_folders = ["movies"]   # Movies/<Genre>/<Title (Year)>/ ; also "series", "music"
```

## Setup

```bash
cargo build --release
```

```bash
./target/release/media_sorter --init-config media_sorter.toml
```

Edit `media_sorter.toml`. Every folder and the schedule are set there:

```toml
watch_dir    = 'C:\Users\Administrator\Downloads'           # folder checked for new media
library_root = 'C:\Users\Administrator\Documents\Library'   # relative folders below resolve against this
schedule     = "*/15 * * * *"                               # cron: min hour day month weekday
dry_run      = true                                         # set to false to actually move files

[folders]
movies = "Movies"
series = "Series"
music  = "Music"
pictures = 'D:\Photos'   # absolute paths work too
books  = "."             # keep the existing Art/, Computers/, Math/ … folders at the library root
```

### Leaving some kinds of files alone

```toml
skip = ["music", "archives", "programs"]   # never move these
skip_extensions = ["iso", "mkv"]           # or by extension
```

`skip` names: `movies`, `series`, `videos` (movies + series + unidentified videos + phone
clips), `music`, `pictures`, `books`, `unsorted` (anything that could not be identified),
`other` (documents + archives + programs + the rest), `documents`, `archives`, `programs`.
The default is `skip = ["other"]`: only media is moved. Setting `skip` replaces the default,
so add `"other"` again if you still want documents and installers left alone.

See the generated file for all options (`settle_seconds`, `recursive`, `min_confidence`,
`log_file`, Wikipedia language, classifier labels and categories…).

## Running

```bash
media_sorter --config media_sorter.toml
```

This stays running and scans on `schedule`. It runs once at start-up if `run_on_start = true`.

### Changing the config while it runs

Edit and save `media_sorter.toml`. There's no need to restart: the running sorter notices the
save and applies it straight away. Changes made during a scan apply when that scan ends.

```text
config reloaded: schedule: "*/15 * * * *" -> "0 * * * *", folders.movies: "Movies" -> "Films"
now watching C:\Users\…\Downloads -> C:\Users\…\Library | schedule "0 * * * *" | DRY RUN
next scan at 2026-11-08 10:00
```

- A new `schedule` takes effect immediately; the next scan time is recalculated.
- If the edited file is invalid (typo in a key, bad cron expression), the error is logged once
  and the previous settings stay in effect until you fix it.
- `--dry-run` / `--move` given on the command line still win over the file after a reload.
- Besides listening for save events, it re-checks the file every 30 s, so edits from tools
  whose saves Windows doesn't report are picked up too.

```bash
media_sorter --config media_sorter.toml --once
```

This runs a single pass and exits, for Windows Task Scheduler or system cron. `--dry-run`
and `--move` override `dry_run` from the config for that run.

To start it automatically at logon on Windows:

```bash
schtasks /Create /TN "MediaSorter" /SC ONLOGON /TR "\"C:\Users\Administrator\Documents\Library\book_sorter_tool\media_sorter\target\release\media_sorter.exe\" --config \"C:\Users\Administrator\Documents\Library\book_sorter_tool\media_sorter\media_sorter.toml\""
```

## Safety

- Source code and program files (`.rs`, `.c`, `.h`, `.py`, `.js`, TypeScript `.ts`, `.json`,
  `.dll`, …) are never moved. A `.ts` file only counts as video when its bytes really are an
  MPEG transport stream.
- Folders that look like software projects (`.git`, `package.json`, `Cargo.toml`,
  `pyproject.toml`, `node_modules`, `.sln`, …) are never scanned, so a project's images or
  PDFs are never pulled out of it.
- `recursive = false` limits scanning to files directly inside `watch_dir`.
- Dry run by default: it only logs `would move` until you set `dry_run = false`.
- Files changed in the last `settle_seconds` are skipped, so it waits for downloads still in
  progress. `.part`, `.crdownload` and similar files are never touched.
- If Wikipedia can't be reached or rate-limits it, affected files are **deferred** to the
  next run instead of being guessed at. Answers are cached in
  `<library>/.media_sorter_cache/wikipedia_en.json`.
- Existing files are never overwritten; a name clash becomes `name (2).ext`.
- Library folders inside the watch folder are never rescanned.
- Set `[wikipedia] contact` to a URL or email. Wikimedia asks API clients to identify
  themselves and is more lenient with them.
