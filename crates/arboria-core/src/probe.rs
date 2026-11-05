//! Probes (ADR 0003): small pure parsers the agent writes for a machine's query commands,
//! run on a schedule by the LaunchAgent helper. New in the Rust core; no Python module
//! ports to it (it shares the file conventions of `arboria.commands`: one `.py` file per
//! probe, metadata read without executing anything, promote to global, todos for the
//! agent).
//!
//! A probe is `.arboria/probes/<name>.py` (project) or `~/.config/arboria/probes/<name>.py`
//! (global) with a PEP 723-style comment block:
//!
//! ```python
//! # /// arb-probe
//! # api = 1
//! # machine = "deltaai"
//! # argv = ["accounts"]
//! # every = "30m"
//! # kind = "number"
//! # ///
//! def deltaai_credits(text: str) -> dict: ...
//! ```
//!
//! Invariants:
//! - **The core never acts on a probe's output**: it only shows it to the human (escaped)
//!   or turns it into a todo. A probe cannot ask the core for anything; it has no
//!   `ctx.remote()`, no network permission.
//! - The core runs `argv` itself (fixed in the header, `ssh -O check` first, NeedLogin
//!   skipped, never retried) and hands stdout to a short-lived `uv run --project <project>
//!   python -I arb_probe_host.py` (JSON in / JSON out, timeout kills the process group,
//!   input and output truncated).
//! - Enabling is a human Decision bound to the file's sha256 (`decisions.subject_type =
//!   'probe'`, `bound_hash`): only in the human's terminal ([`crate::human`]) or the app's
//!   Touch ID panel. The helper runs a file only when its current hash equals
//!   [`enabled_hash`]; one changed byte disables it ("待重新启用" in `arb status`).
//!   install-claude denies `arb probe enable`.
//! - [`set_probe_result`] is the only function that writes `probe_results` (a source scan
//!   in `tests/guards.rs` checks it). It validates again, writes no event and triggers no
//!   backup commit (heartbeat-like).
//! - Results are validated ([`validate_result`]): the header's kind, known keys only,
//!   length limits, finite numbers, no HTML, no control characters. Display escapes anyway.
//! - An `api` this build does not know disables the probe with a message; it is never run.
//!
//! The host is `python -I` ([`crate::commands::python_exe`]) for now, not `uv run
//! --project`: phase 1 allows the project environment's standard library only, and the
//! interpreter is the one arb already uses for the Python side.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use crate::db::Ledger;
use crate::{Error, Result};

/// The header block's type: `# /// arb-probe`.
pub const HEADER_TYPE: &str = "arb-probe";
/// The probe API versions this build can run.
pub const SUPPORTED_API: &[u32] = &[1];
/// Shortest schedule interval.
pub const MIN_EVERY_S: u64 = 60;
/// Longest schedule interval (7 days).
pub const MAX_EVERY_S: u64 = 7 * 86_400;
/// `probes/` under the state dir (project) or the config dir (global).
pub const PROBES_DIR: &str = "probes";
/// Stdout handed to a probe is truncated to this many bytes.
pub const MAX_INPUT_BYTES: usize = 256 * 1024;
/// A probe's JSON answer longer than this is refused unparsed.
pub const MAX_OUTPUT_BYTES: usize = 64 * 1024;
/// `argv` has at most this many words, each at most [`MAX_ARG_CHARS`].
pub const MAX_ARGV: usize = 32;
/// See [`MAX_ARGV`].
pub const MAX_ARG_CHARS: usize = 200;

/// Result limits (code points).
pub const MAX_LABEL: usize = 40;
/// `unit` of a number.
pub const MAX_UNIT: usize = 16;
/// `text` of a text result.
pub const MAX_TEXT: usize = 2000;
/// Table columns.
pub const MAX_COLUMNS: usize = 8;
/// Table rows.
pub const MAX_ROWS: usize = 50;
/// One table cell.
pub const MAX_CELL: usize = 200;

// ---------------------------------------------------------------- names and keys

/// Where a probe file lives.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ProbeScope {
    /// `.arboria/probes/` of this project.
    Project,
    /// `~/.config/arboria/probes/`.
    Global,
    /// Shipped with the core (`balance`); needs no enabling.
    Builtin,
}

impl ProbeScope {
    /// `project` / `global` / `builtin`.
    pub fn as_str(self) -> &'static str {
        match self {
            ProbeScope::Project => "project",
            ProbeScope::Global => "global",
            ProbeScope::Builtin => "builtin",
        }
    }
}

/// Probe names are command names: `^[a-z][a-z0-9_]{0,40}$` (the file stem and the
/// function name, Python `commands.NAME_RE`).
pub fn validate_name(name: &str) -> Result<()> {
    let b = name.as_bytes();
    let ok = !b.is_empty()
        && b.len() <= 41
        && b[0].is_ascii_lowercase()
        && b[1..]
            .iter()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || *c == b'_');
    if ok {
        Ok(())
    } else {
        Err(Error::Arb(format!(
            "invalid probe name {}: use lowercase letters, digits and '_' (must start with a letter, max 41 chars)",
            crate::records::py_repr(name)
        )))
    }
}

/// `<scope>:<name>`: `decisions.subject_id` and `probe_results.probe`.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ProbeKey {
    pub scope: ProbeScope,
    pub name: String,
}

impl ProbeKey {
    /// A validated key.
    pub fn new(scope: ProbeScope, name: &str) -> Result<Self> {
        validate_name(name)?;
        Ok(ProbeKey {
            scope,
            name: name.to_string(),
        })
    }

    /// Parse `project:x` / `global:x` / `builtin:x`.
    pub fn parse(s: &str) -> Result<Self> {
        let (scope, name) = s.split_once(':').ok_or_else(|| {
            Error::Arb(format!("invalid probe key {}", crate::records::py_repr(s)))
        })?;
        let scope = match scope {
            "project" => ProbeScope::Project,
            "global" => ProbeScope::Global,
            "builtin" => ProbeScope::Builtin,
            _ => {
                return Err(Error::Arb(format!(
                    "invalid probe scope {}",
                    crate::records::py_repr(scope)
                )))
            }
        };
        ProbeKey::new(scope, name)
    }
}

impl std::fmt::Display for ProbeKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}:{}", self.scope.as_str(), self.name)
    }
}

// ---------------------------------------------------------------- header

/// What a probe returns.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ProbeKind {
    /// `{"label", "value", "of"?, "unit"?}`.
    Number,
    /// `{"label"?, "columns", "rows"}`.
    Table,
    /// `{"label"?, "text"}`.
    Text,
}

impl ProbeKind {
    /// `number` / `table` / `text`.
    pub fn as_str(self) -> &'static str {
        match self {
            ProbeKind::Number => "number",
            ProbeKind::Table => "table",
            ProbeKind::Text => "text",
        }
    }
}

/// The validated `# /// arb-probe` block.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProbeHeader {
    /// Probe API version ([`SUPPORTED_API`]).
    pub api: u32,
    /// Machine profile name (`clusters.toml` table).
    pub machine: String,
    /// The command the core runs on the machine (fixed; joined with `shlex.join`).
    pub argv: Vec<String>,
    /// The schedule as written (`30m`).
    pub every: String,
    /// [`ProbeHeader::every`] in seconds.
    pub every_s: u64,
    pub kind: ProbeKind,
}

fn header_err(msg: impl std::fmt::Display) -> Error {
    Error::Arb(format!("probe header (# /// {HEADER_TYPE}): {msg}"))
}

/// The TOML text of the `# /// arb-probe` block of `source`, PEP 723 style: a line
/// `# /// arb-probe`, then lines that are `#` or start with `# `, then `# ///`. None
/// without a block; an error for an unterminated block or a second block.
pub fn header_block(source: &str) -> Result<Option<String>> {
    let opener = format!("# /// {HEADER_TYPE}");
    let mut found: Option<String> = None;
    let mut lines = source.lines();
    while let Some(line) = lines.next() {
        if line.trim_end_matches('\r') != opener {
            continue;
        }
        if found.is_some() {
            return Err(header_err("more than one block"));
        }
        let mut body = String::new();
        let mut closed = false;
        for l in lines.by_ref() {
            let l = l.trim_end_matches('\r');
            if l == "# ///" {
                closed = true;
                break;
            }
            if l == "#" {
                body.push('\n');
            } else if let Some(rest) = l.strip_prefix("# ") {
                body.push_str(rest);
                body.push('\n');
            } else {
                return Err(header_err(format!(
                    "every line of the block must start with '# ' (got {})",
                    crate::records::py_repr(l)
                )));
            }
        }
        if !closed {
            return Err(header_err("not closed with '# ///'"));
        }
        found = Some(body);
    }
    Ok(found)
}

/// Parse and validate the header of a probe file's source. Reads comments only; the
/// file is never executed or imported by the core.
pub fn parse_header(source: &str) -> Result<ProbeHeader> {
    let body = header_block(source)?.ok_or_else(|| header_err("missing"))?;
    let table: toml::Table = body
        .parse()
        .map_err(|e: toml::de::Error| header_err(e.message()))?;
    const KEYS: &[&str] = &["api", "machine", "argv", "every", "kind"];
    if let Some(k) = table.keys().find(|k| !KEYS.contains(&k.as_str())) {
        return Err(header_err(format!(
            "unknown key {}; known: {}",
            crate::records::py_repr(k),
            KEYS.join(", ")
        )));
    }
    let get = |k: &str| {
        table
            .get(k)
            .ok_or_else(|| header_err(format!("{k} is required")))
    };

    let api = get("api")?
        .as_integer()
        .ok_or_else(|| header_err("api must be an integer"))?;
    let api = u32::try_from(api).map_err(|_| header_err(format!("api {api} is not supported")))?;
    if !SUPPORTED_API.contains(&api) {
        return Err(header_err(format!(
            "api {api} is not supported by this arb (supported: {}); the probe stays disabled",
            SUPPORTED_API
                .iter()
                .map(u32::to_string)
                .collect::<Vec<_>>()
                .join(", ")
        )));
    }

    let machine = get("machine")?
        .as_str()
        .ok_or_else(|| header_err("machine must be a string"))?;
    crate::model::IdeaName::parse(machine).map_err(|_| {
        header_err(format!(
            "invalid machine name {}",
            crate::records::py_repr(machine)
        ))
    })?;

    let argv = get("argv")?
        .as_array()
        .ok_or_else(|| header_err("argv must be a list of strings"))?;
    if argv.is_empty() || argv.len() > MAX_ARGV {
        return Err(header_err(format!("argv must have 1 to {MAX_ARGV} words")));
    }
    let argv = argv
        .iter()
        .map(|v| {
            let s = v
                .as_str()
                .ok_or_else(|| header_err("argv must be a list of strings"))?;
            if s.is_empty() || s.chars().count() > MAX_ARG_CHARS || s.chars().any(char::is_control)
            {
                return Err(header_err(format!(
                    "argv word {} must be 1 to {MAX_ARG_CHARS} printable characters",
                    crate::records::py_repr(s)
                )));
            }
            Ok(s.to_string())
        })
        .collect::<Result<Vec<_>>>()?;

    let every = get("every")?
        .as_str()
        .ok_or_else(|| header_err("every must be a string like \"30m\""))?;
    let every_s = parse_every(every)?;

    let kind = match get("kind")?.as_str() {
        Some("number") => ProbeKind::Number,
        Some("table") => ProbeKind::Table,
        Some("text") => ProbeKind::Text,
        _ => return Err(header_err("kind must be one of: number, table, text")),
    };
    Ok(ProbeHeader {
        api,
        machine: machine.to_string(),
        argv,
        every: every.to_string(),
        every_s,
        kind,
    })
}

/// `30m`, `1h`, `1h30m`, `2d`, `90s` → seconds; [`MIN_EVERY_S`]..=[`MAX_EVERY_S`].
pub fn parse_every(text: &str) -> Result<u64> {
    let bad = || {
        header_err(format!(
            "every {}: use e.g. 30m, 1h, 1h30m, 1d",
            crate::records::py_repr(text)
        ))
    };
    let mut total: u64 = 0;
    let mut digits = String::new();
    let mut last_unit = 5; // units must appear in d, h, m, s order, each once
    for c in text.chars() {
        if c.is_ascii_digit() {
            digits.push(c);
            continue;
        }
        let (rank, mult) = match c {
            'd' => (0, 86_400),
            'h' => (1, 3_600),
            'm' => (2, 60),
            's' => (3, 1),
            _ => return Err(bad()),
        };
        if digits.is_empty() || (last_unit != 5 && rank <= last_unit) {
            return Err(bad());
        }
        let n: u64 = digits.parse().map_err(|_| bad())?;
        total = n
            .checked_mul(mult)
            .and_then(|x| total.checked_add(x))
            .ok_or_else(bad)?;
        digits.clear();
        last_unit = rank;
    }
    if !digits.is_empty() || last_unit == 5 {
        return Err(bad());
    }
    if !(MIN_EVERY_S..=MAX_EVERY_S).contains(&total) {
        return Err(header_err(format!(
            "every {} must be between 1m and 7d",
            crate::records::py_repr(text)
        )));
    }
    Ok(total)
}

// ---------------------------------------------------------------- results

/// A number result (`剩余 SU 199 / 200`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct NumberResult {
    pub label: String,
    /// As the probe wrote it (`199` stays an integer, `0.5` a float).
    pub value: serde_json::Number,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub of: Option<serde_json::Number>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub unit: Option<String>,
}

/// A table cell: text, a number, or empty.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum Cell {
    Number(serde_json::Number),
    Text(String),
    Empty,
}

/// A table result.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TableResult {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    pub columns: Vec<String>,
    pub rows: Vec<Vec<Cell>>,
}

/// A text result (also what the built-in `balance` falls back to when parsing fails).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TextResult {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    pub text: String,
}

/// A validated probe answer, tagged by kind in the ledger (`{"kind": "number", …}`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum ProbeResult {
    Number(NumberResult),
    Table(TableResult),
    Text(TextResult),
}

impl ProbeResult {
    /// The result's kind.
    pub fn kind(&self) -> ProbeKind {
        match self {
            ProbeResult::Number(_) => ProbeKind::Number,
            ProbeResult::Table(_) => ProbeKind::Table,
            ProbeResult::Text(_) => ProbeKind::Text,
        }
    }

    /// The JSON stored in `probe_results.result_json` (kind tag included).
    pub fn to_ledger_json(&self) -> Result<String> {
        crate::canonical::to_ledger_json(self)
    }
}

fn result_err(msg: impl std::fmt::Display) -> Error {
    Error::Arb(format!("probe result: {msg}"))
}

/// HTML-looking text: `<` followed by optional `/`, spaces and a letter, `!` or `?`
/// (the rule of `inspection._TAG_RE`). Display escapes anyway; this keeps markup out of
/// the ledger altogether.
pub fn looks_like_html(s: &str) -> bool {
    let chars: Vec<char> = s.chars().collect();
    chars.iter().enumerate().any(|(i, c)| {
        if *c != '<' {
            return false;
        }
        let mut j = i + 1;
        while j < chars.len() && chars[j].is_whitespace() {
            j += 1;
        }
        if j < chars.len() && chars[j] == '/' {
            j += 1;
            while j < chars.len() && chars[j].is_whitespace() {
                j += 1;
            }
        }
        j < chars.len() && (chars[j].is_ascii_alphabetic() || chars[j] == '!' || chars[j] == '?')
    })
}

fn check_str(what: &str, v: &Value, max: usize, multiline: bool) -> Result<String> {
    let s = v
        .as_str()
        .ok_or_else(|| result_err(format!("{what} must be a string")))?;
    let s = crate::records::py_strip(s);
    if s.is_empty() {
        return Err(result_err(format!("{what} must not be empty")));
    }
    let n = s.chars().count();
    if n > max {
        return Err(result_err(format!(
            "{what} is too long: {n} chars (max {max})"
        )));
    }
    if s.chars()
        .any(|c| c.is_control() && !(multiline && (c == '\n' || c == '\t')))
    {
        return Err(result_err(format!(
            "{what} must not contain control characters"
        )));
    }
    if looks_like_html(s) {
        return Err(result_err(format!("{what} must not contain HTML")));
    }
    Ok(s.to_string())
}

fn check_num(what: &str, v: &Value) -> Result<serde_json::Number> {
    // serde_json numbers are always finite (JSON has no NaN / Infinity)
    match v {
        Value::Number(n) => Ok(n.clone()),
        _ => Err(result_err(format!("{what} must be a number"))),
    }
}

fn check_keys(obj: &Map<String, Value>, allowed: &[&str]) -> Result<()> {
    if let Some(k) = obj.keys().find(|k| !allowed.contains(&k.as_str())) {
        return Err(result_err(format!(
            "unknown key {}; allowed: {}",
            crate::records::py_repr(k),
            allowed.join(", ")
        )));
    }
    Ok(())
}

/// Validate a probe's JSON answer against the header's kind. The answer is the object
/// the probe function returned, without the `kind` tag (a `kind` key, if present, must
/// equal `expected`).
pub fn validate_result(expected: ProbeKind, value: &Value) -> Result<ProbeResult> {
    let obj = value
        .as_object()
        .ok_or_else(|| result_err("must be a JSON object"))?;
    if let Some(k) = obj.get("kind") {
        if k.as_str() != Some(expected.as_str()) {
            return Err(result_err(format!(
                "kind must be {} (the header's)",
                expected.as_str()
            )));
        }
    }
    let opt_label = |obj: &Map<String, Value>| -> Result<Option<String>> {
        obj.get("label")
            .map(|v| check_str("label", v, MAX_LABEL, false))
            .transpose()
    };
    match expected {
        ProbeKind::Number => {
            check_keys(obj, &["kind", "label", "value", "of", "unit"])?;
            let label = check_str(
                "label",
                obj.get("label")
                    .ok_or_else(|| result_err("label is required"))?,
                MAX_LABEL,
                false,
            )?;
            let value = check_num(
                "value",
                obj.get("value")
                    .ok_or_else(|| result_err("value is required"))?,
            )?;
            let of = obj
                .get("of")
                .filter(|v| !v.is_null())
                .map(|v| check_num("of", v))
                .transpose()?;
            let unit = obj
                .get("unit")
                .filter(|v| !v.is_null())
                .map(|v| check_str("unit", v, MAX_UNIT, false))
                .transpose()?;
            Ok(ProbeResult::Number(NumberResult {
                label,
                value,
                of,
                unit,
            }))
        }
        ProbeKind::Text => {
            check_keys(obj, &["kind", "label", "text"])?;
            let label = opt_label(obj)?;
            let text = check_str(
                "text",
                obj.get("text")
                    .ok_or_else(|| result_err("text is required"))?,
                MAX_TEXT,
                true,
            )?;
            Ok(ProbeResult::Text(TextResult { label, text }))
        }
        ProbeKind::Table => {
            check_keys(obj, &["kind", "label", "columns", "rows"])?;
            let label = opt_label(obj)?;
            let columns = obj
                .get("columns")
                .and_then(Value::as_array)
                .ok_or_else(|| result_err("columns must be a list of strings"))?;
            if columns.is_empty() || columns.len() > MAX_COLUMNS {
                return Err(result_err(format!("columns: 1 to {MAX_COLUMNS}")));
            }
            let columns = columns
                .iter()
                .map(|c| check_str("column", c, MAX_LABEL, false))
                .collect::<Result<Vec<_>>>()?;
            let rows = obj
                .get("rows")
                .and_then(Value::as_array)
                .ok_or_else(|| result_err("rows must be a list of lists"))?;
            if rows.len() > MAX_ROWS {
                return Err(result_err(format!(
                    "too many rows: {} (max {MAX_ROWS})",
                    rows.len()
                )));
            }
            let rows = rows
                .iter()
                .map(|r| {
                    let r = r
                        .as_array()
                        .ok_or_else(|| result_err("rows must be a list of lists"))?;
                    if r.len() != columns.len() {
                        return Err(result_err(format!(
                            "a row has {} cells, the table {} columns",
                            r.len(),
                            columns.len()
                        )));
                    }
                    r.iter()
                        .map(|c| match c {
                            Value::Null => Ok(Cell::Empty),
                            Value::Number(_) => check_num("cell", c).map(Cell::Number),
                            Value::String(s) if crate::records::py_strip(s).is_empty() => {
                                Ok(Cell::Empty)
                            }
                            Value::String(_) => {
                                check_str("cell", c, MAX_CELL, false).map(Cell::Text)
                            }
                            _ => Err(result_err("a cell must be a string, a number or null")),
                        })
                        .collect::<Result<Vec<_>>>()
                })
                .collect::<Result<Vec<_>>>()?;
            Ok(ProbeResult::Table(TableResult {
                label,
                columns,
                rows,
            }))
        }
    }
}

/// Parse the host's stdout (≤ [`MAX_OUTPUT_BYTES`]) and [`validate_result`] it.
pub fn parse_output(expected: ProbeKind, stdout: &[u8]) -> Result<ProbeResult> {
    if stdout.len() > MAX_OUTPUT_BYTES {
        return Err(result_err(format!(
            "answer is {} bytes (max {MAX_OUTPUT_BYTES})",
            stdout.len()
        )));
    }
    let v: Value =
        serde_json::from_slice(stdout).map_err(|e| result_err(format!("not JSON: {e}")))?;
    validate_result(expected, &v)
}

// ---------------------------------------------------------------- ledger

/// The only write path of `probe_results`: validate (again) and insert one row in its own
/// locked write. No event, no backup commit.
pub fn set_probe_result(
    ledger: &mut Ledger,
    machine: &str,
    probe: &ProbeKey,
    input_hash: &str,
    probe_hash: Option<&str>,
    result: &ProbeResult,
) -> Result<i64> {
    crate::model::IdeaName::parse(machine).map_err(|_| {
        result_err(format!(
            "invalid machine name {}",
            crate::records::py_repr(machine)
        ))
    })?;
    let is_hex64 = |h: &str| {
        h.len() == 64
            && h.bytes()
                .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase())
    };
    if !is_hex64(input_hash) || probe_hash.is_some_and(|h| !is_hex64(h)) {
        return Err(result_err("hashes must be lowercase sha256 hex"));
    }
    if (probe.scope == ProbeScope::Builtin) != probe_hash.is_none() {
        return Err(result_err(
            "a file probe's result needs its enabled hash; a built-in has none",
        ));
    }
    // Round-trip through the validator: a ProbeResult built in code obeys the same limits.
    let mut v = serde_json::to_value(result)?;
    if let Some(o) = v.as_object_mut() {
        o.remove("kind");
    }
    let checked = validate_result(result.kind(), &v)?;
    let json = checked.to_ledger_json()?;
    let key = probe.to_string();
    ledger.write(|tx| {
        tx.execute(
            "INSERT INTO probe_results (machine, probe, at, input_hash, result_json, probe_hash)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            rusqlite::params![machine, key, crate::db::now(), input_hash, json, probe_hash],
        )?;
        Ok(tx.last_insert_rowid())
    })
}

/// The hash the human enabled for `probe`: the `bound_hash` of the latest decided probe
/// decision, if that decision is `enable` by the human. None = not enabled (never, or
/// disabled since).
pub fn enabled_hash(conn: &rusqlite::Connection, probe: &ProbeKey) -> Result<Option<String>> {
    use rusqlite::OptionalExtension;
    let row: Option<(String, Option<String>, Option<String>)> = conn
        .query_row(
            "SELECT choice, decided_by, bound_hash FROM decisions
             WHERE subject_type = 'probe' AND subject_id = ?1 AND choice IS NOT NULL
             ORDER BY CAST(substr(id, 2) AS INTEGER) DESC LIMIT 1",
            [probe.to_string()],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )
        .optional()?;
    Ok(match row {
        Some((choice, Some(by), hash)) if choice == "enable" && by == "human" => hash,
        _ => None,
    })
}

// ---------------------------------------------------------------- files

/// A probe file found on disk with its parsed header.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ProbeFile {
    #[serde(skip)]
    pub key: ProbeKey,
    pub path: PathBuf,
    /// sha256 hex of the file's bytes.
    pub sha256: String,
    /// Err text when the header is invalid (listed, never run).
    pub header: std::result::Result<ProbeHeader, String>,
}

/// Where a probe stands for `arb status` / the app.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ProbeState {
    /// Never enabled.
    Disabled,
    /// Enabled and the file is unchanged.
    Enabled,
    /// Enabled, but the file changed since ("待重新启用").
    NeedsReenable,
    /// The header is invalid or its api unsupported.
    Invalid(String),
}

/// `<state dir>/probes` (project) or `<config dir>/probes` (global).
pub fn probes_dir(scope: ProbeScope, state_dir: Option<&Path>) -> Option<PathBuf> {
    match scope {
        ProbeScope::Project => state_dir.map(|s| s.join(PROBES_DIR)),
        ProbeScope::Global => Some(crate::clusters::config_dir().join(PROBES_DIR)),
        ProbeScope::Builtin => None,
    }
}

/// Read one probe file (header parsed from comments only; never executed).
pub fn read_file(key: ProbeKey, path: &Path) -> Result<ProbeFile> {
    let bytes = std::fs::read(path)?;
    let header = match std::str::from_utf8(&bytes) {
        Ok(text) => parse_header(text).map_err(|e| e.to_string()),
        Err(e) => Err(format!("not UTF-8 ({e})")),
    };
    Ok(ProbeFile {
        key,
        path: path.to_path_buf(),
        sha256: crate::cards::sha256_hex(&bytes),
        header,
    })
}

/// `<state dir>/probes` and `<config dir>/probes`, by name; the project wins on a name
/// clash (the shadowed global file is left out).
pub fn discover(state_dir: Option<&Path>) -> Result<Vec<ProbeFile>> {
    let mut out: Vec<ProbeFile> = Vec::new();
    for scope in [ProbeScope::Project, ProbeScope::Global] {
        let Some(dir) = probes_dir(scope, state_dir) else {
            continue;
        };
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        let mut paths: Vec<PathBuf> = entries
            .filter_map(|e| e.ok().map(|e| e.path()))
            .filter(|p| p.extension().is_some_and(|x| x == "py"))
            .collect();
        paths.sort();
        for path in paths {
            let Some(name) = path.file_stem().and_then(|s| s.to_str()) else {
                continue;
            };
            let Ok(key) = ProbeKey::new(scope, name) else {
                continue;
            };
            if out.iter().any(|f| f.key.name == key.name) {
                continue; // shadowed by the project's
            }
            out.push(read_file(key, &path)?);
        }
    }
    out.sort_by(|a, b| a.key.name.cmp(&b.key.name));
    Ok(out)
}

/// One probe file's state against the ledger.
pub fn state(conn: &rusqlite::Connection, file: &ProbeFile) -> Result<ProbeState> {
    if let Err(e) = &file.header {
        return Ok(ProbeState::Invalid(e.clone()));
    }
    Ok(match enabled_hash(conn, &file.key)? {
        None => ProbeState::Disabled,
        Some(h) if h == file.sha256 => ProbeState::Enabled,
        Some(_) => ProbeState::NeedsReenable,
    })
}

/// The header, refusing an invalid one.
pub fn header_of(file: &ProbeFile) -> Result<&ProbeHeader> {
    file.header.as_ref().map_err(|e| {
        Error::Arb(format!(
            "probe {} cannot run: {e}",
            crate::records::py_repr(&file.key.name)
        ))
    })
}

/// What the human sees before enabling (`arb probe enable`).
pub fn describe_for_enable(file: &ProbeFile) -> Result<String> {
    let h = header_of(file)?;
    Ok(format!(
        "probe {} ({})\n  文件：{}\n  机器：{}\n  命令：{}\n  每隔：{}\n  结果：{}\n  sha256：{}\n\
         启用的是这份字节：文件改了一个字节就会停用，要你重新启用。",
        file.key.name,
        file.key.scope.as_str(),
        file.path.display(),
        h.machine,
        crate::slurm::shlex_join(&h.argv),
        h.every,
        h.kind.as_str(),
        file.sha256
    ))
}

/// The question of a probe Decision.
pub const ENABLE_QUESTION: &str = "启用这个 probe（绑定文件内容哈希）吗？";

/// `arb probe enable NAME` (human only): record the hash-bound Decision (`probe_enable`
/// event, backup commit). `confirmed` is the human's typed confirmation
/// ([`crate::human::confirm`] after [`describe_for_enable`] was shown).
pub fn enable(
    ledger: &mut Ledger,
    file: &ProbeFile,
    confirmed: &crate::human::Confirmed,
) -> Result<crate::model::Decision> {
    header_of(file)?;
    // the bytes enabled are the bytes on disk now
    let now = read_file(file.key.clone(), &file.path)?;
    if now.sha256 != file.sha256 {
        return Err(Error::Arb(format!(
            "{} changed while you were looking at it; run `arb probe enable {}` again",
            file.path.display(),
            file.key.name
        )));
    }
    let key = file.key.to_string();
    let id = ledger.write(|tx| {
        let id = crate::records::insert_probe_decision(
            tx,
            &key,
            ENABLE_QUESTION,
            "enable",
            Some(&file.sha256),
            confirmed,
        )?;
        tx.event(
            crate::model::EventKind::ProbeEnable,
            &key,
            &serde_json::json!({"decision": id.as_str(), "sha256": file.sha256,
                                "path": file.path.display().to_string()}),
        )?;
        Ok(id)
    })?;
    crate::records::get_decision_in(ledger.conn(), &id)
}

// ---------------------------------------------------------------- the host

/// How long the parser may run.
pub const HOST_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(10);

/// The short-lived host (`python -I`): reads `{"source", "path", "name", "text"}` on stdin,
/// runs the probe's source in a fresh namespace, calls `<name>(text)` and prints its
/// answer as JSON. It gets the bytes the core hashed (never re-reads the file).
const HOST: &str = r#"
import json, sys
req = json.loads(sys.stdin.buffer.read().decode("utf-8"))
ns = {"__name__": "arb_probe", "__file__": req["path"]}
exec(compile(req["source"], req["path"], "exec"), ns)
fn = ns.get(req["name"])
if not callable(fn):
    sys.stderr.write(f"{req['path']} defines no function {req['name']}(text)\n")
    sys.exit(2)
sys.stdout.write(json.dumps(fn(req["text"]), ensure_ascii=False, allow_nan=False))
"#;

/// The last line of `text` (an error's useful part).
fn last_line(text: &str) -> String {
    crate::records::py_splitlines(crate::records::py_strip(text))
        .last()
        .map(|l| l.to_string())
        .unwrap_or_default()
}

/// Run `source`'s parser on `input` (truncated to [`MAX_INPUT_BYTES`]) in the host and
/// validate the answer. The process group is killed after [`HOST_TIMEOUT`].
fn run_host(file: &ProbeFile, source: &[u8], input: &[u8]) -> Result<ProbeResult> {
    use std::io::{Read as _, Write as _};
    use std::os::unix::process::CommandExt as _;
    let header = header_of(file)?;
    let input = &input[..input.len().min(MAX_INPUT_BYTES)];
    let request = serde_json::json!({
        "source": String::from_utf8_lossy(source),
        "path": file.path.display().to_string(),
        "name": file.key.name,
        "text": String::from_utf8_lossy(input),
    });
    let exe = crate::commands::python_exe();
    let mut child = std::process::Command::new(&exe)
        .args(["-I", "-c", HOST])
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .process_group(0)
        .spawn()
        .map_err(|e| {
            Error::Arb(format!(
                "cannot run {exe} for the probe host ({e}); set ARBORIA_PYTHON"
            ))
        })?;
    let mut stdin = child.stdin.take().expect("piped");
    let body = serde_json::to_vec(&request)?;
    let writer = std::thread::spawn(move || {
        let _ = stdin.write_all(&body);
    });
    let mut stdout = child.stdout.take().expect("piped");
    let mut stderr = child.stderr.take().expect("piped");
    let out_reader = std::thread::spawn(move || {
        let mut buf = Vec::new();
        let _ = (&mut stdout)
            .take(MAX_OUTPUT_BYTES as u64 + 1)
            .read_to_end(&mut buf);
        buf
    });
    let err_reader = std::thread::spawn(move || {
        let mut buf = Vec::new();
        let _ = (&mut stderr).take(64 * 1024).read_to_end(&mut buf);
        buf
    });
    let start = std::time::Instant::now();
    let status = loop {
        if let Some(st) = child.try_wait()? {
            break Some(st);
        }
        if start.elapsed() > HOST_TIMEOUT {
            // the whole group: a parser that forked cannot outlive its deadline
            let _ = std::process::Command::new("kill")
                .args(["-KILL", &format!("-{}", child.id())])
                .status();
            let _ = child.kill();
            let _ = child.wait();
            break None;
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    };
    let _ = writer.join();
    let out = out_reader.join().unwrap_or_default();
    let err = err_reader.join().unwrap_or_default();
    let Some(status) = status else {
        return Err(result_err(format!(
            "the parser ran longer than {}s and was stopped",
            HOST_TIMEOUT.as_secs()
        )));
    };
    if !status.success() {
        return Err(result_err(format!(
            "the parser failed: {}",
            last_line(&String::from_utf8_lossy(&err))
        )));
    }
    parse_output(header.kind, &out)
}

/// `arb probe check NAME --input FILE`: run only the parser on a saved input, never the
/// remote command (for the agent to test its probe). Returns the validated result.
pub fn check(file: &ProbeFile, input: &[u8]) -> Result<ProbeResult> {
    let source = std::fs::read(&file.path)?;
    run_host(file, &source, input)
}

/// `arb probe run NAME`: the human runs an enabled probe once now (the bytes on disk
/// must hash to the enabled hash; ssh gate; the header's argv; the host;
/// [`set_probe_result`]).
pub fn run_once(ledger: &mut Ledger, file: &ProbeFile) -> Result<ProbeResult> {
    run_once_with(
        ledger,
        file,
        std::sync::Arc::new(crate::backend::SystemRunner),
    )
}

/// [`run_once`] with a given runner for the remote command (tests).
pub fn run_once_with(
    ledger: &mut Ledger,
    file: &ProbeFile,
    runner: std::sync::Arc<dyn crate::backend::Runner>,
) -> Result<ProbeResult> {
    let header = header_of(file)?.clone();
    let source = std::fs::read(&file.path)?;
    let sha = crate::cards::sha256_hex(&source);
    match enabled_hash(ledger.conn(), &file.key)? {
        None => {
            return Err(Error::Arb(format!(
                "probe {} is not enabled; the human enables it with `arb probe enable {}`",
                file.key.name, file.key.name
            )))
        }
        Some(h) if h != sha => {
            return Err(Error::Arb(format!(
                "{} changed since it was enabled (待重新启用); the human enables it again with `arb probe enable {}`",
                file.path.display(),
                file.key.name
            )))
        }
        Some(_) => {}
    }
    let profile = crate::clusters::load_profile(&header.machine, None)?;
    let session = crate::backend::SshSession::new(&profile.host, runner);
    let out = session.remote(&crate::slurm::shlex_join(&header.argv), None, &[0])?;
    let input = out.stdout.as_bytes();
    let input = &input[..input.len().min(MAX_INPUT_BYTES)];
    let result = run_host(file, &source, input)?;
    set_probe_result(
        ledger,
        &header.machine,
        &file.key,
        &crate::cards::sha256_hex(input),
        Some(&sha),
        &result,
    )?;
    Ok(result)
}

/// `text` made storable as a text result: control characters (but `\n`, `\t`) and `<`
/// shown as spaces / `‹`, cut to [`MAX_TEXT`].
fn plain_text(text: &str) -> String {
    let cleaned: String = text
        .chars()
        .map(|c| match c {
            '\n' | '\t' => c,
            '<' => '‹',
            c if c.is_control() => ' ',
            c => c,
        })
        .collect();
    let cleaned = crate::records::py_strip(&cleaned).to_string();
    if cleaned.chars().count() > MAX_TEXT {
        cleaned.chars().take(MAX_TEXT - 1).collect::<String>() + "…"
    } else {
        cleaned
    }
}

/// The built-in `balance` probe: `Profile.balance_cmd` parsed by
/// [`crate::machine::parse_accounts`]; the profile's account as a number, or on a parse
/// failure a text result with the raw output (escaped on display).
pub fn builtin_balance(profile: &crate::clusters::Profile, stdout: &str) -> ProbeResult {
    let rows = crate::machine::parse_accounts(stdout);
    let found = rows
        .iter()
        .find(|(a, _)| *a == profile.account)
        .and_then(|(_, b)| b.parse::<f64>().ok().filter(|v| v.is_finite()))
        .and_then(|v| {
            if v.fract() == 0.0 && v.abs() < 9e15 {
                Some(serde_json::Number::from(v as i64))
            } else {
                serde_json::Number::from_f64(v)
            }
        });
    match found {
        Some(value) => ProbeResult::Number(NumberResult {
            label: "余额".into(),
            value,
            of: None,
            unit: Some("小时".into()),
        }),
        None => {
            let text = plain_text(stdout);
            ProbeResult::Text(TextResult {
                label: Some("余额".into()),
                text: if text.is_empty() {
                    "（没有输出）".into()
                } else {
                    text
                },
            })
        }
    }
}

/// A result as plain text (the CLI; the app renders its own, escaped).
pub fn result_text(r: &ProbeResult) -> String {
    match r {
        ProbeResult::Number(n) => {
            let mut out = format!("{}: {}", n.label, n.value);
            if let Some(of) = &n.of {
                out.push_str(&format!(" / {of}"));
            }
            if let Some(u) = &n.unit {
                out.push_str(&format!(" {u}"));
            }
            out
        }
        ProbeResult::Text(t) => match &t.label {
            Some(l) => format!("{l}:\n{}", t.text),
            None => t.text.clone(),
        },
        ProbeResult::Table(t) => {
            let cell = |c: &Cell| match c {
                Cell::Number(n) => n.to_string(),
                Cell::Text(s) => s.clone(),
                Cell::Empty => String::new(),
            };
            let mut lines: Vec<String> = t.label.iter().cloned().collect();
            lines.push(t.columns.join("\t"));
            lines.extend(
                t.rows
                    .iter()
                    .map(|r| r.iter().map(cell).collect::<Vec<_>>().join("\t")),
            );
            lines.join("\n")
        }
    }
}

/// The guide (for the agent).
const GUIDE: &str = include_str!("probe_guide.md");

/// `arb probe guide`: the conventions for writing a probe (for the agent).
pub fn guide() -> &'static str {
    GUIDE
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    const EXAMPLE: &str = r#"# /// arb-probe
# api = 1
# machine = "deltaai"
# argv = ["accounts"]          # 写死；核心执行，shlex 拼接
# every = "30m"
# kind = "number"              # number | table | text
# ///
def deltaai_credits(text: str) -> dict:
    """DeltaAI 剩余 SU。"""
    return {"label": "剩余 SU", "value": 199, "of": 200}
"#;

    #[test]
    fn parses_the_adr_example() {
        let h = parse_header(EXAMPLE).unwrap();
        assert_eq!(h.api, 1);
        assert_eq!(h.machine, "deltaai");
        assert_eq!(h.argv, vec!["accounts"]);
        assert_eq!((h.every.as_str(), h.every_s), ("30m", 1800));
        assert_eq!(h.kind, ProbeKind::Number);
    }

    #[test]
    fn header_block_rules() {
        assert!(header_block("print(1)\n").unwrap().is_none());
        assert!(parse_header("x = 1\n")
            .unwrap_err()
            .to_string()
            .contains("missing"));
        let unclosed = "# /// arb-probe\n# api = 1\n";
        assert!(header_block(unclosed)
            .unwrap_err()
            .to_string()
            .contains("not closed"));
        let twice = format!("{EXAMPLE}\n{EXAMPLE}");
        assert!(header_block(&twice)
            .unwrap_err()
            .to_string()
            .contains("more than one"));
        let bad_line = "# /// arb-probe\n#api = 1\n# ///\n";
        assert!(header_block(bad_line).is_err());
        // `#` alone is an empty line; CRLF files work
        let crlf = EXAMPLE
            .replace("# every", "#\n# every")
            .replace('\n', "\r\n");
        assert_eq!(parse_header(&crlf).unwrap().every_s, 1800);
        // other PEP 723 blocks (`script`) are ignored
        let with_script = format!("# /// script\n# dependencies = []\n# ///\n{EXAMPLE}");
        assert!(parse_header(&with_script).is_ok());
    }

    #[test]
    fn header_validation() {
        let with = |from: &str, to: &str| parse_header(&EXAMPLE.replace(from, to));
        assert!(with("api = 1", "api = 2")
            .unwrap_err()
            .to_string()
            .contains("not supported"));
        assert!(with("api = 1", "api = \"1\"").is_err());
        assert!(with("kind = \"number\"", "kind = \"html\"").is_err());
        assert!(with("argv = [\"accounts\"]", "argv = []").is_err());
        assert!(with("argv = [\"accounts\"]", "argv = \"accounts\"").is_err());
        assert!(with("argv = [\"accounts\"]", "argv = [\"a\\nb\"]").is_err());
        assert!(with("machine = \"deltaai\"", "machine = \"Delta AI\"").is_err());
        assert!(with("# api = 1\n", "# api = 1\n# network = true\n")
            .unwrap_err()
            .to_string()
            .contains("unknown key"));
        assert!(with("# api = 1\n", "")
            .unwrap_err()
            .to_string()
            .contains("api is required"));
        assert!(with("every = \"30m\"", "every = \"30\"").is_err());
    }

    #[test]
    fn every_parsing() {
        assert_eq!(parse_every("1m").unwrap(), 60);
        assert_eq!(parse_every("1h30m").unwrap(), 5400);
        assert_eq!(parse_every("2d").unwrap(), 172_800);
        assert_eq!(parse_every("90s").unwrap(), 90);
        assert_eq!(parse_every("7d").unwrap(), MAX_EVERY_S);
        for bad in [
            "", "30", "m", "59s", "8d", "30m1h", "1h1h", "1.5h", " 30m", "30M", "1x",
        ] {
            assert!(parse_every(bad).is_err(), "{bad:?}");
        }
    }

    #[test]
    fn names_and_keys() {
        assert!(validate_name("deltaai_credits").is_ok());
        for bad in ["", "1a", "a-b", "A", &"a".repeat(42)] {
            assert!(validate_name(bad).is_err(), "{bad:?}");
        }
        let k = ProbeKey::parse("project:deltaai_credits").unwrap();
        assert_eq!(k.to_string(), "project:deltaai_credits");
        assert!(ProbeKey::parse("deltaai_credits").is_err());
        assert!(ProbeKey::parse("other:x").is_err());
    }

    #[test]
    fn number_results() {
        let r = validate_result(
            ProbeKind::Number,
            &json!({"label": "剩余 SU", "value": 199, "of": 200}),
        )
        .unwrap();
        assert_eq!(
            r.to_ledger_json().unwrap(),
            r#"{"kind": "number", "label": "剩余 SU", "of": 200, "value": 199}"#
        );
        assert!(
            validate_result(ProbeKind::Number, &json!({"label": "x", "value": "199"})).is_err()
        );
        assert!(validate_result(ProbeKind::Number, &json!({"value": 1})).is_err());
        assert!(validate_result(
            ProbeKind::Number,
            &json!({"label": "x", "value": 1, "extra": 1})
        )
        .is_err());
        assert!(validate_result(
            ProbeKind::Number,
            &json!({"label": "x", "value": 1, "kind": "text"})
        )
        .is_err());
        assert!(validate_result(
            ProbeKind::Number,
            &json!({"label": "x".repeat(41), "value": 1})
        )
        .is_err());
        assert!(validate_result(ProbeKind::Number, &json!([1])).is_err());
        assert!(validate_result(ProbeKind::Text, &json!({"label": "x", "value": 1})).is_err());
    }

    #[test]
    fn text_and_table_results() {
        let t = validate_result(ProbeKind::Text, &json!({"text": "line 1\nline\t2"})).unwrap();
        assert_eq!(
            t,
            ProbeResult::Text(TextResult {
                label: None,
                text: "line 1\nline\t2".into()
            })
        );
        assert!(
            validate_result(ProbeKind::Text, &json!({"text": "x".repeat(MAX_TEXT + 1)})).is_err()
        );
        assert!(validate_result(ProbeKind::Text, &json!({"text": "a\u{1b}[31mb"})).is_err());
        let tab =
            json!({"columns": ["account", "SU"], "rows": [["bjki", 199], ["x", null], ["", 1.5]]});
        let ProbeResult::Table(t) = validate_result(ProbeKind::Table, &tab).unwrap() else {
            panic!()
        };
        assert_eq!(t.rows[1][1], Cell::Empty);
        assert_eq!(t.rows[2][0], Cell::Empty);
        assert!(validate_result(
            ProbeKind::Table,
            &json!({"columns": ["a"], "rows": [["x", "y"]]})
        )
        .is_err());
        assert!(validate_result(ProbeKind::Table, &json!({"columns": [], "rows": []})).is_err());
        assert!(validate_result(
            ProbeKind::Table,
            &json!({"columns": ["a"], "rows": [[true]]})
        )
        .is_err());
        let many: Vec<Value> = (0..=MAX_ROWS).map(|i| json!([i])).collect();
        assert!(
            validate_result(ProbeKind::Table, &json!({"columns": ["a"], "rows": many})).is_err()
        );
    }

    #[test]
    fn html_is_rejected_everywhere() {
        for s in [
            "<b>x</b>",
            "a < script",
            "x</ div>",
            "<!-- c -->",
            "<?php",
            "<img src=x>",
        ] {
            assert!(looks_like_html(s), "{s:?}");
        }
        assert!(!looks_like_html("1 < 2 and 3 > 2"));
        assert!(!looks_like_html("a <= b, x<3"));
        assert!(validate_result(
            ProbeKind::Text,
            &json!({"text": "<script>alert(1)</script>"})
        )
        .is_err());
        assert!(validate_result(
            ProbeKind::Number,
            &json!({"label": "<b>SU</b>", "value": 1})
        )
        .is_err());
        assert!(validate_result(
            ProbeKind::Table,
            &json!({"columns": ["a"], "rows": [["<a href=x>"]]})
        )
        .is_err());
        assert!(validate_result(ProbeKind::Text, &json!({"text": "SU < 10"})).is_ok());
    }

    #[test]
    fn output_parsing() {
        assert!(parse_output(ProbeKind::Number, br#"{"label": "x", "value": 1}"#).is_ok());
        assert!(parse_output(ProbeKind::Number, b"not json").is_err());
        assert!(parse_output(ProbeKind::Text, &vec![b' '; MAX_OUTPUT_BYTES + 1]).is_err());
    }

    #[test]
    fn enable_binds_the_files_bytes() {
        let dir = std::env::temp_dir().join(format!("arb-probe-enable-{}", std::process::id()));
        let probes = dir.join(PROBES_DIR);
        std::fs::create_dir_all(&probes).unwrap();
        let path = probes.join("credits.py");
        std::fs::write(
            &path,
            format!("{EXAMPLE}\ndef credits(text):\n    return {{}}\n"),
        )
        .unwrap();
        let mut ledger = Ledger::open(&dir).unwrap();
        crate::db::init_backup_repo(&dir).unwrap();
        let file = read_file(
            ProbeKey::new(ProbeScope::Project, "credits").unwrap(),
            &path,
        )
        .unwrap();
        assert_eq!(state(ledger.conn(), &file).unwrap(), ProbeState::Disabled);
        let shown = describe_for_enable(&file).unwrap();
        assert!(
            shown.contains("命令：accounts") && shown.contains(&file.sha256),
            "{shown}"
        );
        let d = enable(&mut ledger, &file, &crate::human::Confirmed::for_tests()).unwrap();
        assert_eq!(d.choice.as_deref(), Some("enable"));
        assert_eq!(d.decided_by.as_deref(), Some("human"));
        assert_eq!(
            enabled_hash(ledger.conn(), &file.key).unwrap().as_deref(),
            Some(file.sha256.as_str())
        );
        assert_eq!(state(ledger.conn(), &file).unwrap(), ProbeState::Enabled);
        // one changed byte: the enabled hash no longer matches
        std::fs::write(
            &path,
            format!("{EXAMPLE}\ndef credits(text):\n    return {{ }}\n"),
        )
        .unwrap();
        let changed = read_file(file.key.clone(), &path).unwrap();
        assert_eq!(
            state(ledger.conn(), &changed).unwrap(),
            ProbeState::NeedsReenable
        );
        // enabling refuses bytes that changed after they were shown
        let err = enable(&mut ledger, &file, &crate::human::Confirmed::for_tests()).unwrap_err();
        assert!(
            err.to_string().contains("changed while you were looking"),
            "{err}"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }
}
