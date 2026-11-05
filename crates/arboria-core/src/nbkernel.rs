//! Driving the notebook kernel (`python -m arboria.kernel`, ADR 0005) and knowing where a
//! notebook lives.
//!
//! - One process per notebook, the human's Python ([`crate::commands::python_exe`]),
//!   working directory the idea's worktree (a seed: the seed home's `seeds/`).
//! - JSON lines over the child's stdin/stdout; a reader thread turns them into
//!   [`Event`]s. A dead kernel is an error for the block that was running, never a hang.
//! - Interrupting is SIGINT to the process ([`Interrupter`], usable from another thread
//!   while a block runs).
//! - Nothing here approves or writes the ledger: blocks are the human's code, run in the
//!   human's kernel, with the same checks as anywhere else (`lab` → `arb lab`).

use std::io::{BufRead as _, BufReader, Write as _};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde::Serialize;
use serde_json::{json, Value};

use crate::db::Ledger;
use crate::model::{IdeaName, IdeaStage};
use crate::nbdoc::{self, Doc};
use crate::nbout::{self, Cached};
use crate::paths::{self, Home, RepoContext};
use crate::{bail, Error, Result};

/// How long a kernel may take to say it is ready.
pub const READY_TIMEOUT: Duration = Duration::from_secs(60);

/// Where one notebook lives.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Notebook {
    /// `idea` or `seed`.
    pub owner: String,
    pub name: String,
    /// The `.md` file.
    pub path: PathBuf,
    /// The marimo `.py` next to it, when there is one (not migrated yet, or kept).
    pub marimo: Option<PathBuf>,
    /// Where the kernel runs.
    pub cwd: PathBuf,
    /// The outputs cache folder.
    pub outputs: PathBuf,
    /// The state dir whose ledger the notebook belongs to.
    pub state_dir: PathBuf,
}

impl Notebook {
    /// An idea's notebook (its worktree while open, else the main worktree).
    pub fn of_idea(ctx: &RepoContext, ledger: &Ledger, name: &IdeaName) -> Result<Notebook> {
        let idea = crate::ideas::get_idea(ledger, name)?;
        let state = ctx.state_dir();
        let cwd = idea
            .worktree_path
            .as_deref()
            .map(PathBuf::from)
            .filter(|w| idea.stage == IdeaStage::Open && w.is_dir())
            .unwrap_or_else(|| ctx.main.clone());
        let py = crate::ideas::notebook_path(&state, name.as_str());
        Ok(Notebook {
            owner: "idea".into(),
            name: name.as_str().into(),
            path: nbdoc::idea_path(&state, name.as_str()),
            marimo: py.is_file().then_some(py),
            cwd,
            outputs: nbout::dir(&state, name.as_str()),
            state_dir: state,
        })
    }

    /// A seed's notebook in the seed home.
    pub fn of_seed(home: &Home, name: &IdeaName) -> Notebook {
        let state = home.state_dir();
        let py = home.notebook(name.as_str());
        Notebook {
            owner: "seed".into(),
            name: name.as_str().into(),
            path: nbdoc::seed_path(&home.seeds_dir(), name.as_str()),
            marimo: py.is_file().then_some(py),
            cwd: home.seeds_dir(),
            outputs: nbout::dir(&state, &format!("seed-{}", name.as_str())),
            state_dir: state,
        }
    }

    /// The notebook of `name` in the project (or seed home) at `root`.
    pub fn locate(root: &Path, name: &str) -> Result<Notebook> {
        let name = IdeaName::parse(name)?;
        let home = paths::home();
        if paths::resolve(root) == paths::resolve(&home.root) {
            return Ok(Notebook::of_seed(&home, &name));
        }
        let ctx = paths::locate(Some(root))?.require_initialized()?;
        let ledger = Ledger::open(&ctx.state_dir())?;
        Notebook::of_idea(&ctx, &ledger, &name)
    }

    pub fn exists(&self) -> bool {
        self.path.is_file()
    }

    /// Read and parse the file; blocks without an id get one (saved back).
    pub fn load(&self) -> Result<(Doc, String)> {
        let text = std::fs::read_to_string(&self.path)
            .map_err(|e| Error::Arb(format!("cannot read {}: {e}", self.path.display())))?;
        let mut doc = nbdoc::parse(&text);
        if nbdoc::ensure_ids(&mut doc) {
            let saved = nbdoc::save(&self.path, &doc)?;
            return Ok((doc, saved));
        }
        Ok((doc, text))
    }
}

/// One thing the kernel said while a block ran.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(tag = "event", rename_all = "lowercase")]
pub enum Event {
    Stream { name: String, text: String },
    Output { data: Value },
}

/// How a block ended.
#[derive(Debug, Clone, PartialEq, Serialize, Default)]
pub struct Done {
    pub ok: bool,
    pub defines: Vec<String>,
    pub uses: Vec<String>,
    pub duration_ms: u64,
}

/// Sends SIGINT to a kernel; cheap to clone, usable while another thread waits on a block.
#[derive(Debug, Clone)]
pub struct Interrupter {
    pid: u32,
    alive: Arc<Mutex<bool>>,
}

impl Interrupter {
    pub fn interrupt(&self) -> Result<()> {
        if !*self.alive.lock().map_err(|e| Error::Arb(e.to_string()))? {
            bail!("kernel 已经停了");
        }
        let ok = Command::new("kill")
            .args(["-INT", &self.pid.to_string()])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()?
            .success();
        if !ok {
            bail!("没能中断 kernel（pid {}）", self.pid);
        }
        Ok(())
    }
}

/// A running kernel.
pub struct Kernel {
    child: Child,
    stdin: ChildStdin,
    events: Receiver<Value>,
    next_id: u64,
    /// The `ready` event (python version, whether `lab` imported).
    pub ready: Value,
    stderr: Arc<Mutex<String>>,
    alive: Arc<Mutex<bool>>,
}

fn tail(s: &str, n: usize) -> String {
    let lines: Vec<&str> = s.trim_end().lines().collect();
    lines[lines.len().saturating_sub(n)..].join("\n")
}

impl Kernel {
    /// Start a kernel in `cwd` with the human's Python.
    pub fn start(cwd: &Path) -> Result<Kernel> {
        Kernel::start_with(&crate::commands::python_exe(), cwd, &[])
    }

    pub fn start_with(python: &str, cwd: &Path, env: &[(&str, &str)]) -> Result<Kernel> {
        let mut cmd = Command::new(python);
        cmd.args(["-m", "arboria.kernel"])
            .current_dir(cwd)
            .env("PYTHONUNBUFFERED", "1")
            .env("MPLBACKEND", "Agg")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        for (k, v) in env {
            cmd.env(k, v);
        }
        let mut child = cmd.spawn().map_err(|e| {
            Error::Arb(format!(
                "cannot start the notebook kernel with {python} ({e}); set ARBORIA_PYTHON"
            ))
        })?;
        let stdin = child.stdin.take().expect("piped stdin");
        let stdout = child.stdout.take().expect("piped stdout");
        let stderr_pipe = child.stderr.take().expect("piped stderr");
        let (tx, rx) = mpsc::channel();
        std::thread::spawn(move || {
            for line in BufReader::new(stdout).lines() {
                let Ok(line) = line else { break };
                if let Ok(v) = serde_json::from_str::<Value>(&line) {
                    if tx.send(v).is_err() {
                        break;
                    }
                }
            }
        });
        let stderr = Arc::new(Mutex::new(String::new()));
        let log = stderr.clone();
        std::thread::spawn(move || {
            for line in BufReader::new(stderr_pipe).lines() {
                let Ok(line) = line else { break };
                if let Ok(mut s) = log.lock() {
                    if s.len() > 200_000 {
                        let cut = s.len() - 100_000;
                        let cut = (cut..s.len()).find(|&i| s.is_char_boundary(i)).unwrap_or(0);
                        s.drain(..cut);
                    }
                    s.push_str(&line);
                    s.push('\n');
                }
            }
        });
        let alive = Arc::new(Mutex::new(true));
        let mut k = Kernel {
            child,
            stdin,
            events: rx,
            next_id: 0,
            ready: Value::Null,
            stderr,
            alive,
        };
        match k.events.recv_timeout(READY_TIMEOUT) {
            Ok(v) if v["event"] == "ready" => {
                k.ready = v;
                Ok(k)
            }
            Ok(v) => bail!("the kernel did not start: {v}"),
            Err(_) => {
                let _ = k.child.kill();
                let _ = k.child.wait();
                bail!(
                    "the notebook kernel did not start ({python} -m arboria.kernel): {}",
                    tail(&k.stderr_text(), 3)
                )
            }
        }
    }

    pub fn pid(&self) -> u32 {
        self.child.id()
    }

    pub fn interrupter(&self) -> Interrupter {
        Interrupter {
            pid: self.child.id(),
            alive: self.alive.clone(),
        }
    }

    /// What the kernel wrote to stderr (stray fd-1 writes, warnings), last ~100 KB.
    pub fn stderr_text(&self) -> String {
        self.stderr.lock().map(|s| s.clone()).unwrap_or_default()
    }

    pub fn alive(&mut self) -> bool {
        let up = matches!(self.child.try_wait(), Ok(None));
        if !up {
            if let Ok(mut a) = self.alive.lock() {
                *a = false;
            }
        }
        up
    }

    /// Run one block; `on_event` sees streams and outputs as they come.
    pub fn exec(
        &mut self,
        block: &str,
        code: &str,
        file: Option<&Path>,
        on_event: &mut dyn FnMut(&Event),
    ) -> Result<Done> {
        if !self.alive() {
            bail!("kernel 已经停了：重启 kernel");
        }
        self.next_id += 1;
        let id = self.next_id;
        let req = json!({"id": id, "op": "exec", "block": block, "code": code,
                         "file": file.map(|f| f.display().to_string())});
        writeln!(self.stdin, "{req}")
            .and_then(|_| self.stdin.flush())
            .map_err(|e| Error::Arb(format!("kernel 不接受请求了（{e}）")))?;
        loop {
            let ev = match self.events.recv_timeout(Duration::from_millis(500)) {
                Ok(ev) => ev,
                Err(RecvTimeoutError::Timeout) => {
                    if self.alive() {
                        continue;
                    }
                    // drain what it said before dying
                    match self.events.try_recv() {
                        Ok(ev) => ev,
                        Err(_) => bail!("kernel 退出了：{}", tail(&self.stderr_text(), 3)),
                    }
                }
                Err(RecvTimeoutError::Disconnected) => {
                    let _ = self.alive();
                    bail!("kernel 退出了：{}", tail(&self.stderr_text(), 3))
                }
            };
            if ev["id"].as_u64() != Some(id) {
                continue;
            }
            match ev["event"].as_str() {
                Some("stream") => on_event(&Event::Stream {
                    name: ev["name"].as_str().unwrap_or("stdout").to_string(),
                    text: ev["text"].as_str().unwrap_or("").to_string(),
                }),
                Some("output") => on_event(&Event::Output {
                    data: ev["data"].clone(),
                }),
                Some("done") => {
                    let strings = |k: &str| -> Vec<String> {
                        ev[k]
                            .as_array()
                            .map(|a| {
                                a.iter()
                                    .filter_map(|x| x.as_str().map(String::from))
                                    .collect()
                            })
                            .unwrap_or_default()
                    };
                    return Ok(Done {
                        ok: ev["ok"].as_bool().unwrap_or(false),
                        defines: strings("defines"),
                        uses: strings("uses"),
                        duration_ms: ev["duration_ms"].as_u64().unwrap_or(0),
                    });
                }
                _ => {}
            }
        }
    }

    /// Stop the kernel.
    pub fn shutdown(mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
        if let Ok(mut a) = self.alive.lock() {
            *a = false;
        }
    }
}

impl Drop for Kernel {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// Merge streamed text into outputs the way the notebook shows them: consecutive
/// `stdout` (or `stderr`) chunks become one `text` output (stderr as `stderr: true`).
pub fn collect_outputs(events: &[Event]) -> Vec<Value> {
    let mut out: Vec<Value> = Vec::new();
    for e in events {
        match e {
            Event::Stream { name, text } => {
                let err = name == "stderr";
                if let Some(last) = out.last_mut() {
                    if last["kind"] == "stream" && last["stderr"].as_bool() == Some(err) {
                        let joined = format!("{}{}", last["text"].as_str().unwrap_or(""), text);
                        last["text"] = joined.into();
                        continue;
                    }
                }
                out.push(json!({"kind": "stream", "stderr": err, "text": text}));
            }
            Event::Output { data } => out.push(data.clone()),
        }
    }
    out
}

/// Run one block of a notebook and store its result.
pub fn run_and_store(
    kernel: &mut Kernel,
    nb: &Notebook,
    block: &nbdoc::Block,
    on_event: &mut dyn FnMut(&Event),
) -> Result<Cached> {
    let mut events = Vec::new();
    let done = kernel.exec(&block.id, &block.code, Some(&nb.path), &mut |e| {
        on_event(e);
        events.push(e.clone());
    })?;
    let cached = Cached {
        code_sha: nbout::code_sha(&block.code),
        ran_at: crate::db::now(),
        duration_ms: done.duration_ms,
        ok: done.ok,
        outputs: collect_outputs(&events),
        defines: done.defines,
        uses: done.uses,
    };
    nbout::store(&nb.outputs, &block.id, &cached)?;
    Ok(cached)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn python() -> Option<String> {
        let p = std::env::var("ARBORIA_PYTHON").ok()?;
        Path::new(&p).exists().then_some(p)
    }

    fn src_env() -> String {
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../src")
            .canonicalize()
            .unwrap()
            .display()
            .to_string()
    }

    /// The real kernel (`ARBORIA_PYTHON` set to a Python that can import arboria's deps).
    #[test]
    fn a_real_kernel_runs_blocks_and_survives_interrupts() {
        let Some(py) = python() else {
            eprintln!("skipped: set ARBORIA_PYTHON");
            return;
        };
        let src = src_env();
        let dir = std::env::temp_dir().join(format!("nbk-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let mut k = Kernel::start_with(&py, &dir, &[("PYTHONPATH", &src)]).unwrap();
        assert_eq!(k.ready["event"], "ready");
        let mut seen = Vec::new();
        let done = k
            .exec(
                "ab12",
                "x = 20\nprint('a')\nprint('b')\nx + 1",
                None,
                &mut |e| seen.push(e.clone()),
            )
            .unwrap();
        assert!(done.ok && done.defines == ["x"]);
        let outs = collect_outputs(&seen);
        assert_eq!(
            outs[0],
            json!({"kind": "stream", "stderr": false, "text": "a\nb\n"})
        );
        assert_eq!(outs[1], json!({"kind": "text", "text": "21"}));
        // a big print does not hang the pipe (the zeromq problem of phase 0)
        let mut n = 0usize;
        let done = k
            .exec("cd34", "print('y' * 5_000_000)", None, &mut |e| {
                if let Event::Stream { text, .. } = e {
                    n += text.len();
                }
            })
            .unwrap();
        assert!(done.ok && n > 100_000);
        // interrupt from another thread
        let int = k.interrupter();
        let t = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(800));
            int.interrupt().unwrap();
        });
        let mut outs = Vec::new();
        let done = k
            .exec("ef56", "import time\ntime.sleep(30)", None, &mut |e| {
                outs.push(e.clone())
            })
            .unwrap();
        t.join().unwrap();
        assert!(!done.ok);
        assert_eq!(collect_outputs(&outs)[0]["ename"], "KeyboardInterrupt");
        let mut outs = Vec::new();
        k.exec("gh78", "x", None, &mut |e| outs.push(e.clone()))
            .unwrap();
        assert_eq!(
            collect_outputs(&outs),
            [json!({"kind": "text", "text": "20"})]
        );
        // a kernel that dies mid-block is an error, not a hang
        let err = k
            .exec("ij90", "import os\nos._exit(3)", None, &mut |_| {})
            .unwrap_err();
        assert!(err.to_string().contains("kernel 退出了"), "{err}");
        assert!(k.exec("kl12", "1", None, &mut |_| {}).is_err());
        k.shutdown();
        let _ = std::fs::remove_dir_all(&dir);
    }
}
