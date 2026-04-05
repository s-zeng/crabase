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
        let mut props = HashMap::new();
        props.insert("name".to_string(), Value::Str(self.name.clone()));
        props.insert("path".to_string(), Value::Str(self.rel_path.clone()));
        props.insert("folder".to_string(), Value::Str(self.folder.clone()));
        props.insert("ext".to_string(), Value::Str(self.ext.clone()));
        props.insert("size".to_string(), Value::Number(self.size as f64));
        props.insert(
            "tags".to_string(),
            Value::List(
                self.tags
                    .iter()
                    .map(|t| Value::Str(t.clone()))
                    .collect(),
            ),
        );
        props.insert(
            "links".to_string(),
            Value::List(
                self.links
                    .iter()
                    .map(|l| Value::Str(l.clone()))
                    .collect(),
            ),
        );
        props
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
        serde_yaml::Value::Sequence(seq) => {
            Value::List(seq.iter().map(yaml_to_value).collect())
        }
        serde_yaml::Value::Mapping(_) => Value::Null,
        serde_yaml::Value::Tagged(tagged) => yaml_to_value(&tagged.value),
    }
}

/// Scan a vault directory and return all .md files
pub fn scan_vault(vault_root: &Path) -> Result<Vec<VaultFile>> {
    let mut files = Vec::new();

    for entry in WalkDir::new(vault_root)
        .follow_links(false)
        .into_iter()
        .filter_map(|e| e.ok())
        .filter(|e| e.file_type().is_file())
    {
        let abs_path = entry.path().to_path_buf();
        let ext = abs_path
            .extension()
            .and_then(|e| e.to_str())
            .unwrap_or("")
            .to_string();

        if ext != "md" {
            continue;
        }

        let rel_path = abs_path
            .strip_prefix(vault_root)
            .map_err(|e| CrabaseError::Io(std::io::Error::new(std::io::ErrorKind::Other, e.to_string())))?
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

        let metadata = std::fs::metadata(&abs_path)?;
        let size = metadata.len();

        let content = std::fs::read_to_string(&abs_path)?;
        let (frontmatter, body) = parse_frontmatter(&content);
        let mut tags = extract_frontmatter_tags(&frontmatter);
        let inline_tags = extract_inline_tags(&body);
        for t in inline_tags {
            if !tags.contains(&t) {
                tags.push(t);
            }
        }
        let links = extract_wikilinks(&content);

        files.push(VaultFile {
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
        });
    }

    Ok(files)
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

    let map: HashMap<String, serde_yaml::Value> = serde_yaml::from_str(yaml_str).unwrap_or_default();
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
    let mut tags = Vec::new();
    for line in body.lines() {
        // Simple inline tag extraction: find #word patterns
        let mut chars = line.char_indices().peekable();
        while let Some((i, c)) = chars.next() {
            if c == '#' {
                // Make sure it's not inside a wikilink or code
                let tag: String = chars
                    .by_ref()
                    .take_while(|(_, c)| c.is_alphanumeric() || *c == '/' || *c == '_' || *c == '-')
                    .map(|(_, c)| c)
                    .collect();
                if !tag.is_empty() && tag.chars().next().map_or(false, |c| c.is_alphabetic()) {
                    // Only add if preceded by whitespace or start of line
                    let preceded_by_space = i == 0 || line[..i].ends_with(char::is_whitespace);
                    if preceded_by_space {
                        tags.push(tag);
                    }
                }
            }
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
