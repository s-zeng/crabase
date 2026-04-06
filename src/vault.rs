use std::collections::HashMap;
use std::path::{Path, PathBuf};
use walkdir::WalkDir;

use crate::error::{CrabaseError, Result};
use crate::expr::eval::Value;

/// Metadata about a single file in the vault
#[derive(Debug, Clone)]
pub struct VaultFile {
    /// Full path on disk
    pub abs_path: PathBuf,
    /// Relative path from vault root (e.g., "Church/Sermons/2025-04-27 foo.md")
    pub rel_path: String,
    /// File name with extension (e.g., "2025-04-27 foo.md")
    pub name: String,
    /// File stem (no extension)
    pub stem: String,
    /// File extension (e.g., "md")
    pub ext: String,
    /// Parent folder relative to vault root
    pub folder: String,
    /// File size in bytes
    pub size: u64,
    /// Frontmatter properties
    pub frontmatter: HashMap<String, serde_yaml::Value>,
    /// Tags (from frontmatter + inline)
    pub tags: Vec<String>,
    /// Wikilinks found in the file
    pub links: Vec<String>,
}

impl VaultFile {
    /// Build the file_props HashMap for eval context
    pub fn file_props(&self) -> HashMap<String, Value> {
        [
            ("name", Value::Str(self.name.clone())),
            ("path", Value::Str(self.rel_path.clone())),
            ("folder", Value::Str(self.folder.clone())),
            ("ext", Value::Str(self.ext.clone())),
            ("size", Value::Number(self.size as f64)),
            (
                "tags",
                Value::List(self.tags.iter().cloned().map(Value::Str).collect()),
            ),
            (
                "links",
                Value::List(self.links.iter().cloned().map(Value::Str).collect()),
            ),
        ]
        .into_iter()
        .map(|(key, value)| (key.to_string(), value))
        .collect()
    }

    /// Build the note_props HashMap for eval context
    pub fn note_props(&self) -> HashMap<String, Value> {
        self.frontmatter
            .iter()
            .map(|(k, v)| (k.clone(), yaml_to_value(v)))
            .collect()
    }
}

fn yaml_to_value(v: &serde_yaml::Value) -> Value {
    match v {
        serde_yaml::Value::Null => Value::Null,
        serde_yaml::Value::Bool(b) => Value::Bool(*b),
        serde_yaml::Value::Number(n) => {
            if let Some(f) = n.as_f64() {
                Value::Number(f)
            } else {
                Value::Null
            }
        }
        serde_yaml::Value::String(s) => Value::Str(s.clone()),
        serde_yaml::Value::Sequence(seq) => Value::List(seq.iter().map(yaml_to_value).collect()),
        serde_yaml::Value::Mapping(_) => Value::Null,
        serde_yaml::Value::Tagged(tagged) => yaml_to_value(&tagged.value),
    }
}

/// Scan a vault directory and return all .md files
pub fn scan_vault(vault_root: &Path) -> Result<Vec<VaultFile>> {
    WalkDir::new(vault_root)
        .follow_links(false)
        .into_iter()
        .filter_map(|e| e.ok())
        .filter(|e| e.file_type().is_file())
        .filter(|entry| entry.path().extension().and_then(|ext| ext.to_str()) == Some("md"))
        .map(|entry| read_vault_file(vault_root, entry.path()))
        .collect()
}

fn read_vault_file(vault_root: &Path, abs_path: &Path) -> Result<VaultFile> {
    let abs_path = abs_path.to_path_buf();
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
    let name = abs_path
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("")
        .to_string();
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
    let size = std::fs::metadata(&abs_path)?.len();
    let content = std::fs::read_to_string(&abs_path)?;
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

    Ok(VaultFile {
        abs_path,
        rel_path,
        name,
        stem,
        ext,
        folder,
        size,
        frontmatter,
        tags,
        links,
    })
}

/// Parse YAML frontmatter from markdown content.
/// Returns (frontmatter_map, rest_of_content)
fn parse_frontmatter(content: &str) -> (HashMap<String, serde_yaml::Value>, String) {
    // Frontmatter must start at the very beginning with "---\n"
    if !content.starts_with("---\n") && !content.starts_with("---\r\n") {
        return (HashMap::new(), content.to_string());
    }

    // Find the closing ---
    let after_open = if content.starts_with("---\r\n") {
        &content[5..]
    } else {
        &content[4..]
    };

    // Find next "---" on its own line
    let close_pos = after_open
        .lines()
        .scan(0usize, |pos, line| {
            let cur = *pos;
            *pos += line.len() + 1; // +1 for newline
            Some((cur, line))
        })
        .find(|(_, line)| *line == "---" || *line == "---\r")
        .map(|(pos, _)| pos);

    let Some(close_pos) = close_pos else {
        return (HashMap::new(), content.to_string());
    };

    let yaml_str = &after_open[..close_pos];
    let rest_start = close_pos + 4; // skip "---\n"
    let body = if rest_start <= after_open.len() {
        after_open[rest_start..].to_string()
    } else {
        String::new()
    };

    let map: HashMap<String, serde_yaml::Value> =
        serde_yaml::from_str(yaml_str).unwrap_or_default();
    (map, body)
}

/// Extract tags from frontmatter `tags` field (list or string)
fn extract_frontmatter_tags(frontmatter: &HashMap<String, serde_yaml::Value>) -> Vec<String> {
    let Some(tags_val) = frontmatter.get("tags") else {
        return Vec::new();
    };
    match tags_val {
        serde_yaml::Value::Sequence(seq) => seq
            .iter()
            .filter_map(|v| v.as_str().map(|s| s.trim_start_matches('#').to_string()))
            .collect(),
        serde_yaml::Value::String(s) => {
            vec![s.trim_start_matches('#').to_string()]
        }
        _ => Vec::new(),
    }
}

/// Extract inline tags from body content (lines containing #tag patterns)
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

/// Extract wikilinks [[...]] from content
fn extract_wikilinks(content: &str) -> Vec<String> {
    let mut links = Vec::new();
    let mut i = 0;
    let bytes = content.as_bytes();
    while i + 1 < bytes.len() {
        if bytes[i] == b'[' && bytes[i + 1] == b'[' {
            // Find closing ]]
            let start = i + 2;
            let end = content[start..].find("]]").map(|pos| start + pos);
            if let Some(end) = end {
                let inner = &content[start..end];
                // Strip display text (after |)
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
