use std::path::PathBuf;

use crabase_lib::base_file::BaseFile;
use crabase_lib::error::{CrabaseError, Result};
use crabase_lib::output::write_csv;
use crabase_lib::query::execute_query;
use crabase_lib::vault::scan_bases;

/// Parse `key=value` style arguments from a list of strings.
fn parse_kv_args(args: &[String]) -> std::collections::HashMap<String, String> {
    let mut map = std::collections::HashMap::new();
    for arg in args {
        if let Some(eq_pos) = arg.find('=') {
            let key = arg[..eq_pos].to_string();
            let val = arg[eq_pos + 1..].to_string();
            map.insert(key, val);
        }
    }
    map
}

/// Entry point: parse CLI arguments and run query
fn run() -> Result<()> {
    let raw_args: Vec<String> = std::env::args().collect();

    // Expect: crabase <subcommand> <key=value>...
    if raw_args.len() < 2 {
        eprintln!(
            "Usage: crabase <subcommand> [args]\n  base:query file=<path> format=csv [vault=<vault_root>] [view=<view_name>]\n  base:views file=<path> [vault=<vault_root>]\n  bases [vault=<vault_root>]"
        );
        return Err(CrabaseError::MissingArg("subcommand".to_string()));
    }

    let subcommand = &raw_args[1];

    if subcommand == "base:views" {
        let kv_args = parse_kv_args(&raw_args[2..]);
        let file_arg = kv_args
            .get("file")
            .ok_or_else(|| CrabaseError::MissingArg("file".to_string()))?;
        let vault_root = kv_args
            .get("vault")
            .map(PathBuf::from)
            .unwrap_or_else(|| std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")));
        let base_file_path = vault_root.join(file_arg);
        let base_content = std::fs::read_to_string(&base_file_path).map_err(|e| {
            CrabaseError::Io(std::io::Error::new(
                e.kind(),
                format!(
                    "Cannot read base file '{}': {}",
                    base_file_path.display(),
                    e
                ),
            ))
        })?;
        let base_file = BaseFile::parse(&base_content)?;
        for view in &base_file.views {
            match &view.name {
                Some(name) => println!("{name}"),
                None => println!("(unnamed)"),
            }
        }
        return Ok(());
    }

    if subcommand == "bases" {
        let kv_args = parse_kv_args(&raw_args[2..]);
        let vault_root = kv_args
            .get("vault")
            .map(PathBuf::from)
            .unwrap_or_else(|| std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")));
        let paths = scan_bases(&vault_root)?;
        for path in paths {
            println!("{path}");
        }
        return Ok(());
    }

    if subcommand != "base:query" {
        return Err(CrabaseError::Query(format!(
            "Unknown subcommand: {subcommand}. Expected 'base:query', 'base:views', or 'bases'"
        )));
    }

    let kv_args = parse_kv_args(&raw_args[2..]);

    let file_arg = kv_args
        .get("file")
        .ok_or_else(|| CrabaseError::MissingArg("file".to_string()))?;

    let format = kv_args.get("format").map(String::as_str).unwrap_or("csv");
    if format != "csv" {
        return Err(CrabaseError::Query(format!(
            "Unsupported format: {format}. Only 'csv' is supported"
        )));
    }

    let vault_root = kv_args
        .get("vault")
        .map(PathBuf::from)
        .unwrap_or_else(|| std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")));

    let view_name = kv_args.get("view").map(String::as_str);

    // Resolve base file path relative to vault root
    let base_file_path = vault_root.join(file_arg);
    let base_content = std::fs::read_to_string(&base_file_path).map_err(|e| {
        CrabaseError::Io(std::io::Error::new(
            e.kind(),
            format!(
                "Cannot read base file '{}': {}",
                base_file_path.display(),
                e
            ),
        ))
    })?;

    let base_file = BaseFile::parse(&base_content)?;
    let view = base_file.get_view(view_name)?;

    // Get the column order for output
    let columns = view.order.clone().unwrap_or_default();

    let rows = execute_query(&vault_root, &base_file, view)?;

    let stdout = std::io::stdout();
    let mut out = stdout.lock();
    write_csv(&mut out, &columns, &rows, &base_file).map_err(CrabaseError::Io)?;

    Ok(())
}

fn main() {
    if let Err(e) = run() {
        eprintln!("Error: {e}");
        std::process::exit(1);
    }
}
