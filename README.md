# filesort

filesort keeps an eye on one folder (usually **Downloads**) and moves what lands there into
a tidy library:

```text
Movies/The Matrix (1999)/            Series/Breaking Bad/Season 01/
Music/Queen/A Night at the Opera/    Pictures/2023/Italy Trip/
Fiction/The Hunger Games/            Math/   Computers/   Papers/   ...
```

It works out what each file is from its name, its own metadata (EPUB, PDF, audio tags,
photo EXIF) and Wikipedia. Book subjects and genres come from a classifier trained on
Wikipedia. When filesort isn't sure, it leaves the file where it is. It runs on Windows,
Linux and macOS, on a schedule or as a background service.

---

## 1. Install

You need Rust (<https://rustup.rs>). Then, from this folder:

```bash
cargo install --path .
```

This builds `filesort` and puts it on your `PATH` (in `~/.cargo/bin`). Check it works:

```bash
filesort --help
```

> Linux: the HTTPS library needs OpenSSL headers (`sudo apt install libssl-dev pkg-config`
> on Debian/Ubuntu, `sudo dnf install openssl-devel` on Fedora).

## 2. Create your config

```bash
filesort init
```

This writes `~/.config/filesort/filesort.toml`. Open it and check these settings first:

| Setting | What it is | Default |
|---|---|---|
| `watch_dir` | the folder filesort sorts from | your Downloads folder |
| `library_root` | where the sorted library goes | `Documents/Library` |
| `schedule` | how often to scan (cron syntax) | every 15 minutes |
| `dry_run` | `true` = only report, move nothing | `true` |

`filesort config` shows where the config, training data and log are.

## 3. Try it safely

```bash
filesort scan --dry-run
```

One pass that only *reports* what it would do:

```text
Movie      The.Matrix.1999.1080p.mkv (+1 extra files) -> Movies\The Matrix (1999)  would move  [wikipedia: The Matrix — 1999 film …, genre Science Fiction (0.91)]
Episode    Euphoria_Season_1_episode_2.mp4 -> Series\Euphoria\Season 01  would move
Book       Six of Crows (Leigh Bardugo).epub -> Fiction  would move  [Fiction (0.99) from filename + metadata + wikipedia …]
leaving    Caraval.epub where it is (best guess Fiction at 55% is below 70%)
```

When you're happy with what it reports, set `dry_run = false` in the config.

## 4. Run it

| Command | What it does |
|---|---|
| `filesort` | keep running, scanning on the schedule |
| `filesort --now` | scan right away, then keep running on the schedule |
| `filesort scan` | one scan, then exit (for cron / Task Scheduler) |
| `filesort service install` | run in the background and start with the computer (see below) |
| `filesort evaluate` | how accurate the classifier is |
| `filesort config` | where the config, training data and log are |

Options that work with any command: `--config <path>` (or `$FILESORT_CONFIG`) to use another
config, and `--dry-run` / `--move` to override `dry_run` for that run.

**Config changes apply while it runs.** Save the file and filesort picks up the new
settings, schedule included, without a restart. If an edit is invalid, the error is logged
and the previous settings stay in effect.

## 5. Run it as a service

```bash
filesort service install      # register + start; also starts at boot / login
filesort service status
filesort service stop
filesort service start
filesort service uninstall
```

| OS | What `install` sets up | Logs |
|---|---|---|
| Windows | a Windows service "FileSort" (automatic start), running as SYSTEM. **Run the command in an Administrator terminal.** | `filesort.log` next to the config |
| Linux | a systemd user service, `~/.config/systemd/user/filesort.service` | `journalctl --user -u filesort -f` |
| macOS | a launchd agent, `~/Library/LaunchAgents/com.filesort.agent.plist` | `filesort.log` next to the config |

The service is always started with the full path of your config file, so it uses the same
config and training data as when you run filesort yourself. On Linux, to keep it running
while you're logged out: `loginctl enable-linger $USER`.

---

## Where things go

| What | Where | How it's recognised |
|---|---|---|
| Movies | `Movies/<Title (Year)>/` | file name + Wikipedia ("1999 film by …"); the genre is recorded |
| TV episodes | `Series/<Show>/Season 01/` | `S01E02`, `1x02`, `Season 1 Episode 2`, `Season_1_episode_3`, … |
| Music | `Music/<Artist>/<Album>/` | audio tags, then `Artist - Title`, then Wikipedia |
| Photos, phone videos | `Pictures/<Year>/` (a folder of photos stays together: `Pictures/<Year>/<Folder>/`) | EXIF date, then a date in the name, then the file date |
| Books, papers | `<Subject>/`, e.g. `Fiction/`, `Math/`, `Papers/` (a series gets its own folder: `Fiction/Mushoku Tensei/`) | classifier: file name + EPUB/PDF metadata + Wikipedia |
| Unidentified videos | `Unsorted/Videos/…` (folder structure kept) | |
| Documents, archives, installers | left alone by default (`skip = ["other"]`) | |

Subtitles, `.nfo` files and artwork (`poster.jpg`, `fanart.jpg`) move with their video. Movie
and series folders get a `wikipedia.json` with the article summary and the predicted genre.

**Never touched:** files still downloading (changed in the last `settle_seconds`, or
`.part`/`.crdownload`), source code and program files, software projects (folders with `.git`,
`package.json`, `Cargo.toml`, PHP or Python files, …) and saved web pages (`Page_files/`).

## Choosing what moves

```toml
skip = ["music", "archives"]       # never move these kinds
skip_extensions = ["iso"]          # or these extensions
recursive = false                  # only look at files directly in watch_dir
```

`skip` names: `movies`, `series`, `videos`, `music`, `pictures`, `books`, `unsorted`, `other`,
`documents`, `archives`, `programs`. Every folder above can be renamed or moved in `[folders]`,
for example `pictures = 'D:\Photos'`.

## The classifier

There's one Naive Bayes model each for books, movies, series and music. Each is trained on
up to 1000 Wikipedia articles per label (title, short description and categories) from the
categories listed in `[classifier.<kind>]`:

```toml
[classifier.movies]
Horror = ["American horror films", "Horror films"]

[classifier.books]
Computers = ["Computer books", "Computer programming books", "-novel", "-fiction"]   # "-" = exclude
```

- **Your own folders teach it too.** Files already in your library's subject folders count
  as examples (`learn_from_library`), and so do folders you add:
  `[classifier.local] Papers = ['C:\Users\Me\Documents\Papers']`. That also adds new
  subjects.
- **When it isn't sure, nothing moves.** Below `[classifier] min_confidence` (default 0.70)
  the file stays where it is, and the log says why.
- **Genre folders.** Movies, series and music only *record* their genre unless you set
  `genre_folders = ["movies", "series", "music"]`.
- **Training data** is kept in one compact file, `training.bin`, next to the config. A copy is
  built into the program, so filesort works offline from the first run. Change a label's
  categories and only that label is pulled from Wikipedia again. The log shows
  `training movies/Horror: 400/1000` while it does.
- **Accuracy:** `filesort evaluate` tests each model on held-out examples, and shows how many
  files would move at your threshold.

## Building from source

```bash
cargo build --release          # target/release/filesort
cargo test
cargo clippy --all-targets
```

`data/training.bin` is embedded at build time (`build.rs`). Without it, the program builds
anyway and pulls its training data from Wikipedia on first run.

## Wikipedia etiquette

Requests are made one at a time, with a pause between them. Set `[wikipedia] contact` in the
config to an email or URL: Wikimedia asks API clients to identify themselves. If Wikipedia
can't be reached or asks filesort to slow down, the affected files are retried on the next
scan instead of being guessed at.
