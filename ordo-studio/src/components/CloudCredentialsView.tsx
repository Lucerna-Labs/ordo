import React, { useEffect, useState } from "react";

interface CloudCredential {
  service: string;
  label: string;
  auth_style: string;
  base_url: string | null;
  has_secret: boolean;
  extras: Record<string, string>;
  created_at: string;
  updated_at: string;
}

interface CredentialsResponse {
  count: number;
  credentials: CloudCredential[];
}

type UpsertPayload = {
  service: string;
  label?: string;
  auth_style?: string;
  base_url?: string;
  secret?: string;
  extras?: Record<string, string>;
};

const CONTROL_API_ORIGIN =
  (typeof window !== "undefined" &&
    (window as Window & { __ORDO_CONTROL_ORIGIN__?: string }).__ORDO_CONTROL_ORIGIN__) ||
  "http://127.0.0.1:4141";

const AUTH_STYLES = [
  { id: "bearer", label: "bearer (Authorization: Bearer ...)" },
  { id: "basic", label: "basic (user:pass)" },
  { id: "api_key_header", label: "api_key_header (custom header)" },
  { id: "api_key_query", label: "api_key_query (URL parameter)" },
  { id: "anthropic", label: "anthropic (x-api-key + anthropic-version)" },
];

function parseExtras(raw: string): Record<string, string> {
  const extras: Record<string, string> = {};
  raw.split(/\r?\n/).forEach((line) => {
    const trimmed = line.trim();
    if (!trimmed) return;
    const separator = trimmed.indexOf("=");
    if (separator < 0) return;
    const key = trimmed.slice(0, separator).trim();
    const value = trimmed.slice(separator + 1).trim();
    if (key) {
      extras[key] = value;
    }
  });
  return extras;
}

export function CloudCredentialsView() {
  const [state, setState] = useState<
    | { status: "loading" }
    | { status: "error"; message: string }
    | { status: "ready"; data: CredentialsResponse }
  >({ status: "loading" });
  const [service, setService] = useState("");
  const [label, setLabel] = useState("");
  const [authStyle, setAuthStyle] = useState("bearer");
  const [baseUrl, setBaseUrl] = useState("");
  const [secret, setSecret] = useState("");
  const [extras, setExtras] = useState("");
  const [busy, setBusy] = useState(false);
  const [message, setMessage] = useState<{ tone: "ok" | "warn" | "err"; text: string } | null>(
    null,
  );
  // Per-service test result. `testing` while in flight, then the
  // backend response shape: { ok: bool, error?: string }.
  const [testResults, setTestResults] = useState<
    Record<string, { status: "testing" | "ok" | "failed"; error?: string }>
  >({});
  // Per-service discovered model list. `loading` while in flight,
  // then `{ models: [...] }` on success or `{ error: '...' }` on
  // failure. Toggling discover hides the panel without dropping
  // the cached result, so re-opening is instant.
  const [discoveries, setDiscoveries] = useState<
    Record<
      string,
      | { status: "loading" }
      | { status: "ready"; models: string[]; open: boolean }
      | { status: "failed"; error: string; open: boolean }
    >
  >({});

  async function refresh() {
    setState({ status: "loading" });
    try {
      const response = await fetch(`${CONTROL_API_ORIGIN}/api/cloud/credentials`);
      if (!response.ok) {
        throw new Error(`control API returned ${response.status}`);
      }
      const data = (await response.json()) as CredentialsResponse;
      setState({ status: "ready", data });
    } catch (error) {
      setState({
        status: "error",
        message:
          error instanceof Error
            ? error.message
            : "control API is unreachable at " + CONTROL_API_ORIGIN,
      });
    }
  }

  useEffect(() => {
    void refresh();
  }, []);

  async function submit(event: React.FormEvent<HTMLFormElement>) {
    event.preventDefault();
    const trimmedService = service.trim();
    if (!trimmedService) {
      setMessage({ tone: "warn", text: "Service name is required." });
      return;
    }
    setBusy(true);
    setMessage({ tone: "ok", text: "Saving credential locally..." });
    const payload: UpsertPayload = { service: trimmedService };
    if (label.trim()) payload.label = label.trim();
    if (authStyle) payload.auth_style = authStyle;
    if (baseUrl.trim()) payload.base_url = baseUrl.trim();
    if (secret) payload.secret = secret;
    const parsedExtras = parseExtras(extras);
    if (Object.keys(parsedExtras).length > 0) payload.extras = parsedExtras;

    try {
      const response = await fetch(`${CONTROL_API_ORIGIN}/api/cloud/credentials`, {
        method: "POST",
        headers: { "content-type": "application/json" },
        body: JSON.stringify(payload),
      });
      if (!response.ok) {
        const text = await response.text();
        throw new Error(text || `control API returned ${response.status}`);
      }
      setSecret("");
      setMessage({ tone: "ok", text: "Credential saved. Secret stored locally only." });
      await refresh();
    } catch (error) {
      setMessage({
        tone: "err",
        text: error instanceof Error ? error.message : String(error),
      });
    } finally {
      setBusy(false);
    }
  }

  // Run a connectivity test against the saved credential's
  // provider. POST /api/cloud/credentials/test always returns
  // 200 with {ok, error?}; the badge below the row reflects ok.
  async function runTest(target: string) {
    setTestResults((prev) => ({ ...prev, [target]: { status: "testing" } }));
    try {
      const response = await fetch(`${CONTROL_API_ORIGIN}/api/cloud/credentials/test`, {
        method: "POST",
        headers: { "content-type": "application/json" },
        body: JSON.stringify({ service: target }),
      });
      const body = (await response.json()) as { ok?: boolean; error?: string };
      if (body.ok) {
        setTestResults((prev) => ({ ...prev, [target]: { status: "ok" } }));
      } else {
        setTestResults((prev) => ({
          ...prev,
          [target]: { status: "failed", error: body.error || "unknown error" },
        }));
      }
    } catch (error) {
      setTestResults((prev) => ({
        ...prev,
        [target]: {
          status: "failed",
          error: error instanceof Error ? error.message : String(error),
        },
      }));
    }
  }

  // Discover the live list of models the provider exposes. POST
  // /api/cloud/credentials/models returns {ok, models?, error?}.
  // The dropdown below the row renders models the operator can
  // click to set as the active model (via an upsert that merges
  // model into extras).
  async function discoverModels(target: string) {
    // Toggle if already loaded — second click hides the panel
    // without re-fetching.
    const current = discoveries[target];
    if (current && current.status !== "loading") {
      setDiscoveries((prev) => ({
        ...prev,
        [target]: { ...current, open: !current.open },
      }));
      return;
    }
    setDiscoveries((prev) => ({ ...prev, [target]: { status: "loading" } }));
    try {
      const response = await fetch(`${CONTROL_API_ORIGIN}/api/cloud/credentials/models`, {
        method: "POST",
        headers: { "content-type": "application/json" },
        body: JSON.stringify({ service: target }),
      });
      const body = (await response.json()) as {
        ok?: boolean;
        models?: string[];
        error?: string;
      };
      if (body.ok && Array.isArray(body.models)) {
        setDiscoveries((prev) => ({
          ...prev,
          [target]: { status: "ready", models: body.models!, open: true },
        }));
      } else {
        setDiscoveries((prev) => ({
          ...prev,
          [target]: {
            status: "failed",
            error: body.error || "discovery failed",
            open: true,
          },
        }));
      }
    } catch (error) {
      setDiscoveries((prev) => ({
        ...prev,
        [target]: {
          status: "failed",
          error: error instanceof Error ? error.message : String(error),
          open: true,
        },
      }));
    }
  }

  // Click-to-set: replace the credential's `model` extra without
  // touching other fields. The upsert merges the new model into
  // the existing extras dict so timeouts / context_window / etc.
  // are preserved.
  async function setModelForCredential(credential: CloudCredential, model: string) {
    setBusy(true);
    setMessage({ tone: "ok", text: `Setting ${credential.service} model to ${model}...` });
    try {
      const nextExtras = { ...credential.extras, model };
      const response = await fetch(`${CONTROL_API_ORIGIN}/api/cloud/credentials`, {
        method: "POST",
        headers: { "content-type": "application/json" },
        body: JSON.stringify({ service: credential.service, extras: nextExtras }),
      });
      if (!response.ok) {
        const text = await response.text();
        throw new Error(text || `control API returned ${response.status}`);
      }
      setMessage({
        tone: "ok",
        text: `Model for ${credential.service} set to ${model}.`,
      });
      await refresh();
    } catch (error) {
      setMessage({
        tone: "err",
        text: error instanceof Error ? error.message : String(error),
      });
    } finally {
      setBusy(false);
    }
  }

  async function remove(target: string) {
    setBusy(true);
    setMessage({ tone: "ok", text: `Removing credential for ${target}...` });
    try {
      const response = await fetch(`${CONTROL_API_ORIGIN}/api/cloud/credentials`, {
        method: "DELETE",
        headers: { "content-type": "application/json" },
        body: JSON.stringify({ service: target }),
      });
      if (!response.ok) {
        const text = await response.text();
        throw new Error(text || `control API returned ${response.status}`);
      }
      setMessage({ tone: "ok", text: `Credential for ${target} removed.` });
      await refresh();
    } catch (error) {
      setMessage({
        tone: "err",
        text: error instanceof Error ? error.message : String(error),
      });
    } finally {
      setBusy(false);
    }
  }

  return (
    <section className="liquid-panel relative overflow-hidden rounded-[2.75rem] border p-10">
      <div className="flex flex-wrap items-start justify-between gap-6">
        <div>
          <div className="text-[11px] font-semibold uppercase tracking-[0.36em] text-slate-400">
            Cloud credentials
          </div>
          <h2 className="mt-3 text-4xl font-light tracking-tight text-white">
            Local-first, not local-only
          </h2>
          <p className="mt-2 max-w-2xl text-sm leading-7 text-slate-400">
            Credentials live in the shared local SQLite store next to runtime settings. They are
            only used to sign outbound requests against the service you configure, and removing a
            credential instantly disables the matching <code>cloud.*</code> capability.
          </p>
        </div>
        <div className="flex items-center gap-3">
          <button
            onClick={() => {
              void refresh();
            }}
            className="rounded-full border border-teal-300/30 bg-teal-500/10 px-4 py-2 text-xs font-semibold uppercase tracking-[0.2em] text-teal-200 transition hover:border-teal-300/50 hover:bg-teal-500/20"
          >
            Refresh
          </button>
        </div>
      </div>

      <form
        onSubmit={submit}
        className="mt-8 grid gap-4 rounded-3xl border border-white/10 bg-white/[0.03] p-6 md:grid-cols-2"
      >
        <label className="text-xs font-semibold uppercase tracking-[0.22em] text-slate-400">
          Service
          <input
            value={service}
            onChange={(event) => setService(event.target.value)}
            placeholder="openai, anthropic, gemini, acme-search"
            className="mt-2 w-full rounded-2xl border border-white/10 bg-black/20 px-3 py-2 text-sm text-slate-100 outline-none transition focus:border-teal-300/40 focus:bg-black/40"
            required
          />
        </label>
        <label className="text-xs font-semibold uppercase tracking-[0.22em] text-slate-400">
          Label
          <input
            value={label}
            onChange={(event) => setLabel(event.target.value)}
            placeholder="OpenAI (prod)"
            className="mt-2 w-full rounded-2xl border border-white/10 bg-black/20 px-3 py-2 text-sm text-slate-100 outline-none transition focus:border-teal-300/40 focus:bg-black/40"
          />
        </label>
        <label className="text-xs font-semibold uppercase tracking-[0.22em] text-slate-400">
          Auth style
          <select
            value={authStyle}
            onChange={(event) => setAuthStyle(event.target.value)}
            className="mt-2 w-full rounded-2xl border border-white/10 bg-black/20 px-3 py-2 text-sm text-slate-100 outline-none transition focus:border-teal-300/40 focus:bg-black/40"
          >
            {AUTH_STYLES.map((entry) => (
              <option key={entry.id} value={entry.id}>
                {entry.label}
              </option>
            ))}
          </select>
        </label>
        <label className="text-xs font-semibold uppercase tracking-[0.22em] text-slate-400">
          Base URL (optional)
          <input
            value={baseUrl}
            onChange={(event) => setBaseUrl(event.target.value)}
            placeholder="https://api.openai.com/v1"
            className="mt-2 w-full rounded-2xl border border-white/10 bg-black/20 px-3 py-2 text-sm text-slate-100 outline-none transition focus:border-teal-300/40 focus:bg-black/40"
          />
        </label>
        <label className="text-xs font-semibold uppercase tracking-[0.22em] text-slate-400 md:col-span-2">
          Secret
          <input
            value={secret}
            onChange={(event) => setSecret(event.target.value)}
            type="password"
            placeholder="sk-... / api-key / user:pass"
            autoComplete="off"
            className="mt-2 w-full rounded-2xl border border-white/10 bg-black/20 px-3 py-2 text-sm text-slate-100 outline-none transition focus:border-teal-300/40 focus:bg-black/40"
          />
          <span className="mt-1 block text-[10px] font-medium tracking-[0.18em] text-slate-500">
            Stored locally. Never echoed back to the UI.
          </span>
        </label>
        <label className="text-xs font-semibold uppercase tracking-[0.22em] text-slate-400 md:col-span-2">
          Extras (key=value, one per line)
          <textarea
            value={extras}
            onChange={(event) => setExtras(event.target.value)}
            placeholder={"header_name=x-api-key\nparam_name=key"}
            rows={3}
            className="mt-2 w-full rounded-2xl border border-white/10 bg-black/20 px-3 py-2 font-mono text-xs text-slate-100 outline-none transition focus:border-teal-300/40 focus:bg-black/40"
          />
          <span className="mt-1 block text-[10px] font-medium tracking-[0.18em] text-slate-500">
            For api_key_header set header_name. For api_key_query set param_name. For anthropic optionally override anthropic-version.
          </span>
        </label>
        <div className="md:col-span-2 flex flex-wrap items-center gap-3">
          <button
            type="submit"
            disabled={busy}
            className="rounded-full border border-teal-300/30 bg-teal-500/15 px-5 py-2 text-xs font-semibold uppercase tracking-[0.2em] text-teal-100 transition hover:border-teal-300/50 hover:bg-teal-500/25 disabled:opacity-50"
          >
            Save credential
          </button>
          <button
            type="button"
            disabled={busy}
            onClick={() => {
              setService("");
              setLabel("");
              setAuthStyle("bearer");
              setBaseUrl("");
              setSecret("");
              setExtras("");
              setMessage({ tone: "ok", text: "Form cleared." });
            }}
            className="rounded-full border border-white/15 bg-white/5 px-5 py-2 text-xs font-semibold uppercase tracking-[0.2em] text-slate-300 transition hover:border-white/30 disabled:opacity-50"
          >
            Reset
          </button>
          {message && (
            <span
              className={`text-xs ${
                message.tone === "err"
                  ? "text-rose-300"
                  : message.tone === "warn"
                    ? "text-amber-300"
                    : "text-teal-200"
              }`}
            >
              {message.text}
            </span>
          )}
        </div>
      </form>

      {state.status === "loading" && (
        <div className="mt-8 rounded-2xl border border-white/10 bg-white/5 p-6 text-sm text-slate-300">
          Loading credential inventory...
        </div>
      )}
      {state.status === "error" && (
        <div className="mt-8 rounded-2xl border border-rose-400/30 bg-rose-500/10 p-6 text-sm text-rose-200">
          {state.message}
          <div className="mt-2 text-xs text-rose-300/80">
            Start the runtime with <code>cargo run</code> (control API defaults to{" "}
            <code>127.0.0.1:4141</code>).
          </div>
        </div>
      )}
      {state.status === "ready" && (
        <div className="mt-8 grid gap-4">
          {state.data.credentials.length === 0 && (
            <div className="rounded-2xl border border-white/10 bg-white/5 p-6 text-sm text-slate-300">
              No cloud credentials saved yet. Add one above to enable an outbound{" "}
              <code>cloud.*</code> service. Nothing leaves this machine until a credential is
              configured.
            </div>
          )}
          {state.data.credentials.map((credential) => (
            <article
              key={credential.service}
              className="rounded-3xl border border-white/10 bg-white/[0.03] p-5"
            >
              <div className="flex flex-wrap items-center justify-between gap-3">
                <div>
                  <div className="text-sm font-semibold text-slate-100">
                    {credential.label || credential.service}
                  </div>
                  <div className="mt-1 text-[11px] uppercase tracking-[0.22em] text-slate-500">
                    service Â· {credential.service} Â· auth Â· {credential.auth_style} Â· secret Â·{" "}
                    {credential.has_secret ? "stored" : "missing"}
                  </div>
                </div>
                <div className="flex items-center gap-2">
                  {(() => {
                    const r = testResults[credential.service];
                    let label: string;
                    let cls: string;
                    if (r?.status === "testing") {
                      label = "Testing...";
                      cls =
                        "border-amber-300/40 bg-amber-500/10 text-amber-200";
                    } else if (r?.status === "ok") {
                      label = "✓ Connected";
                      cls = "border-teal-300/50 bg-teal-500/20 text-teal-100";
                    } else if (r?.status === "failed") {
                      label = "✗ Failed — retry";
                      cls = "border-rose-400/50 bg-rose-500/20 text-rose-100";
                    } else {
                      label = "Test";
                      cls =
                        "border-teal-300/30 bg-teal-500/10 text-teal-200 hover:border-teal-300/50 hover:bg-teal-500/20";
                    }
                    return (
                      <button
                        type="button"
                        disabled={busy || r?.status === "testing"}
                        onClick={() => {
                          void runTest(credential.service);
                        }}
                        className={`rounded-full border px-4 py-1.5 text-[11px] font-semibold uppercase tracking-[0.2em] transition disabled:opacity-50 ${cls}`}
                      >
                        {label}
                      </button>
                    );
                  })()}
                  <button
                    type="button"
                    disabled={busy}
                    onClick={() => {
                      void discoverModels(credential.service);
                    }}
                    className="rounded-full border border-sky-300/30 bg-sky-500/10 px-4 py-1.5 text-[11px] font-semibold uppercase tracking-[0.2em] text-sky-200 transition hover:border-sky-300/50 hover:bg-sky-500/20 disabled:opacity-50"
                  >
                    {discoveries[credential.service]?.status === "loading"
                      ? "Discovering..."
                      : discoveries[credential.service] &&
                          discoveries[credential.service].status !== "loading" &&
                          (discoveries[credential.service] as { open: boolean }).open
                        ? "Hide models"
                        : "Discover models"}
                  </button>
                  <button
                    type="button"
                    disabled={busy}
                    onClick={() => {
                      void remove(credential.service);
                    }}
                    className="rounded-full border border-rose-400/30 bg-rose-500/10 px-4 py-1.5 text-[11px] font-semibold uppercase tracking-[0.2em] text-rose-200 transition hover:border-rose-400/50 hover:bg-rose-500/20 disabled:opacity-50"
                  >
                    Delete
                  </button>
                </div>
              </div>
              {testResults[credential.service]?.status === "failed" && (
                <div className="mt-3 rounded-2xl border border-rose-400/30 bg-rose-500/10 p-3 text-xs text-rose-200">
                  <span className="font-mono">
                    {testResults[credential.service].error}
                  </span>
                </div>
              )}
              {discoveries[credential.service] &&
                discoveries[credential.service].status !== "loading" &&
                (discoveries[credential.service] as { open: boolean }).open && (
                  <div className="mt-3 rounded-2xl border border-sky-400/20 bg-sky-500/[0.04] p-4">
                    {discoveries[credential.service].status === "failed" && (
                      <div className="text-xs text-rose-300">
                        Discovery failed:{" "}
                        {(discoveries[credential.service] as { error: string }).error}
                      </div>
                    )}
                    {discoveries[credential.service].status === "ready" && (
                      <>
                        <div className="mb-2 text-[10px] font-semibold uppercase tracking-[0.22em] text-slate-400">
                          {(discoveries[credential.service] as { models: string[] }).models.length}{" "}
                          model
                          {(discoveries[credential.service] as { models: string[] }).models
                            .length === 1
                            ? ""
                            : "s"}{" "}
                          available · click to set as active
                        </div>
                        <div className="flex flex-wrap gap-2">
                          {(discoveries[credential.service] as { models: string[] }).models.length ===
                          0 ? (
                            <span className="text-xs text-slate-500">
                              Provider returned an empty model list.
                            </span>
                          ) : (
                            (discoveries[credential.service] as { models: string[] }).models.map(
                              (model) => {
                                const isActive = credential.extras.model === model;
                                return (
                                  <button
                                    key={model}
                                    type="button"
                                    disabled={busy || isActive}
                                    onClick={() => {
                                      void setModelForCredential(credential, model);
                                    }}
                                    className={`rounded-full border px-3 py-1 text-[11px] font-mono transition disabled:cursor-default ${
                                      isActive
                                        ? "border-teal-300/50 bg-teal-500/20 text-teal-100"
                                        : "border-white/10 bg-white/5 text-slate-300 hover:border-teal-300/40 hover:bg-teal-500/10 hover:text-teal-100"
                                    }`}
                                  >
                                    {isActive ? `★ ${model}` : model}
                                  </button>
                                );
                              },
                            )
                          )}
                        </div>
                      </>
                    )}
                  </div>
                )}
              <div className="mt-3 grid gap-2 text-xs text-slate-400 md:grid-cols-2">
                <div>
                  base_url · {credential.base_url || "(default for service)"}
                </div>
                <div>updated · {credential.updated_at || "-"}</div>
                <div>model · {credential.extras.model || "(not set)"}</div>
                <div className="md:col-span-2">
                  extras ·{" "}
                  {Object.keys(credential.extras).length === 0
                    ? "(none)"
                    : Object.keys(credential.extras).join(", ")}
                </div>
              </div>
            </article>
          ))}
        </div>
      )}
    </section>
  );
}
