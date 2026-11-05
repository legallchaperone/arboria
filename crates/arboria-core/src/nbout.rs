//! Each command block's last result (ADR 0005), kept out of the notebook file:
//! `<state dir>/outputs/<notebook>/<block>.json`. Written by whoever ran the block (the
//! app, `arb nb run`); read when the notebook opens. Not the ledger: losing it costs a
//! re-run only.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::nbdoc::valid_id;
use crate::Result;

/// One block's last run.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Cached {
    /// sha256 of the code that ran.
    pub code_sha: String,
    pub ran_at: String,
    pub duration_ms: u64,
    pub ok: bool,
    /// The structured outputs, in order (`arboria.output`).
    pub outputs: Vec<Value>,
    #[serde(default)]
    pub defines: Vec<String>,
    #[serde(default)]
    pub uses: Vec<String>,
}

/// The cache folder of one notebook: `outputs/<idea>` or `outputs/seed-<seed>`.
pub fn dir(state_dir: &Path, notebook: &str) -> PathBuf {
    state_dir.join("outputs").join(notebook)
}

/// sha256 of a block's code.
pub fn code_sha(code: &str) -> String {
    crate::cards::sha256_hex(code.as_bytes())
}

/// Store a block's result (temp file + rename).
pub fn store(dir: &Path, block: &str, c: &Cached) -> Result<()> {
    if !valid_id(block) {
        crate::bail!("not a block id: {block:?}");
    }
    std::fs::create_dir_all(dir)?;
    let path = dir.join(format!("{block}.json"));
    let tmp = dir.join(format!(".{block}.json.tmp"));
    std::fs::write(&tmp, serde_json::to_vec(c)?)?;
    std::fs::rename(tmp, path)?;
    Ok(())
}

/// All stored results of a notebook (unreadable files are skipped).
pub fn load(dir: &Path) -> BTreeMap<String, Cached> {
    let mut out = BTreeMap::new();
    let Ok(entries) = std::fs::read_dir(dir) else {
        return out;
    };
    for e in entries.flatten() {
        let name = e.file_name().to_string_lossy().into_owned();
        let Some(id) = name.strip_suffix(".json") else {
            continue;
        };
        if !valid_id(id) {
            continue;
        }
        if let Some(c) = std::fs::read(e.path())
            .ok()
            .and_then(|b| serde_json::from_slice::<Cached>(&b).ok())
        {
            out.insert(id.to_string(), c);
        }
    }
    out
}

/// Forget the results of blocks no longer in the notebook.
pub fn prune(dir: &Path, keep: &[&str]) -> Result<usize> {
    let mut n = 0;
    for id in load(dir).keys() {
        if !keep.contains(&id.as_str()) {
            std::fs::remove_file(dir.join(format!("{id}.json")))?;
            n += 1;
        }
    }
    Ok(n)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn store_load_prune() {
        let d = std::env::temp_dir().join(format!("nbout-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        let c = Cached {
            code_sha: code_sha("1 + 1"),
            ran_at: "2026-11-05T00:00:00Z".into(),
            duration_ms: 3,
            ok: true,
            outputs: vec![serde_json::json!({"kind": "text", "text": "2"})],
            defines: vec![],
            uses: vec![],
        };
        store(&d, "ab12", &c).unwrap();
        store(&d, "cd34", &c).unwrap();
        assert!(store(&d, "../x", &c).is_err());
        std::fs::write(d.join("junk.json"), "{").unwrap();
        assert_eq!(load(&d).len(), 2);
        assert_eq!(load(&d)["ab12"], c);
        assert_eq!(prune(&d, &["ab12"]).unwrap(), 1);
        assert_eq!(load(&d).keys().collect::<Vec<_>>(), ["ab12"]);
        let _ = std::fs::remove_dir_all(&d);
    }
}
