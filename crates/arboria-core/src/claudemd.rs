//! The arb-managed block in the main worktree's CLAUDE.md: stable rules + accepted
//! lessons. Ports `arboria.claudemd`.
//!
//! Invariants:
//! - Only the text between [`BEGIN`] and [`END`] is arb's; everything else in the file is
//!   the user's and is preserved byte for byte. Duplicate or unbalanced markers are an
//!   error, never "repaired".
//! - The block has at most [`MAX_BLOCK_LINES`] lines and [`MAX_ACCEPTED_LESSONS`]
//!   lessons. Current experiment state is never written here (the hook delivers it).
//! - The auto-commit touches only CLAUDE.md (`git commit --only`), and is skipped with a
//!   hint during a merge / rebase / cherry-pick, without a user identity, or when
//!   disabled; it never uses the user's index otherwise.
//! - The rules text ([`RULES`], [`HOME_RULES`]) is Python's verbatim until the app
//!   changes the agent workflow (ADR 0001).

use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

use serde::Serialize;

use crate::db::{self, Ledger};
use crate::model::Lesson;
use crate::paths::RepoContext;
use crate::records::py_strip;
use crate::{bail, gitutil, Error, Result};

/// The file.
pub const FILENAME: &str = "CLAUDE.md";
/// Block start marker.
pub const BEGIN: &str = "<!-- arboria:begin — 由 arb 管理，块内修改会被覆盖 -->";
/// Block end marker.
pub const END: &str = "<!-- arboria:end -->";
/// Lines in the block.
pub const MAX_BLOCK_LINES: usize = 50;
/// Accepted lessons in the block.
pub const MAX_ACCEPTED_LESSONS: usize = 20;
/// The auto-commit's message.
pub const COMMIT_MESSAGE: &str = "arboria: update CLAUDE.md";
/// Heading of the lessons section.
pub const LESSONS_HEADING: &str = "## 经验（已确认）";
/// The project rules (Python `claudemd.RULES`, verbatim).
pub const RULES: &str = r#"## 研究工作流（Arboria）
- 实验状态只通过 `arb` 读写，不要手改 `.arboria/`。
- 当前状态（idea、run、待办、待批准的 run、最新经验）由会话开始时的 hook 注入；本文件只放稳定规则和已确认的经验。
- 一个 idea 一个文件夹（worktree）：只在当前 idea 的文件夹里改代码。
- `notebook.py`（marimo，链接到 `.arboria/notebooks/<idea>.py`）是人的：读它了解人的想法和分析，绝不修改。
- 你的工作来自待办：`arb todo list` 查看，做完用 `arb todo done tNNN --result "..."`（≤600 字）交代结果。
- 命令待办（人在 notebook 里调用了 `lab.<名字>(...)`）：先运行 `arb commands guide` 看约定，只写 `.arboria/commands/<名字>.py`（或全局命令目录）里的那个文件，再 `arb todo done`。读代码待办（`lab.inspect(...)`）按 `arb inspect guide`，用 `arb inspect done` 交付。
- 同一类分析在 notebook 里反复出现，或待办要的是一次分析：写成命令（默认项目范围），在 `arb todo done` 里写明 `lab.<名字>(...)` 怎么调用。
- 看状态用 `arb status`。`arb note` 只由人写；你的结论写进 `arb todo done --result` 或 `arb run summary RUN "..."`。
- 上集群：`arb submit --reason "..." --gpus N --time 12h [-- CMD]`，或 `arb submit --from rNNN --set k=v --reason "..."`。它只在本地生成快照和审核单，不会提交。
- 批准只能由人在自己的终端里做：你不能运行 `arb approve` 和 `arb lesson accept`；绝不直接 `sbatch`/`srun`，也不要 ssh 上集群。
- 看集群上的日志和文件用 `arb logs RUN`、`arb remote ls RUN`，不用 ssh；跑完的输出在 `.arboria/runs/RUN/out/`。
- 发现可复用的教训用 `arb lesson propose "..."` 提出，由人确认；不要直接写进 CLAUDE.md。
- 训练代码约定：
  1. 启动时把完整的解析后配置导出为 `$ARBORIA_OUT/config.yaml`。
  2. `wandb.init(id=ARBORIA_WANDB_ID, group=ARBORIA_IDEA)`。
  3. 路径只从 `ARBORIA_*` 变量读取。
  4. 可续训：checkpoint 存到 `ARBORIA_CKPT_DIR`，重启后能接着跑（超过分区上限的 run 会分段）。"#;
/// The seed home's rules (Python `claudemd.HOME_RULES`, verbatim).
pub const HOME_RULES: &str = r#"## 种子园（Arboria）
- 这里是人的种子园：还没有仓库的研究想法。当前的种子和待办由会话开始时的 hook 注入。
- `seeds/<名字>.py`（marimo）是人的 notebook：读它了解人的想法，绝不修改；也不要改 `.arboria/`。
- 种子就是还没有仓库的 idea：在这里（仓库外）`arb idea` 管的是种子。看种子用 `arb idea list`、`arb idea show NAME`。种子只由人写：`arb idea new/set/drop/revive`（以及旧名 `arb seed …`）你都不能运行。
- 种下（在目标仓库里 `arb idea plant NAME`）由人在自己的终端里做：它会在那个仓库里建 idea 的分支和 worktree。
- 你的工作来自待办：`arb todo list` 查看，做完用 `arb todo done tNNN --result "..."`（≤600 字）交代结果。
- 命令待办（人在种子 notebook 里调用了 `lab.<名字>(...)`）：先运行 `arb commands guide`，只写全局命令目录里的那个文件；种子没有 run，命令里不用 `ctx.run`。
- 讨论想法时引用种子 notebook 里的内容；你的结论写进 `arb todo done --result`，不写进 notebook。"#;

/// What a begin marker line starts with (the rest may differ).
pub const BEGIN_PREFIX: &str = "<!-- arboria:begin";
/// What an end marker line starts with.
pub const END_PREFIX: &str = "<!-- arboria:end";

// ---------------------------------------------------------------- lessons

/// Accepted lessons, newest first (at most `limit`).
pub fn accepted_lessons(ledger: &Ledger, limit: Option<usize>) -> Result<Vec<Lesson>> {
    let mut sql = String::from(
        "SELECT * FROM lessons WHERE status = 'accepted' \
         ORDER BY COALESCE(decided_at, created_at) DESC, id DESC",
    );
    if let Some(n) = limit.filter(|&n| n > 0) {
        sql.push_str(&format!(" LIMIT {n}"));
    }
    db::all(ledger.conn(), &sql, [])
}

/// `- <text>（来自 <source>，<YYYY-MM-DD>）`.
pub fn lesson_line(lesson: &Lesson) -> String {
    let when = lesson
        .decided_at
        .as_deref()
        .filter(|d| !d.is_empty())
        .unwrap_or(&lesson.created_at);
    let date: String = when.chars().take(10).collect();
    let origin = match lesson.source_id.as_deref() {
        Some(s) if !s.is_empty() => format!("来自 {s}，"),
        _ => String::new(),
    };
    format!("- {}（{origin}{date}）", lesson.text)
}

// ---------------------------------------------------------------- block

/// The whole managed block (markers included, trailing newline); enforces the line cap.
pub fn render_block(lessons: &[Lesson]) -> Result<String> {
    let mut lines: Vec<String> = vec![BEGIN.into()];
    lines.extend(RULES.lines().map(String::from));
    lines.push(String::new());
    lines.push(LESSONS_HEADING.into());
    if lessons.is_empty() {
        lines.push("- 暂无".into());
    } else {
        lines.extend(lessons.iter().map(lesson_line));
    }
    lines.push(END.into());
    if lines.len() > MAX_BLOCK_LINES {
        bail!(
            "the CLAUDE.md block would have {} lines (max {MAX_BLOCK_LINES}); \
             retire or merge some lessons first (`arb lesson retire ID`)",
            lines.len()
        );
    }
    Ok(lines.join("\n") + "\n")
}

/// `(start, end including the terminator, stripped content)` of each line, splitting
/// like Python's `str.splitlines(keepends=True)` on the file's bytes.
fn line_spans(text: &[u8]) -> Vec<(usize, usize, String)> {
    let mut spans = Vec::new();
    let mut start = 0;
    let mut i = 0;
    let n = text.len();
    while i < n {
        let len = match text[i] {
            b'\r' if text.get(i + 1) == Some(&b'\n') => 2,
            b'\n' | b'\r' | 0x0b | 0x0c | 0x1c..=0x1e => 1,
            0xc2 if text.get(i + 1) == Some(&0x85) => 2,
            0xe2 if text.get(i + 1) == Some(&0x80)
                && matches!(text.get(i + 2), Some(0xa8 | 0xa9)) =>
            {
                3
            }
            _ => 0,
        };
        if len == 0 {
            i += 1;
            continue;
        }
        let content = String::from_utf8_lossy(&text[start..i]);
        spans.push((start, i + len, py_strip(&content).to_string()));
        i += len;
        start = i;
    }
    if start < n {
        let content = String::from_utf8_lossy(&text[start..]);
        spans.push((start, n, py_strip(&content).to_string()));
    }
    spans
}

/// Byte range `[start, end)` of the block in `text` (None when absent; error on bad
/// markers). The file is bytes: what is outside the block is kept as is, even invalid
/// UTF-8.
pub fn find_block(text: &[u8], where_: &str) -> Result<Option<(usize, usize)>> {
    let spans = line_spans(text);
    let begins: Vec<_> = spans
        .iter()
        .filter(|s| s.2.starts_with(BEGIN_PREFIX))
        .collect();
    let ends: Vec<_> = spans
        .iter()
        .filter(|s| s.2.starts_with(END_PREFIX))
        .collect();
    if begins.is_empty() && ends.is_empty() {
        return Ok(None);
    }
    let hint = format!(
        "; fix the markers in {where_} by hand (keep exactly one `{BEGIN_PREFIX} … -->` \
         line followed by one `{END}` line, or delete both), then run `arb claudemd sync`"
    );
    if begins.len() > 1 || ends.len() > 1 {
        bail!(
            "{where_} has {} arboria:begin and {} arboria:end markers (expected one of each){hint}",
            begins.len(),
            ends.len()
        );
    }
    if begins.is_empty() {
        bail!("{where_} has an arboria:end marker without arboria:begin{hint}");
    }
    if ends.is_empty() {
        bail!("{where_} has an arboria:begin marker without arboria:end{hint}");
    }
    if ends[0].0 < begins[0].0 {
        bail!("{where_} has arboria:end before arboria:begin{hint}");
    }
    Ok(Some((begins[0].0, ends[0].1)))
}

/// `text` (None: no file) with the block replaced or appended; returns
/// `(new bytes, action)`, action = `created` | `replaced` | `appended`.
pub fn apply_block(
    text: Option<&[u8]>,
    block: &str,
    where_: &str,
) -> Result<(Vec<u8>, &'static str)> {
    let Some(text) = text else {
        return Ok((block.as_bytes().to_vec(), "created"));
    };
    if let Some((start, end)) = find_block(text, where_)? {
        let mut out = text[..start].to_vec();
        out.extend_from_slice(block.as_bytes());
        out.extend_from_slice(&text[end..]);
        return Ok((out, "replaced"));
    }
    if text.is_empty() {
        return Ok((block.as_bytes().to_vec(), "appended"));
    }
    let sep: &[u8] = if text.ends_with(b"\n\n") {
        b""
    } else if text.ends_with(b"\n") {
        b"\n"
    } else {
        b"\n\n"
    };
    let mut out = text.to_vec();
    out.extend_from_slice(sep);
    out.extend_from_slice(block.as_bytes());
    Ok((out, "appended"))
}

// ---------------------------------------------------------------- config / git

/// `[claudemd] autocommit` from arboria.toml (default true).
pub fn autocommit_enabled(ctx: &RepoContext) -> Result<bool> {
    let path = ctx.config_path();
    if !path.is_file() {
        return Ok(true);
    }
    let bytes = std::fs::read(&path)?;
    let invalid = |detail: String| {
        Error::Arb(format!(
            "{} is not valid TOML ({detail}); fix it first",
            path.display()
        ))
    };
    let text = String::from_utf8(bytes).map_err(|e| invalid(e.to_string()))?;
    let cfg: toml::Table = text
        .parse()
        .map_err(|e: toml::de::Error| invalid(e.message().into()))?;
    let value = match cfg.get("claudemd") {
        Some(toml::Value::Table(t)) => t.get("autocommit"),
        _ => None,
    };
    match value {
        None => Ok(true),
        Some(toml::Value::Boolean(b)) => Ok(*b),
        Some(_) => bail!(
            "{}: [claudemd] autocommit must be true or false",
            path.display()
        ),
    }
}

fn env_set(name: &str) -> bool {
    std::env::var_os(name).is_some_and(|v| !v.is_empty())
}

/// True if git has an explicit author and committer identity (no auto-detection).
fn user_identity_configured(cwd: &Path) -> Result<bool> {
    let cfg = |key: &str| -> Result<bool> {
        Ok(!gitutil::run(&["config", "--get", key], cwd, &[], false)?
            .stdout
            .is_empty())
    };
    for role in ["author", "committer"] {
        let up = role.to_uppercase();
        let name = env_set(&format!("GIT_{up}_NAME"))
            || cfg(&format!("{role}.name"))?
            || cfg("user.name")?;
        let email = env_set(&format!("GIT_{up}_EMAIL"))
            || cfg(&format!("{role}.email"))?
            || cfg("user.email")?
            || env_set("EMAIL");
        if !(name && email) {
            return Ok(false);
        }
    }
    Ok(true)
}

fn operation_in_progress(main: &Path) -> Result<Option<&'static str>> {
    let markers = [
        ("MERGE_HEAD", "a merge"),
        ("CHERRY_PICK_HEAD", "a cherry-pick"),
        ("REVERT_HEAD", "a revert"),
        ("rebase-merge", "a rebase"),
        ("rebase-apply", "a rebase or am"),
    ];
    for (name, what) in markers {
        let path = gitutil::out(
            &["rev-parse", "--path-format=absolute", "--git-path", name],
            main,
            &[],
        )?;
        if Path::new(&path).exists() {
            return Ok(Some(what));
        }
    }
    Ok(None)
}

fn commit_skip_reason(main: &Path) -> Result<Option<String>> {
    if let Some(op) = operation_in_progress(main)? {
        return Ok(Some(format!("the main worktree is in the middle of {op}")));
    }
    if !gitutil::out(&["ls-files", "-u", "--", FILENAME], main, &[])?.is_empty() {
        return Ok(Some(format!("{FILENAME} has merge conflicts")));
    }
    if !gitutil::ok(&["symbolic-ref", "--quiet", "HEAD"], main)? {
        return Ok(Some("the main worktree is on a detached HEAD".into()));
    }
    if gitutil::ok(&["check-ignore", "-q", "--", FILENAME], main)? {
        return Ok(Some(format!("{FILENAME} is ignored by git")));
    }
    if !user_identity_configured(main)? {
        return Ok(Some(
            "no git identity is configured (set user.name and user.email)".into(),
        ));
    }
    Ok(None)
}

/// Commit only CLAUDE.md in the main worktree; returns `(committed, hint)`.
pub fn commit_claudemd(main: &Path) -> Result<(bool, Option<String>)> {
    if let Some(reason) = commit_skip_reason(main)? {
        return Ok((
            false,
            Some(format!("{reason}; commit {FILENAME} yourself later")),
        ));
    }
    let has_head = gitutil::rev_parse("HEAD", main)?.is_some();
    let tracked = gitutil::ok(&["ls-files", "--error-unmatch", "--", FILENAME], main)?;
    if tracked && has_head && gitutil::ok(&["diff", "--quiet", "HEAD", "--", FILENAME], main)? {
        return Ok((false, None)); // already identical to HEAD's version
    }
    if !tracked {
        gitutil::run(&["add", "--", FILENAME], main, &[], true)?;
    }
    let proc = gitutil::run(
        &[
            "commit",
            "--only",
            "-q",
            "-m",
            COMMIT_MESSAGE,
            "--",
            FILENAME,
        ],
        main,
        &[],
        false,
    )?;
    if proc.status != 0 {
        if !tracked {
            gitutil::run(&["rm", "--cached", "-q", "--", FILENAME], main, &[], false)?;
        }
        let stdout = String::from_utf8_lossy(&proc.stdout).into_owned();
        let detail = if proc.stderr.is_empty() {
            stdout
        } else {
            proc.stderr.clone()
        };
        let last = crate::records::py_splitlines(py_strip(&detail))
            .last()
            .map(|l| l.to_string())
            .unwrap_or_else(|| "no output".into());
        return Ok((
            false,
            Some(format!(
                "git commit of {FILENAME} failed ({last}); commit it yourself"
            )),
        ));
    }
    Ok((true, None))
}

// ---------------------------------------------------------------- sync

/// What a sync did.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct SyncResult {
    pub path: PathBuf,
    /// `created` | `replaced` | `appended`; None = unchanged.
    pub action: Option<String>,
    pub committed: bool,
    pub hints: Vec<String>,
}

impl SyncResult {
    fn unchanged(path: PathBuf) -> Self {
        SyncResult {
            path,
            action: None,
            committed: false,
            hints: Vec::new(),
        }
    }

    /// Whether the file was written.
    pub fn changed(&self) -> bool {
        self.action.is_some()
    }

    /// What the CLI prints (Python `SyncResult.messages`).
    pub fn messages(&self) -> Vec<String> {
        let Some(action) = &self.action else {
            return vec![format!("{} already up to date", self.path.display())];
        };
        let mut out = vec![format!("{action} arboria block in {}", self.path.display())];
        if self.committed {
            out.push(format!("committed {FILENAME} in the main worktree"));
        }
        out.extend(self.hints.iter().map(|h| format!("hint: {h}")));
        out
    }
}

/// What a sync would write: the path, its old bytes (None: no file), the new bytes and
/// the action.
struct Plan {
    path: PathBuf,
    old: Option<Vec<u8>>,
    new: Vec<u8>,
    action: &'static str,
}

/// The [`Plan`] for `lessons`, without writing anything; errors on problems.
fn plan(ctx: &RepoContext, lessons: &[Lesson]) -> Result<Plan> {
    let path = ctx.main.join(FILENAME);
    let old = read_existing(&path)?;
    let (new, action) = apply_block(
        old.as_deref(),
        &render_block(lessons)?,
        &path.display().to_string(),
    )?;
    Ok(Plan {
        path,
        old,
        new,
        action,
    })
}

fn read_existing(path: &Path) -> Result<Option<Vec<u8>>> {
    match std::fs::read(path) {
        Ok(b) => Ok(Some(b)),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e.into()),
    }
}

/// Write `new` over `path` through a temp file, keeping the old file's mode.
fn replace_file(path: &Path, new: &[u8]) -> Result<()> {
    let tmp = path.with_file_name(format!(".{FILENAME}.arboria-tmp"));
    std::fs::write(&tmp, new)?;
    if let Ok(meta) = std::fs::metadata(path) {
        let mode = meta.permissions().mode() & 0o7777;
        std::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(mode))?;
    }
    std::fs::rename(&tmp, path)?;
    Ok(())
}

/// Refuse lessons that would not fit the block (markers, line cap, config).
pub fn check(ctx: &RepoContext, lessons: &[Lesson]) -> Result<()> {
    plan(ctx, lessons)?;
    autocommit_enabled(ctx)?;
    Ok(())
}

/// `arb claudemd sync [--no-commit]`: write the block into `<main>/CLAUDE.md` and
/// (optionally) commit just that file.
pub fn sync(ctx: &RepoContext, ledger: &Ledger, commit: bool) -> Result<SyncResult> {
    let Plan {
        path,
        old,
        new,
        action,
    } = plan(ctx, &accepted_lessons(ledger, None)?)?;
    if old.as_deref() == Some(new.as_slice()) {
        return Ok(SyncResult::unchanged(path));
    }
    let autocommit = autocommit_enabled(ctx)?;
    replace_file(&path, &new)?;
    let mut result = SyncResult::unchanged(path);
    result.action = Some(action.into());
    if !commit {
        return Ok(result);
    }
    if !autocommit {
        result.hints.push(format!(
            "[claudemd] autocommit is off; commit {FILENAME} yourself"
        ));
        return Ok(result);
    }
    let (committed, hint) = commit_claudemd(&ctx.main)?;
    result.committed = committed;
    result.hints.extend(hint);
    Ok(result)
}

// ---------------------------------------------------------------- the seed home

/// The seed home's block (no lessons: they are per project).
pub fn render_home_block() -> String {
    let mut lines = vec![BEGIN];
    lines.extend(HOME_RULES.lines());
    lines.push(END);
    lines.join("\n") + "\n"
}

/// The seed home's CLAUDE.md (`arb install-claude --home`; never committed).
pub fn sync_home(root: &Path) -> Result<SyncResult> {
    let path = root.join(FILENAME);
    let old = read_existing(&path)?;
    let (new, action) = apply_block(
        old.as_deref(),
        &render_home_block(),
        &path.display().to_string(),
    )?;
    if old.as_deref() == Some(new.as_slice()) {
        return Ok(SyncResult::unchanged(path));
    }
    replace_file(&path, &new)?;
    let mut result = SyncResult::unchanged(path);
    result.action = Some(action.into());
    Ok(result)
}
