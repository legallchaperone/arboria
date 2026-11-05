// A small modal form or confirmation (new idea, note, reject, close an idea, seeds,
// remove a project): a title, a line of explanation, fields, one button. While the action
// runs the buttons are off; its error stays in the dialog.

import { useState, type ReactNode } from "react";
import { message } from "../api";

export interface Field {
  key: string;
  label: string;
  placeholder?: string;
  initial?: string;
  multiline?: boolean;
  required?: boolean;
  /** A select over these values ("" = none). */
  options?: { value: string; label: string }[];
  hint?: string;
  max?: number;
  mono?: boolean;
}

export interface DialogSpec {
  title: string;
  body?: ReactNode;
  fields?: Field[];
  confirm: string;
  danger?: boolean;
  /** Throws to keep the dialog open with the message. */
  run: (values: Record<string, string>) => Promise<void>;
}

export function Dialog(props: { spec: DialogSpec; onClose: () => void }) {
  const s = props.spec;
  const [values, setValues] = useState<Record<string, string>>(() =>
    Object.fromEntries((s.fields ?? []).map((f) => [f.key, f.initial ?? f.options?.[0]?.value ?? ""])),
  );
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const missing = (s.fields ?? []).some((f) => f.required && !values[f.key]?.trim());
  const submit = async () => {
    if (busy || missing) return;
    setBusy(true);
    setError(null);
    try {
      await s.run(values);
      props.onClose();
    } catch (e) {
      setError(message(e));
    } finally {
      setBusy(false);
    }
  };
  return (
    <div className="app-modal" role="presentation" onMouseDown={() => !busy && props.onClose()}>
      <form
        className="ab-sheet is-alone app-dialog"
        role="dialog"
        aria-modal="true"
        aria-label={s.title}
        onMouseDown={(e) => e.stopPropagation()}
        onKeyDown={(e) => {
          if (e.key === "Escape" && !busy) props.onClose();
          if (e.key === "Enter" && (e.metaKey || e.ctrlKey)) {
            e.preventDefault();
            void submit();
          }
        }}
        onSubmit={(e) => {
          e.preventDefault();
          void submit();
        }}
      >
        <h2 className="heading">{s.title}</h2>
        {s.body && <div className="small muted app-dialog-body">{s.body}</div>}
        {(s.fields ?? []).map((f, i) => (
          <label key={f.key} className="app-dialog-field">
            <span className="small">
              {f.label}
              {!f.required && !f.options && <span className="muted">（可选）</span>}
            </span>
            {f.options ? (
              <select className="app-field body" value={values[f.key]} disabled={busy} onChange={(e) => setValues((v) => ({ ...v, [f.key]: e.target.value }))}>
                {f.options.map((o) => (
                  <option key={o.value} value={o.value}>
                    {o.label}
                  </option>
                ))}
              </select>
            ) : f.multiline ? (
              <textarea
                autoFocus={i === 0}
                className={"app-field body" + (f.mono ? " code" : "")}
                rows={4}
                placeholder={f.placeholder}
                maxLength={f.max}
                value={values[f.key]}
                disabled={busy}
                onChange={(e) => setValues((v) => ({ ...v, [f.key]: e.target.value }))}
              />
            ) : (
              <input
                autoFocus={i === 0}
                className={"app-field body" + (f.mono ? " code" : "")}
                placeholder={f.placeholder}
                maxLength={f.max}
                value={values[f.key]}
                disabled={busy}
                onChange={(e) => setValues((v) => ({ ...v, [f.key]: e.target.value }))}
              />
            )}
            {f.hint && <span className="small muted">{f.hint}</span>}
          </label>
        ))}
        {error && <div className="err-box app-pre">{error}</div>}
        <div className="app-row app-dialog-actions">
          <span className="ab-spacer" />
          <button type="button" className="ab-btn body" disabled={busy} onClick={props.onClose}>
            取消
          </button>
          <button type="submit" autoFocus={!s.fields?.length} className={"ab-btn body " + (s.danger ? "ab-btn-destructive" : "ab-btn-primary")} disabled={busy || missing}>
            {busy ? "正在做…" : s.confirm}
          </button>
        </div>
      </form>
    </div>
  );
}
