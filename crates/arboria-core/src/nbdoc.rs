//! The app's notebook file (ADR 0005): markdown with a small frontmatter, text and
//! ` ```arb {#id} ` command blocks. The file is the human's; results never go in it.
//!
//! - [`parse`] never fails: anything it does not understand stays text.
//! - [`render`] writes the canonical form; `render(parse(render(d))) == render(d)`.
//! - A block's id is 4–12 lowercase letters, digits or `-`, unique in the notebook;
//!   [`ensure_ids`] gives missing or repeated ones a new id (only those blocks change).
//! - Only top-level fences (no indentation) are blocks; fences inside other code blocks
//!   are text.

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// The format version in `arboria:`.
pub const FORMAT: u32 = 1;
/// The worktree link to an idea's notebook.
pub const LINK_NAME: &str = "notebook.md";
/// The sentence a new seed notebook ends with (left out when the seed is planted).
pub const SEED_PLACEHOLDER: &str = "还没有仓库：先把想法写下来，想清楚了再种进项目。";
/// The notebook file's extension.
pub const EXT: &str = "md";

/// One command block.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Block {
    pub id: String,
    pub code: String,
}

/// A piece of the notebook, in order.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum Part {
    /// Markdown text, without the blank lines around it.
    Text {
        text: String,
    },
    Block(Block),
}

/// A parsed notebook.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct Doc {
    /// The frontmatter lines (`key: value`), in order, as written.
    pub front: Vec<(String, String)>,
    pub parts: Vec<Part>,
}

impl Doc {
    pub fn get(&self, key: &str) -> Option<&str> {
        self.front
            .iter()
            .find(|(k, _)| k == key)
            .map(|(_, v)| v.as_str())
    }

    pub fn set(&mut self, key: &str, value: &str) {
        match self.front.iter_mut().find(|(k, _)| k == key) {
            Some(kv) => kv.1 = value.to_string(),
            None => self.front.push((key.to_string(), value.to_string())),
        }
    }

    pub fn blocks(&self) -> impl Iterator<Item = &Block> {
        self.parts.iter().filter_map(|p| match p {
            Part::Block(b) => Some(b),
            Part::Text { .. } => None,
        })
    }

    pub fn block(&self, id: &str) -> Option<&Block> {
        self.blocks().find(|b| b.id == id)
    }

    /// The text parts joined (what a seed passes on when it is planted).
    pub fn text_only(&self) -> String {
        self.parts
            .iter()
            .filter_map(|p| match p {
                Part::Text { text } => Some(text.as_str()),
                Part::Block(_) => None,
            })
            .collect::<Vec<_>>()
            .join("\n\n")
    }
}

/// Whether `s` is a valid block id.
pub fn valid_id(s: &str) -> bool {
    (4..=12).contains(&s.len())
        && s.bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
}

/// A fence line: its character, its length and the info string after it.
fn fence(line: &str) -> Option<(u8, usize, &str)> {
    let c = *line.as_bytes().first()?;
    if c != b'`' && c != b'~' {
        return None;
    }
    let n = line.bytes().take_while(|&b| b == c).count();
    if n < 3 {
        return None;
    }
    let info = &line[n..];
    if c == b'`' && info.contains('`') {
        return None;
    }
    Some((c, n, info.trim()))
}

/// `arb`, `arb {#id}`: Some(id or "") for an arb block's info string.
fn arb_info(info: &str) -> Option<String> {
    let rest = info.strip_prefix("arb")?;
    if rest.is_empty() {
        return Some(String::new());
    }
    if !rest.starts_with(char::is_whitespace) {
        return None;
    }
    let attr = rest.trim();
    let id = attr
        .strip_prefix("{#")
        .and_then(|a| a.strip_suffix('}'))
        .filter(|id| valid_id(id))
        .unwrap_or("");
    Some(id.to_string())
}

fn closes(line: &str, c: u8, n: usize) -> bool {
    let t = line.trim_end();
    t.len() >= n && t.bytes().all(|b| b == c)
}

fn push_text(parts: &mut Vec<Part>, lines: &mut Vec<&str>) {
    let text = lines.join("\n");
    lines.clear();
    let text = text.trim_matches('\n');
    let trimmed = text.trim_end();
    if !trimmed.trim().is_empty() {
        parts.push(Part::Text {
            text: trimmed.to_string(),
        });
    }
}

/// Read a notebook (never fails: what is not a block is text).
pub fn parse(src: &str) -> Doc {
    let src = src.replace("\r\n", "\n");
    let mut lines: Vec<&str> = src.split('\n').collect();
    if lines.last() == Some(&"") {
        lines.pop();
    }
    let mut doc = Doc::default();
    let mut i = 0;
    if lines.first().map(|l| l.trim_end()) == Some("---") {
        if let Some(end) = lines.iter().skip(1).position(|l| l.trim_end() == "---") {
            for l in &lines[1..=end] {
                if let Some((k, v)) = l.split_once(':') {
                    doc.front.push((k.trim().to_string(), v.trim().to_string()));
                }
            }
            i = end + 2;
        }
    }
    let mut text: Vec<&str> = Vec::new();
    while i < lines.len() {
        let line = lines[i];
        let Some((c, n, info)) = fence(line) else {
            text.push(line);
            i += 1;
            continue;
        };
        // find the closing fence
        let close = (i + 1..lines.len()).find(|&j| closes(lines[j], c, n));
        match (arb_info(info), close) {
            (Some(id), Some(end)) => {
                push_text(&mut doc.parts, &mut text);
                doc.parts.push(Part::Block(Block {
                    id,
                    code: lines[i + 1..end].join("\n"),
                }));
                i = end + 1;
            }
            (_, close) => {
                // another code block (or an unclosed fence): text, as written
                let end = close.unwrap_or(lines.len() - 1);
                text.extend_from_slice(&lines[i..=end]);
                i = end + 1;
            }
        }
    }
    push_text(&mut doc.parts, &mut text);
    doc
}

/// The canonical file.
pub fn render(doc: &Doc) -> String {
    let mut out = String::new();
    if !doc.front.is_empty() {
        out.push_str("---\n");
        for (k, v) in &doc.front {
            out.push_str(k);
            out.push_str(": ");
            out.push_str(v);
            out.push('\n');
        }
        out.push_str("---\n");
    }
    for (i, p) in doc.parts.iter().enumerate() {
        if i > 0 {
            out.push('\n');
        }
        match p {
            Part::Text { text } => {
                out.push_str(text.trim_matches('\n'));
                out.push('\n');
            }
            Part::Block(b) => {
                let longest = b
                    .code
                    .split('\n')
                    .map(|l| l.bytes().take_while(|&c| c == b'`').count())
                    .max()
                    .unwrap_or(0);
                let f = "`".repeat(longest.max(2) + 1);
                out.push_str(&f);
                if b.id.is_empty() {
                    out.push_str("arb\n");
                } else {
                    out.push_str(&format!("arb {{#{}}}\n", b.id));
                }
                if !b.code.is_empty() {
                    out.push_str(&b.code);
                    out.push('\n');
                }
                out.push_str(&f);
                out.push('\n');
            }
        }
    }
    out
}

/// A new block id not in `taken`.
pub fn new_id(taken: &HashSet<String>) -> String {
    const ALPHABET: &[u8] = b"abcdefghijklmnopqrstuvwxyz0123456789";
    let mut seed = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0)
        ^ (std::process::id() as u128) << 64;
    loop {
        let h = crate::cards::sha256_hex(seed.to_string().as_bytes());
        let id: String = h
            .as_bytes()
            .chunks(2)
            .take(4)
            .map(|c| {
                let v = u8::from_str_radix(std::str::from_utf8(c).unwrap_or("0"), 16).unwrap_or(0);
                ALPHABET[v as usize % ALPHABET.len()] as char
            })
            .collect();
        if id.as_bytes()[0].is_ascii_lowercase() && !taken.contains(&id) {
            return id;
        }
        seed = seed.wrapping_add(0x9e37_79b9);
    }
}

/// Give blocks without an id (or with a repeated one) a new id; true when any changed.
pub fn ensure_ids(doc: &mut Doc) -> bool {
    let mut taken: HashSet<String> = HashSet::new();
    let mut changed = false;
    let mut needs = Vec::new();
    for (i, p) in doc.parts.iter().enumerate() {
        if let Part::Block(b) = p {
            if b.id.is_empty() || !taken.insert(b.id.clone()) {
                needs.push(i);
            }
        }
    }
    for i in needs {
        let id = new_id(&taken);
        taken.insert(id.clone());
        if let Part::Block(b) = &mut doc.parts[i] {
            b.id = id;
            changed = true;
        }
    }
    changed
}

// ---------------------------------------------------------------- templates

/// What a new idea's notebook starts with.
pub struct IdeaStart<'a> {
    pub idea: &'a str,
    pub hypothesis: &'a str,
    pub metric: &'a str,
    pub parent: Option<&'a str>,
    pub from_run: Option<&'a str>,
    /// (seed name, the seed notebook's text) when planted from a seed.
    pub seed: Option<(&'a str, &'a str)>,
}

fn one_line(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// A new idea's notebook.
pub fn idea_notebook(s: &IdeaStart<'_>) -> String {
    let mut doc = Doc::default();
    doc.set("arboria", &FORMAT.to_string());
    doc.set("idea", s.idea);
    let mut head = format!(
        "# {}\n\n假设：{}\n\n指标：{}",
        s.idea,
        one_line(s.hypothesis),
        one_line(s.metric)
    );
    match (s.parent, s.from_run) {
        (Some(p), Some(r)) => head.push_str(&format!("\n\n从 `{p}` 的 `{r}` 分出。")),
        (Some(p), None) => head.push_str(&format!("\n\n从 `{p}` 分出。")),
        (None, Some(r)) => head.push_str(&format!("\n\n从 `{r}` 分出。")),
        (None, None) => {}
    }
    if let Some((seed, _)) = s.seed {
        head.push_str(&format!("\n\n来自种子 `{seed}`。"));
    }
    doc.parts.push(Part::Text { text: head });
    if let Some((_, text)) = s.seed {
        let text = text.trim();
        if !text.is_empty() {
            let quoted: Vec<String> = text
                .lines()
                .map(|l| {
                    if l.is_empty() {
                        ">".to_string()
                    } else {
                        format!("> {l}")
                    }
                })
                .collect();
            doc.parts.push(Part::Text {
                text: format!("## 种子里的思路\n\n{}", quoted.join("\n")),
            });
        }
    }
    doc.parts.push(Part::Text {
        text: "## 记录\n\n先看看已有的 run：".to_string(),
    });
    doc.parts.push(Part::Block(Block {
        id: String::new(),
        code: "/runs".to_string(),
    }));
    ensure_ids(&mut doc);
    render(&doc)
}

/// A new seed's notebook.
pub fn seed_notebook(
    name: &str,
    gist: &str,
    hypothesis: Option<&str>,
    metric: Option<&str>,
) -> String {
    let mut doc = Doc::default();
    doc.set("arboria", &FORMAT.to_string());
    doc.set("seed", name);
    let mut text = format!("# {name}\n\n{}", one_line(gist));
    if let Some(h) = hypothesis.filter(|h| !h.trim().is_empty()) {
        text.push_str(&format!("\n\n假设：{}", one_line(h)));
    }
    if let Some(m) = metric.filter(|m| !m.trim().is_empty()) {
        text.push_str(&format!("\n\n指标：{}", one_line(m)));
    }
    text.push_str(&format!("\n\n## 思路\n\n{SEED_PLACEHOLDER}"));
    doc.parts.push(Part::Text { text });
    render(&doc)
}

// ---------------------------------------------------------------- where

/// `.arboria/notebooks/<idea>.md`.
pub fn idea_path(state_dir: &Path, idea: &str) -> PathBuf {
    state_dir.join("notebooks").join(format!("{idea}.{EXT}"))
}

/// `<home>/seeds/<seed>.md`.
pub fn seed_path(seeds_dir: &Path, seed: &str) -> PathBuf {
    seeds_dir.join(format!("{seed}.{EXT}"))
}

/// Write a new notebook; false (and nothing written) when the file exists.
pub fn write_new(path: &Path, text: &str) -> crate::Result<bool> {
    if path.exists() {
        return Ok(false);
    }
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    use std::io::Write as _;
    match std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
    {
        Ok(mut f) => {
            f.write_all(text.as_bytes())?;
            Ok(true)
        }
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => Ok(false),
        Err(e) => Err(e.into()),
    }
}

/// Save the human's edit: canonical, atomically (temp file + rename).
pub fn save(path: &Path, doc: &Doc) -> crate::Result<String> {
    let text = render(doc);
    let tmp = path.with_extension("md.tmp");
    std::fs::write(&tmp, &text)?;
    std::fs::rename(&tmp, path)?;
    Ok(text)
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = "---\narboria: 1\nidea: warmup-long\n---\n前 2k 步的尖峰。\n\n```arb {#k3f9}\n/plot r001 r002 val/loss\n```\n\n尖峰在 r002 里基本消失了。\n";

    #[test]
    fn the_sample_round_trips() {
        let d = parse(SAMPLE);
        assert_eq!(d.get("idea"), Some("warmup-long"));
        assert_eq!(d.blocks().count(), 1);
        assert_eq!(d.block("k3f9").unwrap().code, "/plot r001 r002 val/loss");
        assert_eq!(render(&d), SAMPLE);
    }

    #[test]
    fn other_code_blocks_and_odd_fences_stay_text() {
        let src = "intro\n\n```python\n```arb {#zzzz}\nx\n```\n\n````arb {#ab12}\n```\ninside\n```\n````\n\n~~~\nnot closed";
        let d = parse(src);
        let ids: Vec<_> = d.blocks().map(|b| b.id.clone()).collect();
        assert_eq!(ids, ["ab12"], "{d:#?}");
        assert_eq!(d.block("ab12").unwrap().code, "```\ninside\n```");
        let again = render(&d);
        assert_eq!(render(&parse(&again)), again);
        assert!(
            again.contains("````arb {#ab12}\n```\ninside\n```\n````"),
            "{again}"
        );
        assert!(again.contains("~~~\nnot closed"));
    }

    #[test]
    fn ids_are_given_once() {
        let mut d = parse("```arb\na\n```\n\n```arb {#same}\nb\n```\n\n```arb {#same}\nc\n```\n\n```arb {#BAD!}\nd\n```\n");
        assert!(ensure_ids(&mut d));
        let ids: Vec<_> = d.blocks().map(|b| b.id.clone()).collect();
        assert_eq!(ids[1], "same");
        assert_eq!(ids.iter().collect::<HashSet<_>>().len(), 4, "{ids:?}");
        assert!(ids.iter().all(|i| valid_id(i)));
        assert!(!ensure_ids(&mut d));
    }

    #[test]
    fn templates_parse_back() {
        let t = idea_notebook(&IdeaStart {
            idea: "lr-warmup",
            hypothesis: "长 warmup\n去掉尖峰",
            metric: "val/loss",
            parent: Some("base"),
            from_run: Some("r002"),
            seed: Some(("warm", "# warm\n\n一个想法\n\n第二段")),
        });
        let d = parse(&t);
        assert_eq!(d.get("idea"), Some("lr-warmup"));
        assert_eq!(d.blocks().count(), 1);
        assert!(t.contains("假设：长 warmup 去掉尖峰"));
        assert!(t.contains("从 `base` 的 `r002` 分出"));
        assert!(t.contains("> 一个想法\n>\n> 第二段"), "{t}");
        assert_eq!(render(&d), t);
        let s = seed_notebook("warm", "一句话", Some("h"), None);
        let d = parse(&s);
        assert_eq!(d.get("seed"), Some("warm"));
        assert!(d.text_only().contains("一句话"));
        assert_eq!(render(&d), s);
    }

    #[test]
    fn crlf_and_missing_newline_are_normalized() {
        let d = parse("a\r\n\r\n```arb {#abcd}\r\nx = 1\r\n```");
        assert_eq!(render(&d), "a\n\n```arb {#abcd}\nx = 1\n```\n");
    }

    #[test]
    fn write_new_never_overwrites() {
        let dir = std::env::temp_dir().join(format!("nbdoc-{}", std::process::id()));
        let p = dir.join("x.md");
        let _ = std::fs::remove_dir_all(&dir);
        assert!(write_new(&p, "one").unwrap());
        assert!(!write_new(&p, "two").unwrap());
        assert_eq!(std::fs::read_to_string(&p).unwrap(), "one");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
