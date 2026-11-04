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
//! Real in phase 1: [`parse_header`], [`parse_every`], [`validate_name`],
//! [`validate_result`], [`set_probe_result`], [`enabled_hash`], [`ProbeKey`]. Stubbed:
//! discovery, enabling, running, the host, the guide.

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

// ---------------------------------------------------------------- files (stubs)

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

/// `<state dir>/probes` and `<config dir>/probes`; the project wins on a name clash.
pub fn discover(state_dir: Option<&Path>) -> Result<Vec<ProbeFile>> {
    let _ = state_dir;
    todo!("probe discovery (ADR 0003; mirrors arboria.commands.discover)")
}

/// One probe file's state against the ledger.
pub fn state(conn: &rusqlite::Connection, file: &ProbeFile) -> Result<ProbeState> {
    let _ = (conn, file);
    todo!("probe state (ADR 0003)")
}

/// `arb probe enable NAME` (human only, [`crate::human::require_human`]): show machine,
/// argv, every and the hash, the human types the name, record the hash-bound Decision
/// (`probe_enable` event, backup commit).
pub fn enable(
    ledger: &mut Ledger,
    file: &ProbeFile,
    provenance: &crate::human::Provenance,
) -> Result<crate::model::Decision> {
    let _ = (ledger, file, provenance);
    todo!("arb probe enable (ADR 0003)")
}

/// `arb probe check NAME --input FILE`: run only the parser on a saved input, never the
/// remote command (for the agent to test its probe). Returns the validated result.
pub fn check(file: &ProbeFile, input: &[u8]) -> Result<ProbeResult> {
    let _ = (file, input);
    todo!("arb probe check (ADR 0003): run the host on `input`")
}

/// `arb probe run NAME`: the human runs an enabled probe once now (ssh gate, argv, host,
/// [`set_probe_result`]).
pub fn run_once(ledger: &mut Ledger, file: &ProbeFile) -> Result<ProbeResult> {
    let _ = (ledger, file);
    todo!("arb probe run (ADR 0003)")
}

/// The built-in `balance` probe: `Profile.balance_cmd` parsed by
/// [`crate::machine::parse_accounts`]; on a parse failure a text result with the raw
/// output (escaped on display).
pub fn builtin_balance(profile: &crate::clusters::Profile, stdout: &str) -> ProbeResult {
    let _ = (profile, stdout);
    todo!("built-in balance probe (ADR 0003)")
}

/// `arb probe guide`: the conventions for writing a probe (for the agent).
pub fn guide() -> &'static str {
    todo!("arb probe guide (ADR 0003)")
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
}
