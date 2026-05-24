use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::Path;

use chrono::{Local, NaiveDate, NaiveDateTime, TimeZone};
use polars::prelude::*;
use walkdir::WalkDir;

use crate::error::{CrabaseError, Result};

/// Reserved column names that come from file metadata (not frontmatter).
/// A frontmatter key colliding with any of these will be prefixed with `note_`.
/// `file_name` holds the stem (no extension) — matching the expression-language
/// semantics where `file.name` is the stem.
pub const FILE_META_COLUMNS: &[&str] = &[
    "file_path",
    "file_name",
    "file_folder",
    "file_ext",
    "file_size",
    "file_ctime",
    "file_mtime",
    "file_tags",
    "file_links",
    "file_backlinks",
    // Hidden columns: precomputed natural-sort keys for tie-breaks. They never
    // appear in user output but the query layer reads them as sort columns so
    // Obsidian-style numeric ordering ("Exodus 9" before "Exodus 19") matches.
    "file_path_natkey",
    "file_name_natkey",
];

/// Path-style natural sort key: lowercases everything and replaces each run of
/// digits with a 16-char zero-padded version so numeric runs compare
/// numerically. Punctuation is preserved with its code-point weight, which
/// matches Obsidian's tiebreak order for file paths (e.g. `Study Notes.md`
/// sorts before `Study.md` because space < period at code-point level).
pub fn natural_sort_key(s: &str) -> String {
    natural_key_inner(s, false)
}

/// User-visible sort key for string properties: like `natural_sort_key` but
/// also replaces non-alphanumeric characters (except `/`) with spaces so the
/// comparison ignores punctuation distinctions the way Obsidian's UI sort
/// does (`D. E. Shaw` lands next to `d'Vijff Vlieghen` rather than being
/// split apart by the apostrophe-vs-period code-point gap).
pub fn obsidian_sort_key(s: &str) -> String {
    natural_key_inner(s, true)
}

fn natural_key_inner(s: &str, collapse_punctuation: bool) -> String {
    const PAD: usize = 16;
    let mut out = String::with_capacity(s.len() + 16);
    let mut chars = s.chars().peekable();
    while let Some(c) = chars.next() {
        if c.is_ascii_digit() {
            let mut num = String::new();
            num.push(c);
            while let Some(&n) = chars.peek() {
                if n.is_ascii_digit() {
                    num.push(n);
                    chars.next();
                } else {
                    break;
                }
            }
            let trimmed: &str = num.trim_start_matches('0');
            let trimmed = if trimmed.is_empty() { "0" } else { trimmed };
            if trimmed.len() < PAD {
                for _ in 0..(PAD - trimmed.len()) {
                    out.push('0');
                }
            }
            out.push_str(trimmed);
        } else if collapse_punctuation && !c.is_alphanumeric() && c != '/' {
            out.push(' ');
        } else {
            for lc in c.to_lowercase() {
                out.push(lc);
            }
        }
    }
    out
}

/// Describes the LazyFrame schema produced by `scan_vault_to_lazyframe`.
/// Used by the expression translator to resolve identifiers and column dtypes.
#[derive(Debug, Clone)]
pub struct VaultSchema {
    /// All columns present in the LazyFrame and their dtypes.
    pub schema: SchemaRef,
    /// Frontmatter key → column name. Differs from the raw key only when the
    /// key collides with a reserved `file_*` metadata column (then the column
    /// gets a `note_` prefix).
    pub frontmatter_keys: HashMap<String, String>,
    /// Original DataFrame, retained so expressions like
    /// `link.asFile().properties.X` can do a cross-row lookup at translation
    /// time (build a stem → value table from a column).
    pub df: std::sync::Arc<DataFrame>,
}

impl VaultSchema {
    pub fn has_column(&self, name: &str) -> bool {
        self.schema.contains(name)
    }

    pub fn dtype(&self, name: &str) -> Option<&DataType> {
        self.schema.get(name)
    }

    /// Resolve a frontmatter key (as written in `note.foo` or bare `foo`) to
    /// its column name, accounting for the collision-prefix rule.
    pub fn resolve_frontmatter(&self, key: &str) -> Option<&str> {
        self.frontmatter_keys.get(key).map(String::as_str)
    }
}

/// Walk the vault, parse every `.md` file's frontmatter, and assemble all
/// files into a polars LazyFrame plus a description of its schema.
///
/// Canvas (`.canvas`) files don't appear as rows but they're scanned for
/// wikilinks so they contribute to backlink counts (Obsidian's
/// `file.backlinks` includes references from canvases too).
pub fn scan_vault_to_lazyframe(vault_root: &Path) -> Result<(LazyFrame, VaultSchema)> {
    let raw_files = collect_raw_files(vault_root)?;
    let canvas_links = collect_canvas_links(vault_root)?;
    let (df, mut schema) = build_dataframe(raw_files, canvas_links)?;
    schema.df = std::sync::Arc::new(df.clone());
    Ok((df.lazy(), schema))
}

/// Return relative paths of all `.base` files in the vault, sorted.
pub fn scan_bases(vault_root: &Path) -> Result<Vec<String>> {
    let mut paths: Vec<String> = WalkDir::new(vault_root)
        .follow_links(false)
        .into_iter()
        .filter_map(|e| e.ok())
        .filter(|e| e.file_type().is_file())
        .filter(|e| e.path().extension().and_then(|x| x.to_str()) == Some("base"))
        .filter_map(|e| {
            e.path()
                .strip_prefix(vault_root)
                .ok()
                .and_then(|p| p.to_str())
                .map(|s| s.replace('\\', "/"))
        })
        .collect();
    paths.sort();
    Ok(paths)
}

// ---------- Internal: raw file representation ----------

/// Pre-DataFrame snapshot of a single markdown file. Used during ingest only.
struct RawFile {
    rel_path: String,
    /// File stem (no extension). Mapped to column `file_name`.
    stem: String,
    ext: String,
    folder: String,
    size: u64,
    ctime: Option<NaiveDateTime>,
    mtime: Option<NaiveDateTime>,
    frontmatter: BTreeMap<String, serde_yaml::Value>,
    tags: Vec<String>,
    /// Wikilinks parsed from the body only — matches what Obsidian exposes
    /// through `file.links`.
    links: Vec<String>,
    /// Wikilinks parsed from frontmatter + body. Used for backlink resolution,
    /// where Obsidian does count `[[…]]` mentions embedded in frontmatter
    /// strings (e.g. a `comment:` field that name-drops another note).
    all_links: Vec<String>,
}

fn collect_raw_files(vault_root: &Path) -> Result<Vec<RawFile>> {
    WalkDir::new(vault_root)
        .follow_links(false)
        .into_iter()
        .filter_map(|e| e.ok())
        .filter(|e| e.file_type().is_file())
        .filter(|entry| entry.path().extension().and_then(|ext| ext.to_str()) == Some("md"))
        .map(|entry| read_raw_file(vault_root, entry.path()))
        .collect()
}

/// External wikilink source: a non-markdown file (currently `.canvas`) that
/// contributes to backlink counts but doesn't show up as a row in the
/// LazyFrame.
struct CanvasLinks {
    rel_path: String,
    links: Vec<String>,
}

fn collect_canvas_links(vault_root: &Path) -> Result<Vec<CanvasLinks>> {
    let mut out = Vec::new();
    for entry in WalkDir::new(vault_root)
        .follow_links(false)
        .into_iter()
        .filter_map(|e| e.ok())
        .filter(|e| e.file_type().is_file())
        .filter(|e| e.path().extension().and_then(|x| x.to_str()) == Some("canvas"))
    {
        let abs_path = entry.path();
        let rel_path = abs_path
            .strip_prefix(vault_root)
            .map_err(|e| CrabaseError::Io(std::io::Error::other(e.to_string())))?
            .to_string_lossy()
            .replace('\\', "/");
        let Ok(content) = std::fs::read_to_string(abs_path) else {
            continue;
        };
        // Canvas files are JSON; the embedded note bodies live in `text`
        // properties on text nodes. We just extract the strings without
        // parsing the JSON — the wikilink regex is unambiguous against the
        // surrounding JSON syntax.
        let mut links = extract_wikilinks(&content);
        // The structured form `"file": "path/to/note.md"` also counts as a
        // link to that file. Pull those out by hand.
        for line in content.lines() {
            if let Some(idx) = line.find("\"file\":") {
                let rest = &line[idx + 7..];
                if let Some(start) = rest.find('"') {
                    let after = &rest[start + 1..];
                    if let Some(end) = after.find('"') {
                        let target = &after[..end];
                        if !target.is_empty() {
                            links.push(target.to_string());
                        }
                    }
                }
            }
        }
        out.push(CanvasLinks { rel_path, links });
    }
    Ok(out)
}

fn read_raw_file(vault_root: &Path, abs_path: &Path) -> Result<RawFile> {
    let ext = abs_path
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_string();
    let rel_path = abs_path
        .strip_prefix(vault_root)
        .map_err(|e| CrabaseError::Io(std::io::Error::other(e.to_string())))?
        .to_string_lossy()
        .replace('\\', "/");
    let stem = abs_path
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("")
        .to_string();
    let folder = abs_path
        .parent()
        .and_then(|p| p.strip_prefix(vault_root).ok())
        .map(|p| p.to_string_lossy().replace('\\', "/"))
        .unwrap_or_default();
    let meta = std::fs::metadata(abs_path)?;
    let size = meta.len();
    let systime_to_naive = |t: std::time::SystemTime| -> Option<NaiveDateTime> {
        let secs = t.duration_since(std::time::UNIX_EPOCH).ok()?.as_secs() as i64;
        let utc = chrono::DateTime::from_timestamp(secs, 0)?;
        Some(Local.from_utc_datetime(&utc.naive_utc()).naive_local())
    };
    let mtime = meta.modified().ok().and_then(systime_to_naive);
    let ctime = meta.created().ok().and_then(systime_to_naive);
    let content = std::fs::read_to_string(abs_path)?;
    let (frontmatter, body) = parse_frontmatter(&content);
    let tags = extract_frontmatter_tags(&frontmatter)
        .into_iter()
        .chain(extract_inline_tags(&body))
        .fold(Vec::new(), |mut acc, tag| {
            if !acc.contains(&tag) {
                acc.push(tag);
            }
            acc
        });
    // Obsidian counts frontmatter wikilinks only when the *entire* string
    // value (or each element of a sequence) is a single `[[…]]` token — i.e.
    // a property that Obsidian itself recognizes as a typed link. Inline
    // wikilinks embedded inside a longer string (e.g. a `comment:` field)
    // don't count toward `file.links`.
    let mut links = extract_frontmatter_link_values(&frontmatter);
    links.extend(extract_wikilinks(&body));
    let all_links = {
        let mut v = links.clone();
        v.extend(extract_inline_frontmatter_wikilinks(&frontmatter));
        v
    };

    Ok(RawFile {
        rel_path,
        stem,
        ext,
        folder,
        size,
        ctime,
        mtime,
        frontmatter,
        tags,
        links,
        all_links,
    })
}

// ---------- Frontmatter & body parsing (carry over from previous impl) ----------

fn parse_frontmatter(content: &str) -> (BTreeMap<String, serde_yaml::Value>, String) {
    if !content.starts_with("---\n") && !content.starts_with("---\r\n") {
        return (BTreeMap::new(), content.to_string());
    }

    let after_open = if let Some(stripped) = content.strip_prefix("---\r\n") {
        stripped
    } else {
        &content[4..]
    };

    let close_pos = after_open
        .lines()
        .scan(0usize, |pos, line| {
            let cur = *pos;
            *pos += line.len() + 1;
            Some((cur, line))
        })
        .find(|(_, line)| *line == "---" || *line == "---\r")
        .map(|(pos, _)| pos);

    let Some(close_pos) = close_pos else {
        return (BTreeMap::new(), content.to_string());
    };

    let yaml_str = &after_open[..close_pos];
    let rest_start = close_pos + 4;
    let body = if rest_start <= after_open.len() {
        after_open[rest_start..].to_string()
    } else {
        String::new()
    };

    let map: BTreeMap<String, serde_yaml::Value> =
        serde_yaml::from_str(yaml_str).unwrap_or_default();
    (map, body)
}

fn extract_frontmatter_tags(frontmatter: &BTreeMap<String, serde_yaml::Value>) -> Vec<String> {
    let Some(tags_val) = frontmatter.get("tags") else {
        return Vec::new();
    };
    match tags_val {
        serde_yaml::Value::Sequence(seq) => seq
            .iter()
            .filter_map(|v| v.as_str().map(|s| s.trim_start_matches('#').to_string()))
            .collect(),
        serde_yaml::Value::String(s) => vec![s.trim_start_matches('#').to_string()],
        _ => Vec::new(),
    }
}

fn extract_inline_tags(body: &str) -> Vec<String> {
    body.lines()
        .flat_map(extract_inline_tags_from_line)
        .collect()
}

fn extract_inline_tags_from_line(line: &str) -> Vec<String> {
    let mut chars = line.char_indices().peekable();
    let mut tags = Vec::new();

    while let Some((i, c)) = chars.next() {
        if c != '#' {
            continue;
        }

        let tag: String = chars
            .by_ref()
            .take_while(|(_, c)| c.is_alphanumeric() || *c == '/' || *c == '_' || *c == '-')
            .map(|(_, c)| c)
            .collect();

        let is_valid_tag = !tag.is_empty()
            && tag.chars().next().is_some_and(char::is_alphabetic)
            && (i == 0 || line[..i].ends_with(char::is_whitespace));

        if is_valid_tag {
            tags.push(tag);
        }
    }

    tags
}

/// Pull link targets out of frontmatter values that are *entire* wikilink
/// strings. Obsidian treats e.g. `speaker: "[[Foo Bar]]"` as a typed link
/// property (so `[[Foo Bar]]` contributes to `file.links`), but does NOT
/// count `[[Foo Bar]]` embedded inside a longer string like
/// `comment: "see [[Foo Bar]] for context"`.
fn extract_frontmatter_link_values(
    frontmatter: &BTreeMap<String, serde_yaml::Value>,
) -> Vec<String> {
    fn as_link(s: &str) -> Option<String> {
        let trimmed = s.trim();
        let inner = trimmed.strip_prefix("[[")?.strip_suffix("]]")?;
        // Reject anything that looks like more than a single wikilink — if
        // there's another `[[` inside, the value isn't a pure link.
        if inner.contains("[[") || inner.contains("]]") {
            return None;
        }
        let target = match inner.find('|') {
            Some(p) => &inner[..p],
            None => inner,
        };
        let t = target.trim();
        if t.is_empty() {
            None
        } else {
            Some(t.to_string())
        }
    }
    fn collect(v: &serde_yaml::Value, out: &mut Vec<String>) {
        match v {
            serde_yaml::Value::String(s) => {
                if let Some(link) = as_link(s) {
                    out.push(link);
                }
            }
            serde_yaml::Value::Sequence(seq) => {
                for item in seq {
                    collect(item, out);
                }
            }
            _ => {}
        }
    }
    let mut out = Vec::new();
    for v in frontmatter.values() {
        collect(v, &mut out);
    }
    out
}

/// Wikilinks embedded inside non-link frontmatter strings (e.g. a `comment:`
/// field). Used only for backlink resolution — they don't appear in
/// `file.links`.
fn extract_inline_frontmatter_wikilinks(
    frontmatter: &BTreeMap<String, serde_yaml::Value>,
) -> Vec<String> {
    fn collect(v: &serde_yaml::Value, out: &mut Vec<String>) {
        match v {
            serde_yaml::Value::String(s) => {
                let trimmed = s.trim();
                let is_pure_link = trimmed.starts_with("[[")
                    && trimmed.ends_with("]]")
                    && !trimmed[2..].contains("[[");
                if !is_pure_link {
                    out.extend(extract_wikilinks(s));
                }
            }
            serde_yaml::Value::Sequence(seq) => {
                for item in seq {
                    collect(item, out);
                }
            }
            _ => {}
        }
    }
    let mut out = Vec::new();
    for v in frontmatter.values() {
        collect(v, &mut out);
    }
    out
}

/// Strip Markdown code regions (fenced ``` blocks and inline `` `code` `` spans)
/// from a body string, replacing them with spaces so byte offsets are preserved.
/// Obsidian's link parser does not surface wikilinks that live inside code; the
/// link parser running over the stripped body therefore agrees with Obsidian.
fn strip_code_regions(content: &str) -> String {
    let bytes = content.as_bytes();
    let mut out: Vec<u8> = Vec::with_capacity(bytes.len());
    let mut i = 0;
    let mut at_line_start = true;
    while i < bytes.len() {
        let b = bytes[i];
        // Fenced code block: look for ``` (or more) at line start. Closes on
        // the next line that starts with the same fence char and >= fence len.
        if at_line_start && b == b'`' {
            let mut fence_len = 0;
            while i + fence_len < bytes.len() && bytes[i + fence_len] == b'`' {
                fence_len += 1;
            }
            if fence_len >= 3 {
                // Consume the fence-open line entirely (including any info string).
                let line_end = bytes[i..]
                    .iter()
                    .position(|&c| c == b'\n')
                    .map(|p| i + p)
                    .unwrap_or(bytes.len());
                for _ in i..line_end {
                    out.push(b' ');
                }
                i = line_end;
                if i < bytes.len() {
                    out.push(b'\n');
                    i += 1;
                }
                // Scan lines until a closing fence.
                while i < bytes.len() {
                    let line_start = i;
                    let line_end = bytes[i..]
                        .iter()
                        .position(|&c| c == b'\n')
                        .map(|p| i + p)
                        .unwrap_or(bytes.len());
                    let line = &bytes[line_start..line_end];
                    let trimmed_start = line
                        .iter()
                        .position(|&c| c != b' ' && c != b'\t')
                        .unwrap_or(line.len());
                    let mut close_len = 0;
                    while trimmed_start + close_len < line.len()
                        && line[trimmed_start + close_len] == b'`'
                    {
                        close_len += 1;
                    }
                    let only_fence = trimmed_start + close_len == line.len()
                        || line[trimmed_start + close_len..]
                            .iter()
                            .all(|&c| c == b' ' || c == b'\t');
                    if close_len >= fence_len && only_fence {
                        for _ in line_start..line_end {
                            out.push(b' ');
                        }
                        i = line_end;
                        if i < bytes.len() {
                            out.push(b'\n');
                            i += 1;
                        }
                        break;
                    }
                    for _ in line_start..line_end {
                        out.push(b' ');
                    }
                    i = line_end;
                    if i < bytes.len() {
                        out.push(b'\n');
                        i += 1;
                    }
                }
                at_line_start = true;
                continue;
            }
        }
        // Inline code span: `..` or ``..`` etc. Match same-count backticks
        // on the same line (Obsidian only crosses lines for fenced blocks).
        if b == b'`' {
            let mut tick_len = 0;
            while i + tick_len < bytes.len() && bytes[i + tick_len] == b'`' {
                tick_len += 1;
            }
            let scan_start = i + tick_len;
            let mut j = scan_start;
            let mut closed_at: Option<usize> = None;
            while j < bytes.len() {
                if bytes[j] == b'\n' {
                    break;
                }
                if bytes[j] == b'`' {
                    let mut k = 0;
                    while j + k < bytes.len() && bytes[j + k] == b'`' {
                        k += 1;
                    }
                    if k == tick_len {
                        closed_at = Some(j);
                        break;
                    }
                    j += k;
                    continue;
                }
                j += 1;
            }
            if let Some(end) = closed_at {
                for _ in i..end + tick_len {
                    out.push(b' ');
                }
                i = end + tick_len;
                at_line_start = false;
                continue;
            }
            // Unmatched: fall through and emit the backticks literally.
        }
        out.push(b);
        at_line_start = b == b'\n';
        i += 1;
    }
    // Safe: we only ever emit ASCII bytes or copy existing bytes from `content`,
    // and we never split a multi-byte UTF-8 sequence (we only inspect ASCII bytes
    // and copy others verbatim).
    String::from_utf8(out).unwrap_or_else(|_| content.to_string())
}

fn extract_wikilinks(content: &str) -> Vec<String> {
    let stripped = strip_code_regions(content);
    let mut links = Vec::new();
    let mut i = 0;
    let bytes = stripped.as_bytes();
    while i + 1 < bytes.len() {
        if bytes[i] == b'[' && bytes[i + 1] == b'[' {
            let start = i + 2;
            let end = stripped[start..].find("]]").map(|pos| start + pos);
            if let Some(end) = end {
                let inner = &stripped[start..end];
                // Drop the alias/anchor suffix: the link target is everything
                // before the first `|` or `#`. Obsidian resolves
                // `[[Colossians 4#16|Colossians 4:16]]` as a backlink to
                // `Colossians 4`, not to `Colossians 4#16`.
                let cut = inner
                    .find(|c: char| c == '|' || c == '#')
                    .unwrap_or(inner.len());
                let link_target = inner[..cut].trim();
                if !link_target.is_empty() {
                    links.push(link_target.to_string());
                }
                i = end + 2;
                continue;
            }
        }
        i += 1;
    }
    links
}

// ---------- DataFrame construction ----------

fn build_dataframe(
    raw_files: Vec<RawFile>,
    canvas_links: Vec<CanvasLinks>,
) -> Result<(DataFrame, VaultSchema)> {
    let n = raw_files.len();

    // Fixed file metadata columns -------------------------------------------
    let mut file_path = Vec::with_capacity(n);
    let mut file_name = Vec::with_capacity(n);
    let mut file_folder = Vec::with_capacity(n);
    let mut file_ext = Vec::with_capacity(n);
    let mut file_size = Vec::with_capacity(n);
    let mut file_ctime = Vec::with_capacity(n);
    let mut file_mtime = Vec::with_capacity(n);
    let mut file_tags: Vec<Series> = Vec::with_capacity(n);
    let mut file_links: Vec<Series> = Vec::with_capacity(n);

    for f in &raw_files {
        file_path.push(f.rel_path.clone());
        file_name.push(f.stem.clone());
        file_folder.push(f.folder.clone());
        file_ext.push(f.ext.clone());
        file_size.push(f.size);
        file_ctime.push(f.ctime.map(naive_to_micros));
        file_mtime.push(f.mtime.map(naive_to_micros));
        file_tags.push(Series::new("".into(), &f.tags));
        file_links.push(Series::new("".into(), &f.links));
    }

    let backlinks_per_file = compute_backlinks(&raw_files, &canvas_links);
    let file_backlinks: Vec<Series> = backlinks_per_file
        .into_iter()
        .map(|links| Series::new("".into(), &links))
        .collect();

    let file_path_natkey: Vec<String> = file_path.iter().map(|s| natural_sort_key(s)).collect();
    let file_name_natkey: Vec<String> = file_name.iter().map(|s| natural_sort_key(s)).collect();

    let mut columns: Vec<Column> = vec![
        Column::new("file_path".into(), file_path),
        Column::new("file_name".into(), file_name),
        Column::new("file_folder".into(), file_folder),
        Column::new("file_ext".into(), file_ext),
        Column::new("file_size".into(), file_size),
        datetime_column("file_ctime", &file_ctime)?,
        datetime_column("file_mtime", &file_mtime)?,
        list_string_column("file_tags", file_tags)?,
        list_string_column("file_links", file_links)?,
        list_string_column("file_backlinks", file_backlinks)?,
        Column::new("file_path_natkey".into(), file_path_natkey),
        Column::new("file_name_natkey".into(), file_name_natkey),
    ];

    // Frontmatter columns ---------------------------------------------------
    let reserved: HashSet<&str> = FILE_META_COLUMNS.iter().copied().collect();
    let mut all_keys: HashSet<String> = HashSet::new();
    for f in &raw_files {
        for k in f.frontmatter.keys() {
            all_keys.insert(k.clone());
        }
    }
    let mut ordered_keys: Vec<String> = all_keys.into_iter().collect();
    ordered_keys.sort();

    let mut frontmatter_keys: HashMap<String, String> = HashMap::new();
    for key in &ordered_keys {
        let column_name = if reserved.contains(key.as_str()) {
            format!("note_{key}")
        } else {
            key.clone()
        };
        if frontmatter_keys.values().any(|v| v == &column_name) {
            return Err(CrabaseError::Query(format!(
                "Frontmatter key collision producing column '{column_name}' from multiple sources"
            )));
        }
        let values: Vec<&serde_yaml::Value> = raw_files
            .iter()
            .map(|f| f.frontmatter.get(key).unwrap_or(&serde_yaml::Value::Null))
            .collect();
        let dtype = infer_dtype(&values);
        let column = build_frontmatter_column(&column_name, &values, &dtype)?;
        columns.push(column);
        frontmatter_keys.insert(key.clone(), column_name);
    }

    let df = DataFrame::new(columns)?;
    let schema = VaultSchema {
        schema: df.schema().clone(),
        frontmatter_keys,
        df: std::sync::Arc::new(DataFrame::empty()),
    };
    Ok((df, schema))
}

fn naive_to_micros(dt: NaiveDateTime) -> i64 {
    dt.and_utc().timestamp_micros()
}

/// For each file, return the paths of files that link to it. A link target
/// matches a file if it is either:
///   - the file's relative path (with or without `.md`)
///   - the file's stem (basename, no extension), resolved against the linking
///     file's location: when multiple files share a stem we pick the one in
///     the closest folder (longest common path prefix with the source's
///     folder, then alphabetic) — Obsidian's "shortest path that uniquely
///     identifies" rule reduces to this in practice.
fn compute_backlinks(raw_files: &[RawFile], canvas_links: &[CanvasLinks]) -> Vec<Vec<String>> {
    use std::collections::HashMap;

    let mut by_stem: HashMap<&str, Vec<&str>> = HashMap::new();
    let mut by_path: HashMap<&str, &str> = HashMap::new();
    let mut by_path_no_ext: HashMap<&str, &str> = HashMap::new();
    for f in raw_files {
        by_stem
            .entry(f.stem.as_str())
            .or_default()
            .push(f.rel_path.as_str());
        by_path.insert(f.rel_path.as_str(), f.rel_path.as_str());
        if let Some(no_ext) = f.rel_path.strip_suffix(".md") {
            by_path_no_ext.insert(no_ext, f.rel_path.as_str());
        }
    }
    // Stable resolution order: alphabetic by path so ties pick the same file
    // every run.
    for paths in by_stem.values_mut() {
        paths.sort_unstable();
    }

    let mut targets: HashMap<String, Vec<String>> = HashMap::new();
    let resolve = |link: &str, source_folder: &str| -> Option<String> {
        by_path
            .get(link)
            .copied()
            .or_else(|| by_path_no_ext.get(link).copied())
            .or_else(|| {
                by_stem
                    .get(link)
                    .map(|candidates| resolve_closest(candidates, source_folder))
            })
            .map(str::to_string)
    };
    for f in raw_files {
        for link in &f.all_links {
            if let Some(target) = resolve(link.as_str(), f.folder.as_str()) {
                targets.entry(target).or_default().push(f.rel_path.clone());
            }
        }
    }
    for c in canvas_links {
        let folder = c.rel_path.rsplit_once('/').map(|(d, _)| d).unwrap_or("");
        for link in &c.links {
            if let Some(target) = resolve(link.as_str(), folder) {
                targets.entry(target).or_default().push(c.rel_path.clone());
            }
        }
    }

    raw_files
        .iter()
        .map(|f| {
            let mut v: Vec<String> = targets
                .get(f.rel_path.as_str())
                .cloned()
                .unwrap_or_default();
            v.sort();
            v.dedup();
            v
        })
        .collect()
}

/// Pick the candidate file whose folder shares the longest path-segment prefix
/// with `source_folder`. Candidates must already be sorted alphabetically so
/// the tie-break is deterministic.
fn resolve_closest<'a>(candidates: &[&'a str], source_folder: &str) -> &'a str {
    let source_segments: Vec<&str> = source_folder.split('/').filter(|s| !s.is_empty()).collect();
    let mut best_idx = 0usize;
    let mut best_match = -1i32;
    for (i, &path) in candidates.iter().enumerate() {
        let folder = path.rsplit_once('/').map(|(d, _)| d).unwrap_or("");
        let target_segments: Vec<&str> = folder.split('/').filter(|s| !s.is_empty()).collect();
        let shared = source_segments
            .iter()
            .zip(target_segments.iter())
            .take_while(|(a, b)| a == b)
            .count() as i32;
        if shared > best_match {
            best_match = shared;
            best_idx = i;
        }
    }
    candidates[best_idx]
}

fn datetime_column(name: &str, values: &[Option<i64>]) -> Result<Column> {
    let s = Series::new(name.into(), values);
    let s = s.cast(&DataType::Datetime(TimeUnit::Microseconds, None))?;
    Ok(s.into_column())
}

fn list_string_column(name: &str, series_per_row: Vec<Series>) -> Result<Column> {
    let chunked: ListChunked = ListChunked::from_iter(series_per_row.into_iter().map(Some));
    let series = chunked.into_series().with_name(name.into());
    Ok(series.into_column())
}

// ---------- Dtype inference ----------

#[derive(Debug, Clone, Copy)]
enum TypeProbe {
    Int,
    Float,
    Bool,
    Date,
    Datetime,
    StringList,
    String,
}

fn probe(v: &serde_yaml::Value) -> Option<TypeProbe> {
    match v {
        serde_yaml::Value::Null => None,
        serde_yaml::Value::Bool(_) => Some(TypeProbe::Bool),
        serde_yaml::Value::Number(n) => {
            if n.as_i64().is_some() {
                Some(TypeProbe::Int)
            } else {
                Some(TypeProbe::Float)
            }
        }
        serde_yaml::Value::String(s) => Some(probe_string(s)),
        serde_yaml::Value::Sequence(_) => Some(TypeProbe::StringList),
        serde_yaml::Value::Mapping(_) => Some(TypeProbe::String),
        serde_yaml::Value::Tagged(t) => probe(&t.value).or(Some(TypeProbe::String)),
    }
}

fn probe_string(s: &str) -> TypeProbe {
    // Only treat *bare* date strings as Date/Datetime. Strings with wikilink
    // wrappers (`[[2025-04-27]]`) stay as String so they round-trip through
    // the CSV output unchanged.
    if NaiveDateTime::parse_from_str(s, "%Y-%m-%dT%H:%M:%S").is_ok()
        || NaiveDateTime::parse_from_str(s, "%Y-%m-%d %H:%M:%S").is_ok()
    {
        TypeProbe::Datetime
    } else if NaiveDate::parse_from_str(s, "%Y-%m-%d").is_ok() {
        TypeProbe::Date
    } else {
        TypeProbe::String
    }
}

fn strip_wikilink(s: &str) -> &str {
    let trimmed = s.trim();
    let no_open = trimmed.strip_prefix("[[").unwrap_or(trimmed);
    no_open.strip_suffix("]]").unwrap_or(no_open)
}

fn infer_dtype(values: &[&serde_yaml::Value]) -> DataType {
    let probes: Vec<TypeProbe> = values.iter().filter_map(|v| probe(v)).collect();
    if probes.is_empty() {
        return DataType::String;
    }
    let mut has_int = false;
    let mut has_float = false;
    let mut has_bool = false;
    let mut has_date = false;
    let mut has_datetime = false;
    let mut has_list = false;
    let mut has_string = false;
    for p in &probes {
        match p {
            TypeProbe::Int => has_int = true,
            TypeProbe::Float => has_float = true,
            TypeProbe::Bool => has_bool = true,
            TypeProbe::Date => has_date = true,
            TypeProbe::Datetime => has_datetime = true,
            TypeProbe::StringList => has_list = true,
            TypeProbe::String => has_string = true,
        }
    }
    let only =
        |a: bool, b: bool, c: bool, d: bool, e: bool, f: bool| a && !b && !c && !d && !e && !f;
    if only(
        has_int,
        has_float,
        has_bool,
        has_date,
        has_datetime,
        has_list || has_string,
    ) {
        DataType::Int64
    } else if !has_bool
        && !has_date
        && !has_datetime
        && !has_list
        && !has_string
        && (has_int || has_float)
    {
        DataType::Float64
    } else if only(
        has_bool,
        has_int,
        has_float,
        has_date,
        has_datetime,
        has_list || has_string,
    ) {
        DataType::Boolean
    } else if !has_int && !has_float && !has_bool && !has_list && !has_string && has_datetime {
        DataType::Datetime(TimeUnit::Microseconds, None)
    } else if !has_int
        && !has_float
        && !has_bool
        && !has_list
        && !has_string
        && (has_date || has_datetime)
    {
        // Mix of date + datetime → promote to Datetime for uniform storage
        if has_datetime {
            DataType::Datetime(TimeUnit::Microseconds, None)
        } else {
            DataType::Date
        }
    } else if only(
        has_list,
        has_int,
        has_float,
        has_bool,
        has_date,
        has_datetime || has_string,
    ) {
        DataType::List(Box::new(DataType::String))
    } else {
        DataType::String
    }
}

// ---------- Frontmatter value conversion ----------

fn build_frontmatter_column(
    name: &str,
    values: &[&serde_yaml::Value],
    dtype: &DataType,
) -> Result<Column> {
    match dtype {
        DataType::Int64 => {
            let xs: Vec<Option<i64>> = values.iter().map(|v| value_as_i64(v)).collect();
            Ok(Column::new(name.into(), xs))
        }
        DataType::Float64 => {
            let xs: Vec<Option<f64>> = values.iter().map(|v| value_as_f64(v)).collect();
            Ok(Column::new(name.into(), xs))
        }
        DataType::Boolean => {
            let xs: Vec<Option<bool>> = values.iter().map(|v| value_as_bool(v)).collect();
            Ok(Column::new(name.into(), xs))
        }
        DataType::Date => {
            let xs: Vec<Option<i32>> = values
                .iter()
                .map(|v| {
                    value_as_date(v).map(|d| {
                        (d - NaiveDate::from_ymd_opt(1970, 1, 1).unwrap()).num_days() as i32
                    })
                })
                .collect();
            let s = Series::new(name.into(), xs);
            let s = s.cast(&DataType::Date)?;
            Ok(s.into_column())
        }
        DataType::Datetime(_, _) => {
            let xs: Vec<Option<i64>> = values
                .iter()
                .map(|v| value_as_datetime(v).map(naive_to_micros))
                .collect();
            let s = Series::new(name.into(), xs);
            let s = s.cast(&DataType::Datetime(TimeUnit::Microseconds, None))?;
            Ok(s.into_column())
        }
        DataType::List(inner) if **inner == DataType::String => {
            let mut items_per_row: Vec<Option<Series>> = Vec::with_capacity(values.len());
            for v in values {
                let items: Option<Vec<String>> = match v {
                    serde_yaml::Value::Sequence(seq) => {
                        Some(seq.iter().map(yaml_value_to_string_cell).collect())
                    }
                    // Missing/null frontmatter stays null at the list level
                    // (instead of becoming an empty list) so callers can
                    // distinguish "field absent" from "empty array". Length
                    // on null then null-propagates through arithmetic.
                    serde_yaml::Value::Null => None,
                    other => Some(vec![yaml_value_to_string_cell(other)]),
                };
                items_per_row.push(items.map(|xs| Series::new("".into(), &xs)));
            }
            let chunked: ListChunked = ListChunked::from_iter(items_per_row.into_iter());
            let series = chunked.into_series().with_name(name.into());
            Ok(series.into_column())
        }
        DataType::String => {
            let xs: Vec<Option<String>> = values.iter().map(|v| value_as_string_cell(v)).collect();
            Ok(Column::new(name.into(), xs))
        }
        other => Err(CrabaseError::Query(format!(
            "Unsupported inferred dtype: {other:?}"
        ))),
    }
}

fn value_as_i64(v: &serde_yaml::Value) -> Option<i64> {
    match v {
        serde_yaml::Value::Null => None,
        serde_yaml::Value::Number(n) => n.as_i64(),
        serde_yaml::Value::Bool(b) => Some(*b as i64),
        serde_yaml::Value::String(s) => s.parse::<i64>().ok(),
        _ => None,
    }
}

fn value_as_f64(v: &serde_yaml::Value) -> Option<f64> {
    match v {
        serde_yaml::Value::Null => None,
        serde_yaml::Value::Number(n) => n.as_f64(),
        serde_yaml::Value::Bool(b) => Some(if *b { 1.0 } else { 0.0 }),
        serde_yaml::Value::String(s) => s.parse::<f64>().ok(),
        _ => None,
    }
}

fn value_as_bool(v: &serde_yaml::Value) -> Option<bool> {
    match v {
        serde_yaml::Value::Bool(b) => Some(*b),
        _ => None,
    }
}

fn value_as_date(v: &serde_yaml::Value) -> Option<NaiveDate> {
    match v {
        serde_yaml::Value::String(s) => {
            let stripped = strip_wikilink(s);
            if let Ok(d) = NaiveDate::parse_from_str(stripped, "%Y-%m-%d") {
                Some(d)
            } else {
                NaiveDateTime::parse_from_str(stripped, "%Y-%m-%d %H:%M:%S")
                    .or_else(|_| NaiveDateTime::parse_from_str(stripped, "%Y-%m-%dT%H:%M:%S"))
                    .ok()
                    .map(|dt| dt.date())
            }
        }
        _ => None,
    }
}

fn value_as_datetime(v: &serde_yaml::Value) -> Option<NaiveDateTime> {
    match v {
        serde_yaml::Value::String(s) => {
            let stripped = strip_wikilink(s);
            NaiveDateTime::parse_from_str(stripped, "%Y-%m-%d %H:%M:%S")
                .or_else(|_| NaiveDateTime::parse_from_str(stripped, "%Y-%m-%dT%H:%M:%S"))
                .ok()
                .or_else(|| {
                    NaiveDate::parse_from_str(stripped, "%Y-%m-%d")
                        .ok()
                        .and_then(|d| d.and_hms_opt(0, 0, 0))
                })
        }
        _ => None,
    }
}

fn yaml_value_to_string_cell(v: &serde_yaml::Value) -> String {
    match v {
        // A null *inside* a sequence stringifies as the literal "null" to match
        // Obsidian's CSV output (`scope: [null]` shows up as the string "null",
        // not an empty cell). Top-level nulls are routed through
        // `value_as_string_cell` which keeps producing `None`.
        serde_yaml::Value::Null => "null".to_string(),
        serde_yaml::Value::Bool(b) => b.to_string(),
        serde_yaml::Value::Number(n) => n.to_string(),
        serde_yaml::Value::String(s) => s.clone(),
        serde_yaml::Value::Sequence(seq) => seq
            .iter()
            .map(yaml_value_to_string_cell)
            .collect::<Vec<_>>()
            .join(", "),
        serde_yaml::Value::Mapping(_) => String::new(),
        serde_yaml::Value::Tagged(t) => yaml_value_to_string_cell(&t.value),
    }
}

fn value_as_string_cell(v: &serde_yaml::Value) -> Option<String> {
    match v {
        serde_yaml::Value::Null => None,
        other => Some(yaml_value_to_string_cell(other)),
    }
}
