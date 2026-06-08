import React, { useEffect, useMemo, useState } from "react";

interface PluginManifestRow {
  name: string;
  version: string;
  enabled: boolean;
  description: string;
  expected_lanes: string[];
  manifest_path: string;
}

interface PluginErrorRow {
  manifest_path: string;
  error: string;
}

interface PluginLiveRow {
  name: string;
  version: string;
  tool_count: number;
  capabilities: string[];
  manifest_path: string;
  state: "active" | "disabled" | "failed" | "invalid";
  state_detail: string | null;
}

interface PluginsResponse {
  plugins_dir: string | null;
  loaded: PluginManifestRow[];
  errors: PluginErrorRow[];
  live: PluginLiveRow[];
}

const CONTROL_API_ORIGIN =
  (typeof window !== "undefined" &&
    (window as Window & { __ORDO_CONTROL_ORIGIN__?: string }).__ORDO_CONTROL_ORIGIN__) ||
  "http://127.0.0.1:4141";

const STATE_STYLES: Record<string, { label: string; color: string; bg: string }> = {
  active: { label: "active", color: "#5eead4", bg: "rgba(94, 234, 212, 0.12)" },
  disabled: { label: "disabled", color: "#94a3b8", bg: "rgba(148, 163, 184, 0.12)" },
  failed: { label: "failed", color: "#fb7185", bg: "rgba(251, 113, 133, 0.12)" },
  invalid: { label: "invalid", color: "#fbbf24", bg: "rgba(251, 191, 36, 0.12)" },
};

export function PluginsView() {
  const [state, setState] = useState<
    | { status: "loading" }
    | { status: "error"; message: string }
    | { status: "ready"; data: PluginsResponse }
  >({ status: "loading" });
  const [busy, setBusy] = useState(false);
  const [message, setMessage] = useState<{ tone: "ok" | "warn" | "err"; text: string } | null>(
    null,
  );

  async function refresh() {
    setState({ status: "loading" });
    try {
      const response = await fetch(`${CONTROL_API_ORIGIN}/api/plugins`);
      if (!response.ok) {
        throw new Error(`control API returned ${response.status}`);
      }
      const data = (await response.json()) as PluginsResponse;
      setState({ status: "ready", data });
    } catch (error) {
      setState({
        status: "error",
        message:
          error instanceof Error
            ? error.message
            : `control API is unreachable at ${CONTROL_API_ORIGIN}`,
      });
    }
  }

  useEffect(() => {
    void refresh();
  }, []);

  async function toggle(name: string, enabled: boolean) {
    setBusy(true);
    setMessage({
      tone: "ok",
      text: `${enabled ? "Enabling" : "Disabling"} ${name}...`,
    });
    try {
      const response = await fetch(
        `${CONTROL_API_ORIGIN}/api/plugins/${encodeURIComponent(name)}/enabled`,
        {
          method: "POST",
          headers: { "content-type": "application/json" },
          body: JSON.stringify({ enabled }),
        },
      );
      if (!response.ok) {
        const text = await response.text();
        throw new Error(text || `control API returned ${response.status}`);
      }
      const payload = await response.json();
      setMessage({
        tone: "ok",
        text: `${name} is now ${enabled ? "enabled" : "disabled"}. ${payload.note ?? ""}`,
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

  const liveByName = useMemo(() => {
    if (state.status !== "ready") return new Map<string, PluginLiveRow>();
    return new Map(state.data.live.map((entry) => [entry.name, entry]));
  }, [state]);

  return (
    <section className="liquid-panel relative overflow-hidden rounded-[2.75rem] border p-10">
      <div className="flex flex-wrap items-start justify-between gap-6">
        <div>
          <div className="text-[11px] font-semibold uppercase tracking-[0.36em] text-slate-400">
            Plugins
          </div>
          <h2 className="mt-3 text-4xl font-light tracking-tight text-white">
            Extend the runtime without rebuilding it
          </h2>
          <p className="mt-2 max-w-2xl text-sm leading-7 text-slate-400">
            Plugins are MCP servers (JSON-RPC over stdio) that contribute new
            capabilities to the shared bus. Drop a folder with a{" "}
            <code className="rounded bg-white/10 px-1.5 py-0.5 text-[11px] text-slate-200">
              plugin.json
            </code>{" "}
            into the plugins directory, enable it here, and restart the
            runtime. Reserved lanes like <code>cloud.*</code> and{" "}
            <code>runtime.*</code> are always core-only.
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

      {state.status === "loading" && (
        <div className="mt-8 rounded-2xl border border-white/10 bg-white/5 p-6 text-sm text-slate-300">
          Loading plugin inventory...
        </div>
      )}
      {state.status === "error" && (
        <div className="mt-8 rounded-2xl border border-rose-400/30 bg-rose-500/10 p-6 text-sm text-rose-200">
          {state.message}
        </div>
      )}

      {state.status === "ready" && (
        <>
          <div className="mt-6 flex flex-wrap items-center gap-3 text-xs text-slate-400">
            <div>
              Plugins directory:{" "}
              <code className="rounded bg-white/10 px-2 py-0.5 text-slate-200">
                {state.data.plugins_dir ?? "(not configured)"}
              </code>
            </div>
            <div>
              Installed: <strong className="text-slate-200">{state.data.loaded.length}</strong>
            </div>
            <div>
              Live: <strong className="text-slate-200">{state.data.live.length}</strong>
            </div>
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

          {state.data.loaded.length === 0 && state.data.errors.length === 0 && (
            <div className="mt-6 rounded-2xl border border-white/10 bg-white/5 p-6 text-sm text-slate-300">
              No plugins installed yet. Point{" "}
              <code className="rounded bg-white/10 px-1.5 py-0.5 text-[11px] text-slate-200">
                ORDO_PLUGINS_PATH
              </code>{" "}
              at a directory of <code>plugin.json</code> manifests, or copy one
              into the location above and refresh.
            </div>
          )}

          <div className="mt-6 grid gap-4">
            {state.data.loaded.map((manifest) => {
              const live = liveByName.get(manifest.name);
              const stateKey =
                live?.state ?? (manifest.enabled ? "failed" : "disabled");
              const style = STATE_STYLES[stateKey] ?? STATE_STYLES.disabled;
              return (
                <article
                  key={manifest.manifest_path}
                  className="rounded-3xl border border-white/10 bg-white/[0.03] p-5"
                >
                  <div className="flex flex-wrap items-start justify-between gap-3">
                    <div>
                      <div className="flex items-center gap-3">
                        <span className="text-sm font-semibold text-slate-100">
                          {manifest.name}
                        </span>
                        <span className="text-[11px] uppercase tracking-[0.22em] text-slate-500">
                          v{manifest.version}
                        </span>
                        <span
                          className="rounded-full border px-2 py-0.5 text-[10px] font-semibold uppercase tracking-[0.18em]"
                          style={{
                            borderColor: `${style.color}55`,
                            color: style.color,
                            background: style.bg,
                          }}
                        >
                          {style.label}
                        </span>
                      </div>
                      {manifest.description && (
                        <p className="mt-2 text-sm text-slate-300">
                          {manifest.description}
                        </p>
                      )}
                      <div className="mt-2 text-[11px] uppercase tracking-[0.22em] text-slate-500">
                        expected lanes Â· {manifest.expected_lanes.length === 0 ? "(none)" : manifest.expected_lanes.join(", ")}
                      </div>
                      {live && live.capabilities.length > 0 && (
                        <div className="mt-2 text-xs text-slate-400">
                          capabilities: {live.capabilities.join(", ")}
                        </div>
                      )}
                      {live?.state_detail && (
                        <div className="mt-2 text-xs text-rose-300">
                          {live.state_detail}
                        </div>
                      )}
                      <div className="mt-2 text-[10px] uppercase tracking-[0.22em] text-slate-500">
                        {manifest.manifest_path}
                      </div>
                    </div>
                    <button
                      type="button"
                      disabled={busy}
                      onClick={() => {
                        void toggle(manifest.name, !manifest.enabled);
                      }}
                      className={`rounded-full border px-4 py-1.5 text-[11px] font-semibold uppercase tracking-[0.2em] transition disabled:opacity-50 ${
                        manifest.enabled
                          ? "border-amber-400/30 bg-amber-500/10 text-amber-200 hover:border-amber-400/50 hover:bg-amber-500/20"
                          : "border-teal-400/30 bg-teal-500/10 text-teal-200 hover:border-teal-400/50 hover:bg-teal-500/20"
                      }`}
                    >
                      {manifest.enabled ? "Disable" : "Enable"}
                    </button>
                  </div>
                </article>
              );
            })}

            {state.data.errors.map((err) => (
              <article
                key={err.manifest_path}
                className="rounded-3xl border border-rose-400/20 bg-rose-500/5 p-5"
              >
                <div className="text-sm font-semibold text-rose-200">
                  Invalid manifest
                </div>
                <div className="mt-1 text-xs text-rose-300">{err.error}</div>
                <div className="mt-2 text-[10px] uppercase tracking-[0.22em] text-rose-300/80">
                  {err.manifest_path}
                </div>
              </article>
            ))}
          </div>
        </>
      )}
    </section>
  );
}
