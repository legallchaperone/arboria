//! Spike: can a Rust core drive an ipykernel (from a uv venv) over the Jupyter protocol?
//! start → execute → structured output → state → interrupt → error → 50 MB output → shutdown.
use anyhow::{bail, Context, Result};
use jupyter_protocol::{
    ConnectionInfo, ExecuteRequest, ExecutionState, InterruptRequest, JupyterMessage, JupyterMessageContent,
    KernelInfoRequest, ShutdownRequest, Transport,
};
use jupyter_zmq_client as zc;
use std::{net::IpAddr, path::PathBuf, time::{Duration, Instant}};

struct Kernel {
    shell: zc::ClientShellConnection,
    iopub: zc::ClientIoPubConnection,
    control: zc::ClientControlConnection,
    session: String,
    child: tokio::process::Child,
}

#[derive(Default, Debug)]
struct Outcome {
    plain: Vec<String>,       // execute_result text/plain
    custom: Vec<(String, usize)>, // (mime, bytes) of application/vnd.arboria.* outputs
    stream_bytes: usize,
    error: Option<String>,
    secs: f64,
}

impl Kernel {
    async fn start(dir: &PathBuf) -> Result<(Kernel, f64)> {
        let t0 = Instant::now();
        let ip: IpAddr = "127.0.0.1".parse()?;
        eprintln!("[peek]"); let p = zc::peek_ports(ip, 5).await?; eprintln!("[ports {p:?}]");
        let info = ConnectionInfo {
            ip: ip.to_string(), transport: Transport::TCP,
            shell_port: p[0], iopub_port: p[1], stdin_port: p[2], control_port: p[3], hb_port: p[4],
            key: uuid_like(), signature_scheme: "hmac-sha256".into(), kernel_name: Some("python3".into()),
        };
        let conn = dir.join("kernel.json");
        tokio::fs::write(&conn, serde_json::to_vec(&info)?).await?;
        let child = tokio::process::Command::new(dir.join(".venv/bin/python"))
            .args(["-m", "ipykernel_launcher", "-f"]).arg(&conn)
            .env("PYTHONPATH", dir.join("pylib"))
            .stdout(std::process::Stdio::null()).stderr(std::process::Stdio::null())
            .kill_on_drop(true).spawn().context("spawn ipykernel")?;
        // wait until the kernel listens on every port (zeromq-rs connect does not retry)
        for port in [info.shell_port, info.iopub_port, info.control_port] {
            while tokio::net::TcpStream::connect(("127.0.0.1", port)).await.is_err() {
                if t0.elapsed() > Duration::from_secs(60) { bail!("kernel did not open port {port}"); }
                tokio::time::sleep(Duration::from_millis(50)).await;
            }
        }
        eprintln!("[ports open after {:.2}s]", t0.elapsed().as_secs_f64());
        let session = uuid_like();
        let iopub = zc::create_client_iopub_connection(&info, "", &session).await?;
        #[allow(deprecated)]
        let shell = zc::create_client_shell_connection_with_identity(&info, &session, zc::peer_identity_for_session(&session)?).await?;
        let control = zc::create_client_control_connection(&info, &session).await?;
        eprintln!("[connected]");
        let mut k = Kernel { shell, iopub, control, session, child };
        // kernel_info until it answers (the kernel may still be booting)
        loop {
            let req = k.msg(KernelInfoRequest {});
            k.shell.send(req).await?;
            match tokio::time::timeout(Duration::from_secs(2), k.shell.read()).await {
                Ok(Ok(m)) if matches!(m.content, JupyterMessageContent::KernelInfoReply(_)) => break,
                _ if t0.elapsed() > Duration::from_secs(60) => bail!("kernel did not start"),
                _ => continue,
            }
        }
        // drain the startup status messages
        while let Ok(Ok(_)) = tokio::time::timeout(Duration::from_millis(300), k.iopub.read()).await {}
        Ok((k, t0.elapsed().as_secs_f64()))
    }

    fn msg(&self, c: impl Into<JupyterMessageContent>) -> JupyterMessage {
        let mut m = JupyterMessage::new(c, None);
        m.header.session = self.session.clone();
        m
    }

    /// Run code; collect iopub messages whose parent is this request until status: idle.
    /// `interrupt_after`: send interrupt_request on the control channel after this long.
    async fn run(&mut self, code: &str, interrupt_after: Option<Duration>) -> Result<Outcome> {
        let t0 = Instant::now();
        let req = self.msg(ExecuteRequest::new(code.to_string()));
        let id = req.header.msg_id.clone();
        self.shell.send(req).await?;
        let mut out = Outcome::default();
        let mut interrupted = false;
        loop {
            let wait = match (interrupt_after, interrupted) {
                (Some(d), false) => d.saturating_sub(t0.elapsed()).max(Duration::from_millis(1)),
                _ => Duration::from_secs(120),
            };
            let m = match tokio::time::timeout(wait, self.iopub.read()).await {
                Ok(m) => m?,
                Err(_) if interrupt_after.is_some() && !interrupted => {
                    interrupted = true;
                    let ir = self.msg(InterruptRequest {});
                    self.control.send(ir).await?;
                    continue;
                }
                Err(_) => bail!("timed out waiting for {code:?}"),
            };
            if m.parent_header.as_ref().map(|h| h.msg_id.as_str()) != Some(id.as_str()) { continue; }
            match m.content {
                JupyterMessageContent::StreamContent(s) => out.stream_bytes += s.text.len(),
                JupyterMessageContent::ExecuteResult(r) => {
                    for mt in r.data.content { if let jupyter_protocol::MediaType::Plain(s) = mt { out.plain.push(s) } }
                }
                JupyterMessageContent::DisplayData(dd) => {
                    for mt in dd.data.content {
                        if let jupyter_protocol::MediaType::Other((k, v)) = mt {
                            if k.starts_with("application/vnd.arboria.") { out.custom.push((k, v.to_string().len())) }
                        }
                    }
                }
                JupyterMessageContent::ErrorOutput(e) => out.error = Some(format!("{}: {}", e.ename, e.evalue)),
                JupyterMessageContent::Status(s) if matches!(s.execution_state, ExecutionState::Idle) => break,
                _ => {}
            }
        }
        // consume the execute_reply on shell
        let _ = tokio::time::timeout(Duration::from_secs(5), self.shell.read()).await;
        out.secs = t0.elapsed().as_secs_f64();
        Ok(out)
    }

    async fn shutdown(mut self) -> Result<f64> {
        let t0 = Instant::now();
        let m = self.msg(ShutdownRequest { restart: false });
        self.control.send(m).await?;
        let _ = tokio::time::timeout(Duration::from_secs(10), self.child.wait()).await;
        Ok(t0.elapsed().as_secs_f64())
    }
}

fn uuid_like() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    let n = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_nanos();
    format!("{:032x}", n ^ 0x9e3779b97f4a7c15f39cc0605cedc834u128)
}

fn rss_mb() -> f64 {
    std::fs::read_to_string("/proc/self/status").ok()
        .and_then(|s| s.lines().find(|l| l.starts_with("VmRSS:")).map(|l| l.split_whitespace().nth(1).unwrap().parse::<f64>().unwrap() / 1024.0))
        .unwrap_or(0.0)
}

#[tokio::main]
async fn main() -> Result<()> {
    let dir = std::env::current_dir()?;
    let (mut k, boot) = Kernel::start(&dir).await?;
    println!("启动 kernel（到能回 kernel_info）: {boot:.2} 秒");
    if let Ok(n) = std::env::var("ONLY_SIZE") {
        let o = k.run(&format!("print('x' * {n})"), None).await?;
        println!("SIZE {n} ok {:.2}s", o.secs);
        return Ok(());
    }

    let o = k.run("1 + 1", None).await?;
    println!("1 + 1 → {:?}，{:.3} 秒", o.plain, o.secs);

    let o = k.run("import lab; lab.runs()", None).await?;
    println!("lab.runs() → 结构化输出 {:?}，{:.3} 秒", o.custom, o.secs);

    k.run("x = 41", None).await?;
    let o = k.run("x + 1", None).await?;
    println!("变量留在 kernel 里：x + 1 → {:?}", o.plain);

    let o = k.run("1 / 0", None).await?;
    println!("出错 → {:?}", o.error);

    let o = k.run("import time\nfor i in range(600): time.sleep(0.05)", Some(Duration::from_millis(800))).await?;
    println!("中断（0.8 秒后发 interrupt_request）→ {:?}，从开始到 idle {:.2} 秒", o.error, o.secs);
    let o = k.run("x", None).await?;
    println!("中断后 kernel 还活着，状态还在：x → {:?}", o.plain);

    for n in [100_000usize, 1_000_000, 4_000_000, 16_000_000] {
        let o = tokio::time::timeout(Duration::from_secs(60), k.run(&format!("print('x' * {n})"), None)).await;
        match o { Ok(Ok(o)) => println!("文本输出 {:.1} MB → 收到 {:.1} MB，{:.2} 秒", n as f64/1e6, o.stream_bytes as f64/1e6, o.secs),
                  _ => { println!("文本输出 {:.1} MB → 60 秒内没完成", n as f64/1e6); break } }
    }
    let before = rss_mb();
    let o = k.run("print('x' * 50_000_000)", None).await?;
    println!("50 MB 文本输出 → 收到 {:.1} MB，{:.2} 秒，驱动进程内存 {:.0} → {:.0} MB", o.stream_bytes as f64 / 1e6, o.secs, before, rss_mb());

    let o = k.run("lab.plot('r011', 'r012', 'r013')", None).await?;
    println!("大图表（3 条 × 20 万点的结构化输出）→ {:?}，{:.2} 秒，驱动内存 {:.0} MB", o.custom.iter().map(|(m, b)| (m.as_str(), format!("{:.1} MB", *b as f64 / 1e6))).collect::<Vec<_>>(), o.secs, rss_mb());

    let mut lat = vec![];
    for _ in 0..50 { lat.push(k.run("lab.runs()", None).await?.secs * 1000.0); }
    lat.sort_by(|a, b| a.partial_cmp(b).unwrap());
    println!("lab.runs() 往返 50 次：中位数 {:.1} ms，p95 {:.1} ms", lat[25], lat[47]);

    let s = k.shutdown().await?;
    println!("关闭 kernel：{s:.2} 秒");
    Ok(())
}
