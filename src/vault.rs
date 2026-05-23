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
];

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
pub fn scan_vault_to_lazyframe(vault_root: &Path) -> Result<(LazyFrame, VaultSchema)> {
    let raw_files = collect_raw_files(vault_root)?;
    let (df, schema) = build_dataframe(raw_files)?;
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
    links: Vec<String>,
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
    let links = extract_wikilinks(&content);

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
    body.lines().flat_map(extract_inline_tags_from_line).collect()
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

fn extract_wikilinks(content: &str) -> Vec<String> {
    let mut links = Vec::new();
    let mut i = 0;
    let bytes = content.as_bytes();
    while i + 1 < bytes.len() {
        if bytes[i] == b'[' && bytes[i + 1] == b'[' {
            let start = i + 2;
            let end = content[start..].find("]]").map(|pos| start + pos);
            if let Some(end) = end {
                let inner = &content[start..end];
                let link_target = if let Some(pipe_pos) = inner.find('|') {
                    inner[..pipe_pos].trim()
                } else {
                    inner.trim()
                };
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

fn build_dataframe(raw_files: Vec<RawFile>) -> Result<(DataFrame, VaultSchema)> {
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
            .map(|f| {
                f.frontmatter
                    .get(key)
                    .unwrap_or(&serde_yaml::Value::Null)
            })
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
    };
    Ok((df, schema))
}

fn naive_to_micros(dt: NaiveDateTime) -> i64 {
    dt.and_utc().timestamp_micros()
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
    if only(has_int, has_float, has_bool, has_date, has_datetime, has_list || has_string) {
        DataType::Int64
    } else if !has_bool
        && !has_date
        && !has_datetime
        && !has_list
        && !has_string
        && (has_int || has_float)
    {
        DataType::Float64
    } else if only(has_bool, has_int, has_float, has_date, has_datetime, has_list || has_string) {
        DataType::Boolean
    } else if !has_int && !has_float && !has_bool && !has_list && !has_string && has_datetime {
        DataType::Datetime(TimeUnit::Microseconds, None)
    } else if !has_int && !has_float && !has_bool && !has_list && !has_string && (has_date || has_datetime) {
        // Mix of date + datetime → promote to Datetime for uniform storage
        if has_datetime {
            DataType::Datetime(TimeUnit::Microseconds, None)
        } else {
            DataType::Date
        }
    } else if only(has_list, has_int, has_float, has_bool, has_date, has_datetime || has_string) {
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
                .map(|v| value_as_date(v).map(|d| (d - NaiveDate::from_ymd_opt(1970, 1, 1).unwrap()).num_days() as i32))
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
            let mut series_vec: Vec<Series> = Vec::with_capacity(values.len());
            for v in values {
                let items: Vec<String> = match v {
                    serde_yaml::Value::Sequence(seq) => seq
                        .iter()
                        .map(yaml_value_to_string_cell)
                        .collect(),
                    serde_yaml::Value::Null => Vec::new(),
                    other => vec![yaml_value_to_string_cell(other)],
                };
                series_vec.push(Series::new("".into(), &items));
            }
            list_string_column(name, series_vec)
        }
        DataType::String => {
            let xs: Vec<Option<String>> =
                values.iter().map(|v| value_as_string_cell(v)).collect();
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
        serde_yaml::Value::Null => String::new(),
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

