//! The app's own notebook (ADR 0005): load and save the `.md` file, run its command
//! blocks in a kernel per notebook, stream what the kernel says to the window.
//!
//! - The file is the human's: the window sends the whole text, it is parsed, given ids
//!   where missing and saved canonically (temp file + rename).
//! - One kernel per notebook, started on the first run, kept while the app runs; a run
//!   holds that kernel, so blocks of one notebook run one after another. Interrupting
//!   does not wait for the lock (SIGINT through an `Interrupter`).
//! - Streams and outputs go to the window as `nb-event` while a block runs; the result
//!   is stored in the outputs cache (`nbout`) and returned.
//! - Nothing here approves (`lab.run` only makes a card; signing is the card panel).

use std::collections::{BTreeMap, HashMap};
use std::path::Path;
use std::sync::{Arc, Mutex};

use arboria_core::nbdoc::{self, Doc};
use arboria_core::nbkernel::{self, Event, Interrupter, Kernel, Notebook};
use arboria_core::nbout::{self, Cached};
use serde::Serialize;
use serde_json::Value;
use tauri::{AppHandle, Emitter};

fn err(e: impl std::fmt::Display) -> String {
    e.to_string()
}

type Slot = Arc<Mutex<Option<Kernel>>>;

#[derive(Default)]
pub struct Kernels {
    slots: Mutex<HashMap<String, Slot>>,
    interrupters: Mutex<HashMap<String, Interrupter>>,
    info: Mutex<HashMap<String, Value>>,
}

fn key(root: &str, idea: &str) -> String {
    format!("{root}\u{0}{idea}")
}

impl Kernels {
    fn slot(&self, k: &str) -> Result<Slot, String> {
        Ok(self
            .slots
            .lock()
            .map_err(err)?
            .entry(k.to_string())
            .or_default()
            .clone())
    }
}

/// What the window needs to show a notebook.
#[derive(Debug, Serialize)]
pub struct NbView {
    pub notebook: Notebook,
    pub exists: bool,
    /// The file's text (canonical once saved by the app).
    pub text: String,
    pub doc: Doc,
    /// Each block's last result, by block id.
    pub outputs: BTreeMap<String, Cached>,
    /// The kernel's `ready` event when one runs (python version, whether `lab` imported).
    pub kernel: Option<Value>,
}

pub fn load(ks: &Kernels, root: &Path, idea: &str) -> Result<NbView, String> {
    let nb = Notebook::locate(root, idea).map_err(err)?;
    let kernel = ks
        .info
        .lock()
        .map_err(err)?
        .get(&key(&root.display().to_string(), idea))
        .cloned();
    if !nb.exists() {
        return Ok(NbView {
            exists: false,
            text: String::new(),
            doc: Doc::default(),
            outputs: BTreeMap::new(),
            kernel,
            notebook: nb,
        });
    }
    let (doc, text) = nb.load().map_err(err)?;
    let outputs = nbout::load(&nb.outputs);
    Ok(NbView {
        exists: true,
        text,
        doc,
        outputs,
        kernel,
        notebook: nb,
    })
}

/// Save the window's text; the canonical text and parsed doc come back.
pub fn save(root: &Path, idea: &str, text: &str) -> Result<(String, Doc), String> {
    let nb = Notebook::locate(root, idea).map_err(err)?;
    if !nb.exists() {
        return Err(format!("{} 没有 notebook", nb.name));
    }
    let mut doc = nbdoc::parse(text);
    // the frontmatter is kept from the file when the window sent none
    if doc.front.is_empty() {
        if let Ok((old, _)) = nb.load() {
            doc.front = old.front;
        }
    }
    nbdoc::ensure_ids(&mut doc);
    let saved = nbdoc::save(&nb.path, &doc).map_err(err)?;
    Ok((saved, doc))
}

#[derive(Debug, Clone, Serialize)]
struct NbEvent<'a> {
    root: &'a str,
    idea: &'a str,
    block: &'a str,
    #[serde(flatten)]
    event: &'a Event,
}

/// Run one block (its code as the window has it) and store the result.
pub fn run(
    app: &AppHandle,
    ks: &Kernels,
    root: &Path,
    idea: &str,
    block: &str,
    code: &str,
) -> Result<Cached, String> {
    if !nbdoc::valid_id(block) {
        return Err(format!("not a block id: {block}"));
    }
    let nb = Notebook::locate(root, idea).map_err(err)?;
    let root_s = root.display().to_string();
    let k = key(&root_s, idea);
    let slot = ks.slot(&k)?;
    let mut guard = slot.lock().map_err(err)?;
    let alive = guard.as_mut().is_some_and(Kernel::alive);
    if !alive {
        let kernel = Kernel::start(&nb.cwd).map_err(err)?;
        ks.interrupters
            .lock()
            .map_err(err)?
            .insert(k.clone(), kernel.interrupter());
        let mut info = kernel.ready.clone();
        info["pid"] = kernel.pid().into();
        ks.info.lock().map_err(err)?.insert(k.clone(), info.clone());
        let _ = app.emit("nb-kernel", (&root_s, idea, &info));
        *guard = Some(kernel);
    }
    let kernel = guard.as_mut().expect("started above");
    let b = nbdoc::Block {
        id: block.to_string(),
        code: code.to_string(),
    };
    let res = nbkernel::run_and_store(kernel, &nb, &b, &mut |e| {
        let _ = app.emit(
            "nb-event",
            NbEvent {
                root: &root_s,
                idea,
                block,
                event: e,
            },
        );
    });
    if res.is_err() && !kernel.alive() {
        *guard = None;
        ks.info.lock().map_err(err)?.remove(&k);
        let _ = app.emit("nb-kernel", (&root_s, idea, Value::Null));
    }
    res.map_err(err)
}

pub fn interrupt(ks: &Kernels, root: &Path, idea: &str) -> Result<(), String> {
    let k = key(&root.display().to_string(), idea);
    let i = ks.interrupters.lock().map_err(err)?.get(&k).cloned();
    match i {
        Some(i) => i.interrupt().map_err(err),
        None => Err("kernel 没有在跑".into()),
    }
}

/// Stop the notebook's kernel (the next run starts a fresh one).
pub fn restart(app: &AppHandle, ks: &Kernels, root: &Path, idea: &str) -> Result<(), String> {
    let root_s = root.display().to_string();
    let k = key(&root_s, idea);
    if let Some(i) = ks.interrupters.lock().map_err(err)?.remove(&k) {
        let _ = i.interrupt();
    }
    let slot = ks.slot(&k)?;
    // a running block ends with "kernel 退出了" once the process is killed
    if let Ok(mut g) = slot.try_lock() {
        if let Some(kernel) = g.take() {
            kernel.shutdown();
        }
    } else {
        let pid = ks
            .info
            .lock()
            .map_err(err)?
            .get(&k)
            .and_then(|v| v["pid"].as_u64());
        if let Some(pid) = pid {
            let _ = std::process::Command::new("kill")
                .arg(pid.to_string())
                .status();
        }
    }
    ks.info.lock().map_err(err)?.remove(&k);
    let _ = app.emit("nb-kernel", (&root_s, idea, Value::Null));
    Ok(())
}
