// Converter for Gingko Desktop 2.x .gko files: 7z archives containing a
// PouchDB-on-leveldb database of git-like objects (commit / tree / ref).
// Rebuilds the card tree from the head commit and emits the current
// <gingko-card> markdown format (mirroring Coders.treeToMarkdownOutline).

use std::collections::HashMap;
use std::fs;
use std::io::Read;
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

use rusty_leveldb::LdbIterator;
use serde_json::Value;

const SEVENZ_MAGIC: [u8; 6] = *b"7z\xbc\xaf\x27\x1c";

pub fn is_legacy_gko(path: &Path) -> bool {
    let mut magic = [0; 6];
    fs::File::open(path)
        .and_then(|mut f| f.read_exact(&mut magic))
        .is_ok()
        && magic == SEVENZ_MAGIC
}

pub fn convert_gko_to_markdown(gko_path: &Path) -> Result<String, String> {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let tmp = std::env::temp_dir().join(format!("gko-import-{}", nanos));
    fs::create_dir_all(&tmp).map_err(|e| e.to_string())?;

    let result = convert_inner(gko_path, &tmp);
    let _ = fs::remove_dir_all(&tmp);
    result
}

fn convert_inner(gko_path: &Path, tmp: &Path) -> Result<String, String> {
    sevenz_rust2::decompress_file(gko_path, tmp)
        .map_err(|e| format!("Could not extract the archive: {}", e))?;

    let db_dir = tmp.join("leveldb");
    if !db_dir.is_dir() {
        return Err("The archive does not contain a leveldb database.".into());
    }

    // leveldb holds a LOCK file from the last writer; it is stale by definition.
    let _ = fs::remove_file(db_dir.join("LOCK"));

    let mut opts = rusty_leveldb::Options::default();
    opts.create_if_missing = false;
    opts.paranoid_checks = false;
    let mut db = rusty_leveldb::DB::open(&db_dir, opts)
        .map_err(|e| format!("Could not read the database: {}", e))?;

    let mut trees: HashMap<String, Value> = HashMap::new();
    let mut commits: Vec<Value> = Vec::new();
    let mut head_ref: Option<String> = None;

    let mut iter = db.new_iter().map_err(|e| e.to_string())?;
    while iter.advance() {
        let Some((_key, val)) = iter.current() else {
            continue;
        };
        let Ok(doc) = serde_json::from_slice::<Value>(&val) else {
            continue;
        };
        match doc["type"].as_str() {
            Some("tree") => {
                if let Some(id) = doc["_id"].as_str().map(str::to_string) {
                    trees.insert(id, doc);
                }
            }
            Some("commit") => commits.push(doc),
            // by-sequence keys iterate in write order; the last ref wins.
            Some("ref") if doc["_id"] == "heads/master" => {
                if let Some(v) = doc["value"].as_str() {
                    head_ref = Some(v.to_string());
                }
            }
            _ => {}
        }
    }

    if commits.is_empty() || trees.is_empty() {
        return Err("No document data found in the archive.".into());
    }

    // Head commit: the heads/master ref if present, else the newest commit.
    let head_commit = head_ref
        .and_then(|sha| commits.iter().find(|c| c["_id"] == sha.as_str()))
        .or_else(|| {
            commits
                .iter()
                .max_by_key(|c| c["timestamp"].as_i64().unwrap_or(0))
        })
        .ok_or_else(|| "No commit found in the archive.".to_string())?;

    let root_sha = head_commit["tree"]
        .as_str()
        .ok_or_else(|| "Head commit has no tree.".to_string())?;

    let root = trees
        .get(root_sha)
        .ok_or_else(|| "Root tree object not found.".to_string())?;

    // Mirrors Coders.treeToMarkdownOutline False: children of the root only.
    Ok(tree_children(root)
        .iter()
        .map(|(sha, id)| card_to_markdown(&trees, sha, id))
        .collect::<Vec<_>>()
        .join("\n"))
}

fn tree_children(tree: &Value) -> Vec<(String, String)> {
    tree["children"]
        .as_array()
        .map(|arr| {
            arr.iter()
                .filter_map(|pair| {
                    Some((pair[0].as_str()?.to_string(), pair[1].as_str()?.to_string()))
                })
                .collect()
        })
        .unwrap_or_default()
}

// Mirrors Coders.treeToMarkdownOutlineRecurse.
fn card_to_markdown(trees: &HashMap<String, Value>, sha: &str, card_id: &str) -> String {
    let (content, children) = match trees.get(sha) {
        Some(t) => (t["content"].as_str().unwrap_or(""), tree_children(t)),
        None => ("", Vec::new()),
    };
    let mut parts: Vec<String> = children
        .iter()
        .map(|(csha, cid)| card_to_markdown(trees, csha, cid))
        .collect();
    parts.push(String::new());
    format!(
        "<gingko-card id=\"{}\">\n\n{}\n\n{}</gingko-card>",
        card_id,
        content,
        parts.join("\n")
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn converts_sample_gko() {
        let sample = Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .unwrap()
            .join("tests/historic/sample_3columns.gko");
        assert!(is_legacy_gko(&sample));
        let md = convert_gko_to_markdown(&sample).expect("conversion should succeed");
        assert!(
            md.contains("<gingko-card id=\""),
            "should contain card tags"
        );
        assert!(
            md.contains("Nested Card") && md.contains("last nested"),
            "should include cards from all three columns"
        );
    }
}
