// 设置: 通用 (background checks, notifications, where clones go), 机器 (clusters.toml as
// forms), 项目 (a project's arboria.toml as a form), 高级 (the clusters.toml text). Every
// save is checked by the core as arb reads the file; nothing is written on an error.

import { useEffect, useState, type ReactNode } from "react";
import { api, message } from "../api";
import type { AppSettings, MachineForm, MachinesPage, ProjectSettings, ProjectSummary } from "../types";
import { ago } from "../lib/format";
import { Glyph } from "../components/Glyph";
import { Machines as RawEditor } from "./RawClusters";
import type { SettingsTab } from "./route";

const ROLES: [string, string][] = [
  ["runs", "代码和日志"],
  ["data", "数据"],
  ["cache", "缓存"],
  ["ckpt", "checkpoint"],
  ["out", "输出（拉回本机）"],
];

function Switch(props: { on: boolean; label: string; onChange: (v: boolean) => void; disabled?: boolean }) {
  return (
    <button
      type="button"
      role="switch"
      aria-checked={props.on}
      aria-label={props.label}
      className="switch"
      disabled={props.disabled}
      onClick={() => props.onChange(!props.on)}
    />
  );
}

function Field(props: { label: string; hint?: string; children: ReactNode }) {
  return (
    <div className="field">
      <label>{props.label}</label>
      <div>{props.children}</div>
      {props.hint && <div className="hint">{props.hint}</div>}
    </div>
  );
}

const lines = (xs: string[]) => xs.join("\n");
const unlines = (s: string) => s.split("\n").map((x) => x.trimEnd()).filter((x) => x.trim() !== "");

function General(props: { now: number; toast: (m: string) => void }) {
  const [s, setS] = useState<AppSettings | null>(null);
  const [busy, setBusy] = useState(false);
  const load = () => api.appSettings().then(setS, (e) => props.toast(message(e)));
  useEffect(() => void load(), []);
  if (!s) return <p className="small muted">读取中…</p>;
  const toggleHelper = async (on: boolean) => {
    setBusy(true);
    try {
      if (on) await api.helperInstall();
      else await api.helperUninstall();
      await load();
    } catch (e) {
      props.toast(message(e));
    } finally {
      setBusy(false);
    }
  };
  return (
    <div className="form">
      <div className="toggle-row">
        <div className="txt">
          <span>在后台检查作业</span>
          <span className="d">
            app 关着也会定时查集群上的作业状态，记进账本。
            {s.helper.installed && s.helper.last_tick && ` 上次检查 ${ago(s.helper.last_tick, props.now)}。`}
          </span>
        </div>
        <Switch on={s.helper.installed} label="在后台检查作业" disabled={busy} onChange={(v) => void toggleHelper(v)} />
      </div>
      <div className="toggle-row">
        <div className="txt">
          <span>系统通知</span>
          <span className="d">run 跑完、失败或等你签字时提醒你。</span>
        </div>
        <Switch
          on={s.notifications}
          label="系统通知"
          onChange={(v) => api.setNotifications(v).then((n) => setS({ ...s, notifications: n }), (e) => props.toast(message(e)))}
        />
      </div>
      <div className="toggle-row">
        <div className="txt">
          <span>克隆仓库放在</span>
          <span className="d mono">{s.clone_dir}</span>
        </div>
        <button
          type="button"
          className="ab-btn"
          onClick={async () => {
            try {
              const p = await api.pickFolder();
              if (p) setS({ ...s, clone_dir: await api.cloneDir(p) });
            } catch (e) {
              props.toast(message(e));
            }
          }}
        >
          换一个…
        </button>
      </div>
    </div>
  );
}

function MachineEditor(props: {
  original: string | null;
  initial: MachineForm;
  error: string | null;
  onSaved: (p: MachinesPage, name: string) => void;
  onRemoved: (p: MachinesPage) => void;
  onProbe: (cmd: string) => void;
}) {
  const [f, setF] = useState<MachineForm>(props.initial);
  const [err, setErr] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  useEffect(() => {
    setF(props.initial);
    setErr(null);
  }, [props.original, props.initial]);
  const set = <K extends keyof MachineForm>(k: K, v: MachineForm[K]) => setF({ ...f, [k]: v });
  const save = async () => {
    setBusy(true);
    setErr(null);
    try {
      props.onSaved(await api.saveMachine(props.original, f), f.name);
    } catch (e) {
      setErr(message(e));
    } finally {
      setBusy(false);
    }
  };
  const fake = f.kind === "fake";
  return (
    <div className="form">
      {props.error && !err && <div className="err-box">arb 现在会拒绝这台机器：{props.error.replace(/^clusters\.toml: \[[^\]]*\]:? ?/, "")}</div>}
      <fieldset className="fieldset">
        <legend>连接</legend>
        <Field label="名字" hint="arb submit --machine 用的名字">
          <input className="input code" value={f.name} onChange={(e) => set("name", e.target.value)} />
        </Field>
        <Field label="类型">
          <select className="select" value={f.kind} onChange={(e) => set("kind", e.target.value)}>
            <option value="slurm">Slurm 集群</option>
            <option value="fake">本机模拟（测试用）</option>
          </select>
        </Field>
        {fake ? (
          <Field label="模拟状态目录" hint="本机上的一个文件夹">
            <input className="input code" value={f.root ?? ""} onChange={(e) => set("root", e.target.value)} />
          </Field>
        ) : (
          <Field label="登录节点" hint="~/.ssh/config 里的别名">
            <input className="input code" value={f.host} onChange={(e) => set("host", e.target.value)} />
          </Field>
        )}
        <Field label="账户" hint="sbatch --account">
          <input className="input code" value={f.account} onChange={(e) => set("account", e.target.value)} />
        </Field>
      </fieldset>
      <fieldset className="fieldset">
        <legend>每个节点</legend>
        <Field label="GPU 数">
          <input className="input is-small" type="number" min={1} value={f.gpus_per_node} onChange={(e) => set("gpus_per_node", Number(e.target.value))} />
        </Field>
        <Field label="每 GPU 的 CPU">
          <input className="input is-small" type="number" min={1} value={f.cpus_per_gpu} onChange={(e) => set("cpus_per_gpu", Number(e.target.value))} />
        </Field>
        <Field label="每 GPU 的内存（GB）">
          <input className="input is-small" type="number" min={0} step="any" value={f.mem_per_gpu_gb} onChange={(e) => set("mem_per_gpu_gb", Number(e.target.value))} />
        </Field>
      </fieldset>
      <fieldset className="fieldset">
        <legend>分区</legend>
        <table className="mini-table">
          <thead>
            <tr>
              <th>名字</th>
              <th>最长时长</th>
              <th>计费倍数</th>
              <th>默认</th>
              <th />
            </tr>
          </thead>
          <tbody>
            {f.partitions.map((p, i) => (
              <tr key={i}>
                <td>
                  <input className="input code" value={p.name} onChange={(e) => set("partitions", f.partitions.map((x, j) => (j === i ? { ...x, name: e.target.value } : x)))} />
                </td>
                <td>
                  <input className="input code is-small" value={p.max_walltime} placeholder="48h" onChange={(e) => set("partitions", f.partitions.map((x, j) => (j === i ? { ...x, max_walltime: e.target.value } : x)))} />
                </td>
                <td>
                  <input className="input is-small" type="number" min={0} step="any" value={p.charge} onChange={(e) => set("partitions", f.partitions.map((x, j) => (j === i ? { ...x, charge: Number(e.target.value) } : x)))} />
                </td>
                <td>
                  <input type="radio" name="default-partition" aria-label={`默认分区 ${p.name}`} checked={f.default_partition === p.name} onChange={() => set("default_partition", p.name)} />
                </td>
                <td>
                  <button type="button" className="icon-btn" aria-label="删除这个分区" onClick={() => set("partitions", f.partitions.filter((_, j) => j !== i))}>
                    <Glyph name="cross" />
                  </button>
                </td>
              </tr>
            ))}
          </tbody>
        </table>
        <div>
          <button type="button" className="ab-btn small" onClick={() => set("partitions", [...f.partitions, { name: "", max_walltime: "24h", charge: 1 }])}>
            <Glyph name="plus" /> 加一个分区
          </button>
        </div>
      </fieldset>
      <fieldset className="fieldset">
        <legend>集群上的路径</legend>
        {ROLES.map(([role, label]) => (
          <Field key={role} label={label} hint={role === "runs" ? "可以写 $USER；arb 只在这些目录下面读写" : undefined}>
            <input className="input code" value={f.paths[role] ?? ""} onChange={(e) => set("paths", { ...f.paths, [role]: e.target.value })} />
          </Field>
        ))}
      </fieldset>
      <details className="full">
        <summary>更多设置</summary>
        <div className="form" style={{ marginTop: 12 }}>
          <Field label="作业环境" hint="作业开始时依次运行，每行一条（比如 module load、source venv）">
            <textarea className="textarea code" value={lines(f.env_activate)} onChange={(e) => set("env_activate", unlines(e.target.value))} />
          </Field>
          <Field label="sbatch 前置行" hint="写进作业脚本开头，每行一条">
            <textarea className="textarea code" value={lines(f.prelude)} onChange={(e) => set("prelude", unlines(e.target.value))} />
          </Field>
          <Field label="最多分几段" hint="超过分区最长时长时自动续跑的段数">
            <input className="input is-small" type="number" min={1} value={f.max_segments} onChange={(e) => set("max_segments", Number(e.target.value))} />
          </Field>
          <Field label="每 SU 的计费单位" hint="sacct billing ÷ 这个数 = SU/小时；0 = 本机估算（DeltaAI 是 1000）">
            <input className="input is-small" type="number" min={0} step="any" value={f.billing_per_su} onChange={(e) => set("billing_per_su", Number(e.target.value))} />
          </Field>
          <Field label="查余额的命令" hint="比如 accounts；留空就不查">
            <input className="input code" value={f.balance_cmd} onChange={(e) => set("balance_cmd", e.target.value)} />
          </Field>
          <Field label="架构">
            <input className="input code is-small" value={f.arch} placeholder="aarch64" onChange={(e) => set("arch", e.target.value)} />
          </Field>
        </div>
      </details>
      {err && <div className="err-box">{err}</div>}
      <div className="form-foot">
        <button type="button" className="ab-btn ab-btn-primary" disabled={busy || !f.name} onClick={() => void save()}>
          保存
        </button>
        {!fake && f.host && (
          <button type="button" className="ab-btn" onClick={() => props.onProbe(`arb machine init ${f.name} --host ${f.host}`)}>
            登录上去自动测一遍…
          </button>
        )}
        <span style={{ flex: 1 }} />
        {props.original && (
          <button
            type="button"
            className="ab-btn ab-btn-destructive"
            onClick={async () => {
              try {
                props.onRemoved(await api.removeMachine(props.original!));
              } catch (e) {
                setErr(message(e));
              }
            }}
          >
            删除这台机器
          </button>
        )}
      </div>
    </div>
  );
}

function MachinesTab(props: { toast: (m: string) => void; onTerminal: (cmd: string) => void; onRaw: () => void }) {
  const [page, setPage] = useState<MachinesPage | null>(null);
  const [sel, setSel] = useState<string | null>(null);
  const [draft, setDraft] = useState<MachineForm | null>(null);
  useEffect(() => {
    api.settingsMachines().then(
      (p) => {
        setPage(p);
        setSel(p.machines[0]?.form.name ?? null);
      },
      (e) => props.toast(message(e)),
    );
  }, []);
  if (!page) return <p className="small muted">读取中…</p>;
  if (page.syntax_error)
    return (
      <div className="form">
        <div className="err-box">clusters.toml 不是合法的 TOML（{page.syntax_error}），先在「高级」里修好它。</div>
        <div>
          <button type="button" className="ab-btn" onClick={props.onRaw}>
            打开高级
          </button>
        </div>
      </div>
    );
  const current = draft ?? page.machines.find((m) => m.form.name === sel)?.form ?? null;
  const error = draft ? null : page.machines.find((m) => m.form.name === sel)?.error ?? null;
  return (
    <div className="form">
      <div className="rows">
        {page.machines.map((m) => (
          <div
            key={m.form.name}
            className={"row" + (!draft && m.form.name === sel ? " is-sel" : "")}
            role="button"
            tabIndex={0}
            onClick={() => {
              setDraft(null);
              setSel(m.form.name);
            }}
          >
            <Glyph name="server" tone={m.error ? "bad" : "muted"} />
            <span className="main">
              <span className="t">{m.form.name}</span>
              <span className="m">{m.error ? "有问题，点开看" : m.form.kind === "fake" ? "本机模拟" : `${m.form.host} · ${m.form.account}`}</span>
            </span>
            <span className="r">{m.form.partitions.length} 个分区</span>
          </div>
        ))}
        <div className={"row" + (draft ? " is-sel" : "")} role="button" tabIndex={0} onClick={() => api.blankMachine("").then(setDraft)}>
          <Glyph name="plus" tone="muted" />
          <span className="main">
            <span className="t muted">添加一台机器</span>
          </span>
          <span />
        </div>
      </div>
      {current && (
        <MachineEditor
          original={draft ? null : sel}
          initial={current}
          error={error}
          onProbe={props.onTerminal}
          onSaved={(p, name) => {
            setPage(p);
            setDraft(null);
            setSel(name);
            props.toast(`已保存 ${name}`);
          }}
          onRemoved={(p) => {
            setPage(p);
            setSel(p.machines[0]?.form.name ?? null);
          }}
        />
      )}
      {!current && page.machines.length === 0 && (
        <p className="small muted">还没有机器。点「添加一台机器」，或者在终端里 arb machine init 名字 --host 登录节点 --write，它会登录上去测好再写进来。</p>
      )}
    </div>
  );
}

function ProjectTab(props: { projects: ProjectSummary[]; root?: string; toast: (m: string) => void }) {
  const choices = props.projects.filter((p) => p.kind === "project" && !p.error);
  const [root, setRoot] = useState(props.root ?? choices[0]?.root ?? "");
  const [s, setS] = useState<ProjectSettings | null>(null);
  const [f, setF] = useState<ProjectSettings["form"] | null>(null);
  const [err, setErr] = useState<string | null>(null);
  useEffect(() => {
    if (!root) return;
    setErr(null);
    api.settingsProject(root).then(
      (x) => {
        setS(x);
        setF(x.form);
      },
      (e) => props.toast(message(e)),
    );
  }, [root]);
  if (choices.length === 0) return <p className="small muted">还没有项目。</p>;
  const set = <K extends keyof ProjectSettings["form"]>(k: K, v: ProjectSettings["form"][K]) => f && setF({ ...f, [k]: v });
  return (
    <div className="form">
      <Field label="项目">
        <select className="select" value={root} onChange={(e) => setRoot(e.target.value)}>
          {choices.map((p) => (
            <option key={p.root} value={p.root}>
              {p.name}
            </option>
          ))}
        </select>
      </Field>
      {s && f && (
        <>
          {s.error && <div className="err-box">arb 现在读这个项目的设置会报错：{s.error}</div>}
          <fieldset className="fieldset">
            <legend>提交</legend>
            <Field label="默认机器" hint="run 没指定机器时用它">
              <select className="select" value={f.default_machine} onChange={(e) => set("default_machine", e.target.value)}>
                <option value="">（不设）</option>
                {s.machines.map((m) => (
                  <option key={m} value={m}>
                    {m}
                  </option>
                ))}
              </select>
            </Field>
            <Field label="训练命令" hint="run 没写命令时用它，比如 python train.py">
              <input className="input code" value={f.run_command} onChange={(e) => set("run_command", e.target.value)} />
            </Field>
            <Field label="参数写法" hint="--set k=v 变成命令行的样子">
              <select className="select" value={f.override_style} onChange={(e) => set("override_style", e.target.value)}>
                {s.override_styles.map((o) => (
                  <option key={o} value={o}>
                    {o === "hydra" ? "hydra（k=v）" : o === "argparse" ? "argparse（--k v）" : `${o}（--k v）`}
                  </option>
                ))}
              </select>
            </Field>
            <Field label="预算（SU）" hint="0 = 不设预算">
              <input className="input is-small" type="number" min={0} step="any" value={f.total_su} onChange={(e) => set("total_su", Number(e.target.value))} />
            </Field>
          </fieldset>
          <fieldset className="fieldset">
            <legend>Weights &amp; Biases</legend>
            <Field label="entity">
              <input className="input code" value={f.wandb_entity} onChange={(e) => set("wandb_entity", e.target.value)} />
            </Field>
            <Field label="project">
              <input className="input code" value={f.wandb_project} onChange={(e) => set("wandb_project", e.target.value)} />
            </Field>
          </fieldset>
          <fieldset className="fieldset">
            <legend>盯作业</legend>
            <Field label="观察期（分钟）" hint="作业开始后这段时间里看日志，出错就取消">
              <input className="input is-small" type="number" min={0} value={f.observation_minutes} onChange={(e) => set("observation_minutes", Number(e.target.value))} />
            </Field>
            <Field label="失败的样子" hint="日志里出现这些（正则，每行一条）就取消剩下的作业">
              <textarea className="textarea code" value={lines(f.fail_patterns)} onChange={(e) => set("fail_patterns", unlines(e.target.value))} />
            </Field>
          </fieldset>
          {err && <div className="err-box">{err}</div>}
          <div className="form-foot">
            <button
              type="button"
              className="ab-btn ab-btn-primary"
              disabled={JSON.stringify(f) === JSON.stringify(s.form)}
              onClick={async () => {
                setErr(null);
                try {
                  const x = await api.saveProject(root, f);
                  setS(x);
                  setF(x.form);
                  props.toast("已保存项目设置");
                } catch (e) {
                  setErr(message(e));
                }
              }}
            >
              保存
            </button>
            <button type="button" className="ab-btn" disabled={JSON.stringify(f) === JSON.stringify(s.form)} onClick={() => setF(s.form)}>
              撤销修改
            </button>
            <span className="small faint mono" style={{ marginLeft: "auto" }}>
              {s.path}
            </span>
          </div>
        </>
      )}
    </div>
  );
}

export function SettingsView(props: {
  tab: SettingsTab;
  root?: string;
  projects: ProjectSummary[];
  now: number;
  go: (tab: SettingsTab) => void;
  toast: (m: string) => void;
  onTerminal: (cmd: string) => void;
}) {
  const tabs: [SettingsTab, string][] = [
    ["general", "通用"],
    ["machines", "机器"],
    ["project", "项目"],
    ["advanced", "高级"],
  ];
  return (
    <div className="page is-wide">
      <div className="page-head">
        <h1 className="title">设置</h1>
      </div>
      <div className="settings">
        <div className="settings-nav">
          {tabs.map(([k, label]) => (
            <button key={k} type="button" className={"nav" + (props.tab === k ? " is-on" : "")} onClick={() => props.go(k)}>
              <span className="lbl">{label}</span>
            </button>
          ))}
        </div>
        <div>
          {props.tab === "general" && <General now={props.now} toast={props.toast} />}
          {props.tab === "machines" && <MachinesTab toast={props.toast} onTerminal={props.onTerminal} onRaw={() => props.go("advanced")} />}
          {props.tab === "project" && <ProjectTab projects={props.projects} root={props.root} toast={props.toast} />}
          {props.tab === "advanced" && <RawEditor onToast={props.toast} onTerminal={props.onTerminal} />}
        </div>
      </div>
    </div>
  );
}
