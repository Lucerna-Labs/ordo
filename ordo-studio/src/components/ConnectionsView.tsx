import React, { useEffect, useMemo, useState } from "react";

// Schema-driven Connections tab.
//
// Operator picks a tile (OpenAI, SSH, webhook, ...) -> modal
// renders form fields described by the catalog -> save calls the
// control API, which seals the secret in the vault and immediately
// runs the type's tester. Green/red status comes back in the same
// response so the operator sees the result without an extra click.
//
// Schema lives in `ordo-connections::types::catalog()` (Rust) and is
// fetched at runtime â€” no UI change required to add a new type.

const CONTROL_API_ORIGIN =
  (typeof window !== "undefined" &&
    (window as Window & { __ORDO_CONTROL_ORIGIN__?: string }).__ORDO_CONTROL_ORIGIN__) ||
  "http://127.0.0.1:4141";

type FieldType = "text" | "url" | "email" | "number" | "long_text";

interface FieldSchema {
  name: string;
  label: string;
  field_type: FieldType;
  required: boolean;
  placeholder: string;
  help: string;
}

type ConnectionCategory = "ai_provider" | "infrastructure" | "generic";

interface ConnectionType {
  id: string;
  display_name: string;
  description: string;
  icon: string;
  category: ConnectionCategory;
  fields: FieldSchema[];
  requires_secret: boolean;
  secret_label: string;
  secret_placeholder: string;
  secret_help: string;
  has_test: boolean;
}

type ConnectionStatus = "untested" | "ok" | "error";

interface ConnectionRow {
  id: string;
  workspace_id: string;
  type_id: string;
  friendly_name: string;
  fields: Record<string, unknown>;
  vault_secret_id: string | null;
  status: ConnectionStatus;
  status_detail: string | null;
  last_test_at_ms: number | null;
  created_at_ms: number;
  updated_at_ms: number;
}

interface CatalogResponse {
  count: number;
  types: ConnectionType[];
}

interface ListResponse {
  count: number;
  connections: ConnectionRow[];
}

interface TestReport {
  status: "ok" | "error" | "not_applicable";
  detail: string;
  duration_ms: number;
}

interface TestResponse {
  report: TestReport;
  connection: ConnectionRow;
}

const CATEGORY_LABELS: Record<ConnectionCategory, string> = {
  ai_provider: "AI Provider",
  infrastructure: "Infrastructure",
  generic: "Generic",
};

function statusBadge(status: ConnectionStatus): {
  text: string;
  className: string;
  dotClassName: string;
} {
  switch (status) {
    case "ok":
      return {
        text: "Connected",
        className: "border-emerald-300/30 bg-emerald-500/10 text-emerald-200",
        dotClassName: "bg-emerald-400",
      };
    case "error":
      return {
        text: "Error",
        className: "border-rose-300/30 bg-rose-500/10 text-rose-200",
        dotClassName: "bg-rose-400",
      };
    case "untested":
    default:
      return {
        text: "Needs setup",
        className: "border-amber-300/30 bg-amber-500/10 text-amber-200",
        dotClassName: "bg-amber-400",
      };
  }
}

function formatTimestamp(ms: number | null): string {
  if (!ms) return "never";
  const d = new Date(ms);
  return d.toLocaleString();
}

interface FormState {
  friendlyName: string;
  fields: Record<string, string>;
  secret: string;
}

function emptyFormState(type: ConnectionType, existing?: ConnectionRow): FormState {
  const fields: Record<string, string> = {};
  for (const field of type.fields) {
    const incoming = existing?.fields?.[field.name];
    fields[field.name] = incoming === undefined || incoming === null ? "" : String(incoming);
  }
  return {
    friendlyName: existing?.friendly_name ?? "",
    fields,
    secret: "",
  };
}

export function ConnectionsView() {
  const [catalog, setCatalog] = useState<ConnectionType[] | null>(null);
  const [connections, setConnections] = useState<ConnectionRow[] | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [modalType, setModalType] = useState<ConnectionType | null>(null);
  const [modalExisting, setModalExisting] = useState<ConnectionRow | null>(null);
  const [form, setForm] = useState<FormState | null>(null);
  const [busy, setBusy] = useState(false);
  const [formMessage, setFormMessage] = useState<{
    tone: "ok" | "warn" | "err";
    text: string;
  } | null>(null);
  const [testing, setTesting] = useState<string | null>(null);

  async function refreshAll() {
    setError(null);
    try {
      const [catalogResp, listResp] = await Promise.all([
        fetch(`${CONTROL_API_ORIGIN}/api/connections/types`),
        fetch(`${CONTROL_API_ORIGIN}/api/connections`),
      ]);
      if (!catalogResp.ok) {
        throw new Error(`catalog: ${catalogResp.status}`);
      }
      if (!listResp.ok) {
        throw new Error(`list: ${listResp.status}`);
      }
      const catalogData = (await catalogResp.json()) as CatalogResponse;
      const listData = (await listResp.json()) as ListResponse;
      setCatalog(catalogData.types);
      setConnections(listData.connections);
    } catch (err) {
      setError(
        err instanceof Error
          ? err.message
          : `control API unreachable at ${CONTROL_API_ORIGIN}`,
      );
    }
  }

  useEffect(() => {
    void refreshAll();
  }, []);

  function openCreate(type: ConnectionType) {
    setModalType(type);
    setModalExisting(null);
    setForm(emptyFormState(type));
    setFormMessage(null);
  }

  function openEdit(row: ConnectionRow) {
    if (!catalog) return;
    const type = catalog.find((t) => t.id === row.type_id);
    if (!type) {
      setFormMessage({
        tone: "err",
        text: `unknown type ${row.type_id}; can't edit`,
      });
      return;
    }
    setModalType(type);
    setModalExisting(row);
    setForm(emptyFormState(type, row));
    setFormMessage(null);
  }

  function closeModal() {
    setModalType(null);
    setModalExisting(null);
    setForm(null);
    setFormMessage(null);
  }

  async function submitForm(event: React.FormEvent<HTMLFormElement>) {
    event.preventDefault();
    if (!modalType || !form) return;
    const trimmedName = form.friendlyName.trim();
    if (!trimmedName) {
      setFormMessage({ tone: "warn", text: "Friendly name is required." });
      return;
    }

    // Cast number fields to numbers; pass everything else as strings.
    const fieldPayload: Record<string, unknown> = {};
    for (const field of modalType.fields) {
      const raw = form.fields[field.name] ?? "";
      const trimmed = raw.trim();
      if (!trimmed && !field.required) continue;
      if (field.field_type === "number") {
        const n = Number(trimmed);
        if (Number.isNaN(n)) {
          setFormMessage({
            tone: "warn",
            text: `${field.label} must be a number.`,
          });
          return;
        }
        fieldPayload[field.name] = n;
      } else {
        fieldPayload[field.name] = trimmed;
      }
    }

    setBusy(true);
    setFormMessage({ tone: "ok", text: "Saving and testing connection..." });
    try {
      let url = `${CONTROL_API_ORIGIN}/api/connections`;
      let method: "POST" | "PATCH" = "POST";
      let body: Record<string, unknown> = {
        type_id: modalType.id,
        friendly_name: trimmedName,
        fields: fieldPayload,
      };
      if (form.secret) {
        body.secret = form.secret;
      }
      if (modalExisting) {
        url = `${CONTROL_API_ORIGIN}/api/connections/${modalExisting.id}`;
        method = "PATCH";
        body = {
          friendly_name: trimmedName,
          fields: fieldPayload,
        };
        if (form.secret) {
          body.secret = form.secret;
        }
      }
      const response = await fetch(url, {
        method,
        headers: { "content-type": "application/json" },
        body: JSON.stringify(body),
      });
      if (!response.ok) {
        const text = await response.text();
        throw new Error(text || `control API returned ${response.status}`);
      }
      const saved = (await response.json()) as ConnectionRow;
      const badge = statusBadge(saved.status);
      setFormMessage({
        tone: saved.status === "ok" ? "ok" : saved.status === "error" ? "err" : "warn",
        text:
          saved.status === "ok"
            ? `Saved. ${badge.text}: ${saved.status_detail ?? ""}`
            : saved.status === "error"
              ? `Saved, but test failed: ${saved.status_detail ?? "(no detail)"}`
              : `Saved. Test was not applicable.`,
      });
      await refreshAll();
    } catch (err) {
      setFormMessage({
        tone: "err",
        text: err instanceof Error ? err.message : String(err),
      });
    } finally {
      setBusy(false);
    }
  }

  async function runTest(row: ConnectionRow) {
    setTesting(row.id);
    try {
      const response = await fetch(
        `${CONTROL_API_ORIGIN}/api/connections/${row.id}/test`,
        { method: "POST" },
      );
      if (!response.ok) {
        const text = await response.text();
        throw new Error(text || `control API returned ${response.status}`);
      }
      // Result is reflected via refresh.
      await refreshAll();
      const data = (await response.json()) as TestResponse;
      // Surface a transient status badge if the modal is closed.
      if (data.report.status === "ok") {
        // Could pop a toast here later â€” for now the row's status
        // dot already shows green.
      }
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err));
    } finally {
      setTesting(null);
    }
  }

  async function deleteRow(row: ConnectionRow) {
    if (!confirm(`Remove "${row.friendly_name}"? This retires the stored secret.`)) {
      return;
    }
    try {
      const response = await fetch(
        `${CONTROL_API_ORIGIN}/api/connections/${row.id}`,
        { method: "DELETE" },
      );
      if (!response.ok) {
        const text = await response.text();
        throw new Error(text || `control API returned ${response.status}`);
      }
      await refreshAll();
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err));
    }
  }

  const grouped = useMemo(() => {
    if (!catalog) return null;
    const groups: Record<ConnectionCategory, ConnectionType[]> = {
      ai_provider: [],
      infrastructure: [],
      generic: [],
    };
    for (const t of catalog) {
      groups[t.category].push(t);
    }
    return groups;
  }, [catalog]);

  return (
    <section className="liquid-panel relative overflow-hidden rounded-[2.75rem] border p-10">
      <div className="flex flex-wrap items-start justify-between gap-6">
        <div>
          <div className="text-[11px] font-semibold uppercase tracking-[0.36em] text-slate-400">
            Connections
          </div>
          <h2 className="mt-3 text-4xl font-light tracking-tight text-white">
            Plug in your real backends
          </h2>
          <p className="mt-2 max-w-2xl text-sm leading-7 text-slate-400">
            Pick a tile to add an account: OpenAI, local model server, SSH, generic API key,
            generic webhook. Credentials are sealed in your local vault â€” they never leave this
            machine. On save, Ordo runs a real Test Connection against the live service
            so you see green or red right away.
          </p>
        </div>
        <button
          onClick={() => {
            void refreshAll();
          }}
          className="rounded-full border border-teal-300/30 bg-teal-500/10 px-4 py-2 text-xs font-semibold uppercase tracking-[0.2em] text-teal-200 transition hover:border-teal-300/50 hover:bg-teal-500/20"
        >
          Refresh
        </button>
      </div>

      {error && (
        <div className="mt-6 rounded-2xl border border-rose-400/30 bg-rose-500/10 p-5 text-sm text-rose-200">
          {error}
          <div className="mt-1 text-xs text-rose-300/80">
            Start the runtime with <code>cargo run -p ordo-cli -- runtime</code>. Control API
            defaults to <code>127.0.0.1:4141</code>.
          </div>
        </div>
      )}

      {connections !== null && connections.length > 0 && (
        <div className="mt-8">
          <div className="text-[11px] font-semibold uppercase tracking-[0.28em] text-slate-400">
            Configured
          </div>
          <div className="mt-3 grid gap-3">
            {connections.map((row) => {
              const badge = statusBadge(row.status);
              const type = catalog?.find((t) => t.id === row.type_id);
              return (
                <article
                  key={row.id}
                  className="rounded-3xl border border-white/10 bg-white/[0.03] p-5"
                >
                  <div className="flex flex-wrap items-center justify-between gap-3">
                    <div className="flex items-center gap-3">
                      <div className="grid h-10 w-10 place-items-center rounded-2xl border border-white/10 bg-white/[0.04] text-xl">
                        {type?.icon ?? "ðŸ”Œ"}
                      </div>
                      <div>
                        <div className="text-sm font-semibold text-slate-100">
                          {row.friendly_name}
                        </div>
                        <div className="mt-1 text-[11px] uppercase tracking-[0.22em] text-slate-500">
                          {type?.display_name ?? row.type_id}
                          {row.vault_secret_id ? " Â· secret stored" : " Â· no secret"}
                        </div>
                      </div>
                    </div>
                    <div className="flex flex-wrap items-center gap-3">
                      <span
                        className={`inline-flex items-center gap-2 rounded-full border px-3 py-1 text-[11px] font-semibold uppercase tracking-[0.2em] ${badge.className}`}
                      >
                        <span className={`h-2 w-2 rounded-full ${badge.dotClassName}`} />
                        {badge.text}
                      </span>
                      <button
                        type="button"
                        disabled={testing === row.id}
                        onClick={() => {
                          void runTest(row);
                        }}
                        className="rounded-full border border-teal-300/30 bg-teal-500/10 px-3 py-1.5 text-[11px] font-semibold uppercase tracking-[0.2em] text-teal-200 transition hover:border-teal-300/50 hover:bg-teal-500/20 disabled:opacity-50"
                      >
                        {testing === row.id ? "Testing..." : "Test"}
                      </button>
                      <button
                        type="button"
                        onClick={() => openEdit(row)}
                        className="rounded-full border border-white/15 bg-white/5 px-3 py-1.5 text-[11px] font-semibold uppercase tracking-[0.2em] text-slate-200 transition hover:border-white/30"
                      >
                        Edit
                      </button>
                      <button
                        type="button"
                        onClick={() => {
                          void deleteRow(row);
                        }}
                        className="rounded-full border border-rose-400/30 bg-rose-500/10 px-3 py-1.5 text-[11px] font-semibold uppercase tracking-[0.2em] text-rose-200 transition hover:border-rose-400/50 hover:bg-rose-500/20"
                      >
                        Delete
                      </button>
                    </div>
                  </div>
                  {row.status_detail && (
                    <div className="mt-3 text-xs text-slate-400">
                      <span className="font-semibold text-slate-300">last result Â· </span>
                      {row.status_detail}
                    </div>
                  )}
                  <div className="mt-1 text-[10px] uppercase tracking-[0.22em] text-slate-500">
                    last test Â· {formatTimestamp(row.last_test_at_ms)}
                  </div>
                </article>
              );
            })}
          </div>
        </div>
      )}

      {grouped && (
        <div className="mt-10">
          <div className="text-[11px] font-semibold uppercase tracking-[0.28em] text-slate-400">
            Add a connection
          </div>
          <div className="mt-3 space-y-6">
            {(Object.keys(CATEGORY_LABELS) as ConnectionCategory[]).map((cat) => {
              const types = grouped[cat];
              if (types.length === 0) return null;
              return (
                <div key={cat}>
                  <div className="text-[10px] font-semibold uppercase tracking-[0.28em] text-slate-500">
                    {CATEGORY_LABELS[cat]}
                  </div>
                  <div className="mt-2 grid gap-3 sm:grid-cols-2 lg:grid-cols-3 xl:grid-cols-4">
                    {types.map((type) => (
                      <button
                        key={type.id}
                        type="button"
                        onClick={() => openCreate(type)}
                        className="group flex flex-col items-start gap-2 rounded-3xl border border-white/10 bg-white/[0.02] p-4 text-left transition hover:border-teal-300/40 hover:bg-white/[0.06]"
                      >
                        <div className="flex items-center gap-3">
                          <div className="grid h-10 w-10 place-items-center rounded-2xl border border-white/10 bg-white/[0.05] text-xl">
                            {type.icon}
                          </div>
                          <div className="text-sm font-semibold text-slate-100">
                            {type.display_name}
                          </div>
                        </div>
                        <div className="text-xs leading-5 text-slate-400">
                          {type.description}
                        </div>
                      </button>
                    ))}
                  </div>
                </div>
              );
            })}
          </div>
        </div>
      )}

      {modalType && form && (
        <div className="fixed inset-0 z-50 flex items-center justify-center bg-black/60 px-4 backdrop-blur-sm">
          <form
            onSubmit={submitForm}
            className="liquid-panel relative w-full max-w-lg rounded-[2rem] border p-7"
          >
            <div className="flex items-start justify-between gap-3">
              <div className="flex items-center gap-3">
                <div className="grid h-10 w-10 place-items-center rounded-2xl border border-white/10 bg-white/[0.05] text-xl">
                  {modalType.icon}
                </div>
                <div>
                  <div className="text-[10px] font-semibold uppercase tracking-[0.28em] text-slate-400">
                    {modalExisting ? "Edit connection" : "New connection"}
                  </div>
                  <div className="text-lg font-semibold text-white">
                    {modalType.display_name}
                  </div>
                </div>
              </div>
              <button
                type="button"
                onClick={closeModal}
                className="rounded-full border border-white/15 bg-white/5 px-3 py-1.5 text-[11px] font-semibold uppercase tracking-[0.2em] text-slate-300 transition hover:border-white/30"
              >
                Close
              </button>
            </div>
            <p className="mt-3 text-xs leading-6 text-slate-400">{modalType.description}</p>

            <div className="mt-5 grid gap-4">
              <label className="text-[10px] font-semibold uppercase tracking-[0.22em] text-slate-400">
                Friendly name
                <input
                  value={form.friendlyName}
                  onChange={(event) =>
                    setForm((prev) =>
                      prev ? { ...prev, friendlyName: event.target.value } : prev,
                    )
                  }
                  placeholder={`${modalType.display_name} (personal)`}
                  className="mt-2 w-full rounded-2xl border border-white/10 bg-black/20 px-3 py-2 text-sm text-slate-100 outline-none transition focus:border-teal-300/40 focus:bg-black/40"
                  required
                />
              </label>

              {modalType.fields.map((field) => (
                <label
                  key={field.name}
                  className="text-[10px] font-semibold uppercase tracking-[0.22em] text-slate-400"
                >
                  {field.label}
                  {field.required ? " *" : ""}
                  {field.field_type === "long_text" ? (
                    <textarea
                      value={form.fields[field.name] ?? ""}
                      onChange={(event) => {
                        const v = event.target.value;
                        setForm((prev) =>
                          prev
                            ? { ...prev, fields: { ...prev.fields, [field.name]: v } }
                            : prev,
                        );
                      }}
                      placeholder={field.placeholder}
                      rows={5}
                      className="mt-2 w-full rounded-2xl border border-white/10 bg-black/20 px-3 py-2 font-mono text-xs text-slate-100 outline-none transition focus:border-teal-300/40 focus:bg-black/40"
                      required={field.required}
                    />
                  ) : (
                    <input
                      type={
                        field.field_type === "number"
                          ? "number"
                          : field.field_type === "email"
                            ? "email"
                            : field.field_type === "url"
                              ? "url"
                              : "text"
                      }
                      value={form.fields[field.name] ?? ""}
                      onChange={(event) => {
                        const v = event.target.value;
                        setForm((prev) =>
                          prev
                            ? { ...prev, fields: { ...prev.fields, [field.name]: v } }
                            : prev,
                        );
                      }}
                      placeholder={field.placeholder}
                      className="mt-2 w-full rounded-2xl border border-white/10 bg-black/20 px-3 py-2 text-sm text-slate-100 outline-none transition focus:border-teal-300/40 focus:bg-black/40"
                      required={field.required}
                    />
                  )}
                  {field.help && (
                    <span className="mt-1 block text-[10px] font-medium tracking-[0.18em] text-slate-500">
                      {field.help}
                    </span>
                  )}
                </label>
              ))}

              {modalType.requires_secret && (
                <label className="text-[10px] font-semibold uppercase tracking-[0.22em] text-slate-400">
                  {modalType.secret_label}
                  {modalExisting?.vault_secret_id ? " (leave blank to keep current)" : " *"}
                  <input
                    type="password"
                    value={form.secret}
                    onChange={(event) => {
                      const v = event.target.value;
                      setForm((prev) => (prev ? { ...prev, secret: v } : prev));
                    }}
                    placeholder={modalType.secret_placeholder}
                    autoComplete="new-password"
                    className="mt-2 w-full rounded-2xl border border-white/10 bg-black/20 px-3 py-2 text-sm text-slate-100 outline-none transition focus:border-teal-300/40 focus:bg-black/40"
                    required={!modalExisting?.vault_secret_id}
                  />
                  {modalType.secret_help && (
                    <span className="mt-1 block text-[10px] font-medium tracking-[0.18em] text-slate-500">
                      {modalType.secret_help}
                    </span>
                  )}
                </label>
              )}
            </div>

            <div className="mt-6 flex flex-wrap items-center gap-3">
              <button
                type="submit"
                disabled={busy}
                className="rounded-full border border-teal-300/30 bg-teal-500/15 px-5 py-2 text-xs font-semibold uppercase tracking-[0.2em] text-teal-100 transition hover:border-teal-300/50 hover:bg-teal-500/25 disabled:opacity-50"
              >
                {busy ? "Saving..." : modalExisting ? "Save changes" : "Save and test"}
              </button>
              {formMessage && (
                <span
                  className={`text-xs ${
                    formMessage.tone === "err"
                      ? "text-rose-300"
                      : formMessage.tone === "warn"
                        ? "text-amber-300"
                        : "text-teal-200"
                  }`}
                >
                  {formMessage.text}
                </span>
              )}
            </div>
          </form>
        </div>
      )}
    </section>
  );
}
