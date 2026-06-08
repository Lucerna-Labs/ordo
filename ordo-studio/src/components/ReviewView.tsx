import React, { useEffect, useMemo, useRef, useState } from "react";

type ReviewState =
  | "open"
  | "approved"
  | "edited_and_approved"
  | "denied"
  | "expired";

interface ReviewRequest {
  id: string;
  created_at: string;
  resolved_at: string | null;
  origin_capability: string;
  origin_plugin: string | null;
  title: string;
  content_type: string;
  content: string;
  metadata: Record<string, unknown>;
  state: ReviewState;
  edited_content: string | null;
  decision_note: string | null;
}

type ReviewEvent =
  | { event: "opened"; request: ReviewRequest }
  | { event: "resolved"; request: ReviewRequest }
  | { event: "queue_snapshot"; pending: ReviewRequest[]; total: number }
  | { event: "lagged"; skipped: number };

const CONTROL_API_ORIGIN =
  (typeof window !== "undefined" &&
    (window as Window & { __ORDO_CONTROL_ORIGIN__?: string }).__ORDO_CONTROL_ORIGIN__) ||
  "http://127.0.0.1:4141";

function wsUrlFrom(origin: string): string {
  const url = new URL("/ws/review", origin);
  url.protocol = url.protocol === "https:" ? "wss:" : "ws:";
  return url.toString();
}

const STATE_STYLES: Record<ReviewState, { label: string; color: string; bg: string }> = {
  open: { label: "awaiting", color: "#fbbf24", bg: "rgba(251,191,36,0.12)" },
  approved: { label: "approved", color: "#5eead4", bg: "rgba(94,234,212,0.12)" },
  edited_and_approved: {
    label: "edited + approved",
    color: "#60a5fa",
    bg: "rgba(96,165,250,0.12)",
  },
  denied: { label: "denied", color: "#fb7185", bg: "rgba(251,113,133,0.12)" },
  expired: { label: "expired", color: "#94a3b8", bg: "rgba(148,163,184,0.12)" },
};

function StateBadge({ state }: { state: ReviewState }) {
  const style = STATE_STYLES[state];
  return (
    <span
      className="rounded-full border px-2 py-0.5 text-[10px] font-semibold uppercase tracking-[0.18em]"
      style={{ borderColor: `${style.color}55`, color: style.color, background: style.bg }}
    >
      {style.label}
    </span>
  );
}

function renderMarkdown(md: string): string {
  // Intentionally minimal: headings, **bold**, *italic*, `code`, links,
  // lists, paragraphs. Escaped first, then a few deterministic
  // substitutions. This lives inside the Review tab only â€” we don't
  // need a full markdown engine in the bundle for an MVP.
  const escape = (s: string) =>
    s
      .replace(/&/g, "&amp;")
      .replace(/</g, "&lt;")
      .replace(/>/g, "&gt;")
      .replace(/"/g, "&quot;");
  let html = escape(md);
  html = html.replace(/^### (.*)$/gm, "<h3>$1</h3>");
  html = html.replace(/^## (.*)$/gm, "<h2>$1</h2>");
  html = html.replace(/^# (.*)$/gm, "<h1>$1</h1>");
  html = html.replace(/\*\*(.+?)\*\*/g, "<strong>$1</strong>");
  html = html.replace(/(?<!\*)\*(?!\*)(.+?)(?<!\*)\*(?!\*)/g, "<em>$1</em>");
  html = html.replace(/`([^`]+)`/g, "<code>$1</code>");
  html = html.replace(/\[([^\]]+)\]\(([^)]+)\)/g, '<a href="$2" target="_blank" rel="noreferrer">$1</a>');
  // lists
  html = html.replace(/(^|\n)- (.+)(?=\n|$)/g, "$1<li>$2</li>");
  html = html.replace(/(<li>.*<\/li>\n?)+/g, (chunk) => `<ul>${chunk.replace(/\n/g, "")}</ul>`);
  // paragraphs
  html = html
    .split(/\n{2,}/)
    .map((block) => (block.match(/^<(h\d|ul|ol|pre|blockquote|li)/) ? block : `<p>${block.replace(/\n/g, "<br>")}</p>`))
    .join("\n");
  return html;
}

function ContentPreview({ request }: { request: ReviewRequest }) {
  const effective = request.edited_content ?? request.content;
  const ct = request.content_type.toLowerCase();

  if (ct === "text/markdown") {
    return (
      <div
        className="prose prose-invert max-w-none rounded-2xl border border-white/10 bg-black/20 p-4 text-sm text-slate-100"
        dangerouslySetInnerHTML={{ __html: renderMarkdown(effective) }}
      />
    );
  }
  if (ct === "text/html") {
    return (
      <iframe
        sandbox=""
        title={`review-${request.id}`}
        srcDoc={effective}
        className="h-96 w-full rounded-2xl border border-white/10 bg-white"
      />
    );
  }
  if (ct === "application/json") {
    let pretty = effective;
    try {
      pretty = JSON.stringify(JSON.parse(effective), null, 2);
    } catch {
      // pass through
    }
    return (
      <pre className="overflow-auto rounded-2xl border border-white/10 bg-black/30 p-4 font-mono text-xs text-slate-100">
        {pretty}
      </pre>
    );
  }
  if (ct.startsWith("image/")) {
    const src = effective.startsWith("data:") ? effective : `data:${ct};base64,${effective}`;
    return (
      <img
        src={src}
        alt={request.title}
        className="max-h-96 rounded-2xl border border-white/10 bg-black/30"
      />
    );
  }
  // text/plain and fallback
  return (
    <pre className="whitespace-pre-wrap rounded-2xl border border-white/10 bg-black/30 p-4 font-mono text-xs text-slate-100">
      {effective}
    </pre>
  );
}

export function ReviewView() {
  const [pending, setPending] = useState<ReviewRequest[]>([]);
  const [recent, setRecent] = useState<ReviewRequest[]>([]);
  const [selectedId, setSelectedId] = useState<string | null>(null);
  const [connectionState, setConnectionState] = useState<"connecting" | "open" | "closed">(
    "connecting",
  );
  const [message, setMessage] = useState<{ tone: "ok" | "err"; text: string } | null>(null);
  const [busy, setBusy] = useState(false);
  const [editBuffer, setEditBuffer] = useState<string | null>(null);
  const socketRef = useRef<WebSocket | null>(null);

  useEffect(() => {
    let cancelled = false;
    async function bootstrap() {
      try {
        const response = await fetch(`${CONTROL_API_ORIGIN}/api/review/recent?limit=40`);
        if (!response.ok) throw new Error(`recent returned ${response.status}`);
        const payload = (await response.json()) as {
          count: number;
          recent: ReviewRequest[];
        };
        if (cancelled) return;
        const allPending = payload.recent.filter((r) => r.state === "open");
        const allResolved = payload.recent.filter((r) => r.state !== "open");
        setPending(allPending);
        setRecent(allResolved);
      } catch (error) {
        if (!cancelled) {
          setMessage({
            tone: "err",
            text: error instanceof Error ? error.message : String(error),
          });
        }
      }
    }
    void bootstrap();

    const socket = new WebSocket(wsUrlFrom(CONTROL_API_ORIGIN));
    socketRef.current = socket;
    socket.onopen = () => {
      setConnectionState("open");
    };
    socket.onclose = () => {
      setConnectionState("closed");
    };
    socket.onerror = () => {
      setConnectionState("closed");
    };
    socket.onmessage = (event) => {
      let parsed: ReviewEvent;
      try {
        parsed = JSON.parse(event.data) as ReviewEvent;
      } catch {
        return;
      }
      if (parsed.event === "queue_snapshot") {
        setPending(parsed.pending);
      } else if (parsed.event === "opened") {
        setPending((prev) => {
          if (prev.some((r) => r.id === parsed.request.id)) return prev;
          return [...prev, parsed.request];
        });
      } else if (parsed.event === "resolved") {
        setPending((prev) => prev.filter((r) => r.id !== parsed.request.id));
        setRecent((prev) => [parsed.request, ...prev].slice(0, 40));
      }
    };

    return () => {
      cancelled = true;
      socket.close();
    };
  }, []);

  const selected = useMemo(() => {
    if (!selectedId) return pending[0] ?? null;
    return (
      pending.find((r) => r.id === selectedId) ||
      recent.find((r) => r.id === selectedId) ||
      null
    );
  }, [selectedId, pending, recent]);

  useEffect(() => {
    // Reset the edit buffer any time the selection changes.
    setEditBuffer(null);
  }, [selected?.id]);

  async function decide(path: "approve" | "deny", note?: string) {
    if (!selected) return;
    setBusy(true);
    setMessage({ tone: "ok", text: `Sending ${path}...` });
    try {
      const response = await fetch(
        `${CONTROL_API_ORIGIN}/api/review/${selected.id}/${path}`,
        {
          method: "POST",
          headers: { "content-type": "application/json" },
          body: JSON.stringify({ note: note ?? null }),
        },
      );
      if (!response.ok) {
        throw new Error(await response.text());
      }
      setMessage({ tone: "ok", text: `${path}d` });
    } catch (error) {
      setMessage({
        tone: "err",
        text: error instanceof Error ? error.message : String(error),
      });
    } finally {
      setBusy(false);
    }
  }

  async function submitEdit() {
    if (!selected || editBuffer === null) return;
    setBusy(true);
    setMessage({ tone: "ok", text: "Saving edit..." });
    try {
      const response = await fetch(
        `${CONTROL_API_ORIGIN}/api/review/${selected.id}/edit`,
        {
          method: "POST",
          headers: { "content-type": "application/json" },
          body: JSON.stringify({ content: editBuffer, note: null }),
        },
      );
      if (!response.ok) {
        throw new Error(await response.text());
      }
      setMessage({ tone: "ok", text: "Edited + approved" });
      setEditBuffer(null);
    } catch (error) {
      setMessage({
        tone: "err",
        text: error instanceof Error ? error.message : String(error),
      });
    } finally {
      setBusy(false);
    }
  }

  const connectionBadge =
    connectionState === "open"
      ? { text: "live", color: "#5eead4", bg: "rgba(94,234,212,0.15)" }
      : connectionState === "connecting"
        ? { text: "connecting", color: "#fbbf24", bg: "rgba(251,191,36,0.15)" }
        : { text: "offline", color: "#fb7185", bg: "rgba(251,113,133,0.15)" };

  return (
    <section className="liquid-panel relative overflow-hidden rounded-[2.75rem] border p-10">
      <div className="flex flex-wrap items-start justify-between gap-6">
        <div>
          <div className="text-[11px] font-semibold uppercase tracking-[0.36em] text-slate-400">
            Review
          </div>
          <h2 className="mt-3 text-4xl font-light tracking-tight text-white">
            Approve, deny, or edit before it ships
          </h2>
          <p className="mt-2 max-w-2xl text-sm leading-7 text-slate-400">
            Every time a capability is asked to produce content under review, the
            draft lands here. A live WebSocket pushes new items the moment the
            runtime queues them; decisions are auditable and go back to the waiting
            agent instantly.
          </p>
        </div>
        <div className="flex items-center gap-3">
          <span
            className="rounded-full border px-3 py-1 text-[10px] font-semibold uppercase tracking-[0.2em]"
            style={{
              borderColor: `${connectionBadge.color}55`,
              color: connectionBadge.color,
              background: connectionBadge.bg,
            }}
          >
            ws {connectionBadge.text}
          </span>
          <span className="rounded-full border border-white/10 bg-white/5 px-3 py-1 text-[10px] font-semibold uppercase tracking-[0.2em] text-slate-300">
            pending {pending.length}
          </span>
        </div>
      </div>

      {message && (
        <div
          className={`mt-4 text-xs ${
            message.tone === "err" ? "text-rose-300" : "text-teal-200"
          }`}
        >
          {message.text}
        </div>
      )}

      <div className="mt-6 grid gap-4 lg:grid-cols-[minmax(280px,1fr)_2fr]">
        <div className="grid gap-2">
          <div className="text-[11px] uppercase tracking-[0.22em] text-slate-500">Pending</div>
          {pending.length === 0 && (
            <div className="rounded-2xl border border-white/10 bg-white/5 p-4 text-xs text-slate-400">
              Nothing awaiting review.
            </div>
          )}
          {pending.map((request) => (
            <button
              key={request.id}
              onClick={() => setSelectedId(request.id)}
              className={`rounded-2xl border p-3 text-left transition ${
                selected?.id === request.id
                  ? "border-teal-300/40 bg-teal-500/10"
                  : "border-white/10 bg-white/[0.03] hover:border-white/30"
              }`}
            >
              <div className="flex items-center justify-between gap-2">
                <span className="text-sm font-semibold text-slate-100">{request.title}</span>
                <StateBadge state={request.state} />
              </div>
              <div className="mt-1 text-[10px] uppercase tracking-[0.22em] text-slate-500">
                {request.origin_capability} Â· {new Date(request.created_at).toLocaleTimeString()}
              </div>
            </button>
          ))}

          {recent.length > 0 && (
            <>
              <div className="mt-4 text-[11px] uppercase tracking-[0.22em] text-slate-500">
                Recently resolved
              </div>
              {recent.slice(0, 10).map((request) => (
                <button
                  key={request.id}
                  onClick={() => setSelectedId(request.id)}
                  className={`rounded-2xl border p-3 text-left transition ${
                    selected?.id === request.id
                      ? "border-teal-300/30 bg-teal-500/5"
                      : "border-white/5 bg-white/[0.02] hover:border-white/20"
                  }`}
                >
                  <div className="flex items-center justify-between gap-2">
                    <span className="text-xs font-semibold text-slate-200">{request.title}</span>
                    <StateBadge state={request.state} />
                  </div>
                  <div className="mt-1 text-[10px] uppercase tracking-[0.22em] text-slate-500">
                    {request.origin_capability}
                  </div>
                </button>
              ))}
            </>
          )}
        </div>

        <div className="grid gap-4">
          {!selected && (
            <div className="rounded-3xl border border-white/10 bg-white/[0.03] p-8 text-sm text-slate-400">
              Select a request on the left to preview it. Or wait â€” new items arrive
              automatically.
            </div>
          )}

          {selected && (
            <>
              <div className="rounded-3xl border border-white/10 bg-white/[0.03] p-5">
                <div className="flex flex-wrap items-start justify-between gap-3">
                  <div>
                    <div className="text-lg font-semibold text-slate-100">{selected.title}</div>
                    <div className="mt-1 text-[11px] uppercase tracking-[0.22em] text-slate-500">
                      {selected.origin_capability}
                      {selected.origin_plugin ? ` Â· plugin ${selected.origin_plugin}` : ""} Â·{" "}
                      {selected.content_type}
                    </div>
                    <div className="mt-1 text-[11px] text-slate-500">
                      queued {new Date(selected.created_at).toLocaleString()}
                      {selected.resolved_at
                        ? ` Â· resolved ${new Date(selected.resolved_at).toLocaleString()}`
                        : ""}
                    </div>
                  </div>
                  <StateBadge state={selected.state} />
                </div>
                {selected.decision_note && (
                  <div className="mt-3 text-xs text-slate-400">
                    <strong>Note:</strong> {selected.decision_note}
                  </div>
                )}
              </div>

              {editBuffer === null ? (
                <ContentPreview request={selected} />
              ) : (
                <textarea
                  value={editBuffer}
                  onChange={(event) => setEditBuffer(event.target.value)}
                  className="h-96 w-full rounded-2xl border border-teal-300/40 bg-black/30 p-4 font-mono text-sm text-slate-100 outline-none focus:border-teal-300/70"
                />
              )}

              {selected.state === "open" && (
                <div className="flex flex-wrap gap-3">
                  <button
                    type="button"
                    disabled={busy}
                    onClick={() => {
                      void decide("approve");
                    }}
                    className="rounded-full border border-teal-300/30 bg-teal-500/15 px-5 py-2 text-xs font-semibold uppercase tracking-[0.2em] text-teal-100 transition hover:border-teal-300/50 hover:bg-teal-500/25 disabled:opacity-50"
                  >
                    Approve
                  </button>
                  <button
                    type="button"
                    disabled={busy}
                    onClick={() => {
                      void decide("deny");
                    }}
                    className="rounded-full border border-rose-400/30 bg-rose-500/10 px-5 py-2 text-xs font-semibold uppercase tracking-[0.2em] text-rose-200 transition hover:border-rose-400/50 hover:bg-rose-500/20 disabled:opacity-50"
                  >
                    Deny
                  </button>
                  {editBuffer === null ? (
                    <button
                      type="button"
                      disabled={busy}
                      onClick={() => setEditBuffer(selected.content)}
                      className="rounded-full border border-blue-400/30 bg-blue-500/10 px-5 py-2 text-xs font-semibold uppercase tracking-[0.2em] text-blue-200 transition hover:border-blue-400/50 hover:bg-blue-500/20 disabled:opacity-50"
                    >
                      Edit
                    </button>
                  ) : (
                    <>
                      <button
                        type="button"
                        disabled={busy}
                        onClick={() => {
                          void submitEdit();
                        }}
                        className="rounded-full border border-blue-400/30 bg-blue-500/15 px-5 py-2 text-xs font-semibold uppercase tracking-[0.2em] text-blue-200 transition hover:border-blue-400/50 hover:bg-blue-500/25 disabled:opacity-50"
                      >
                        Save + approve
                      </button>
                      <button
                        type="button"
                        disabled={busy}
                        onClick={() => setEditBuffer(null)}
                        className="rounded-full border border-white/15 bg-white/5 px-5 py-2 text-xs font-semibold uppercase tracking-[0.2em] text-slate-300 transition hover:border-white/30 disabled:opacity-50"
                      >
                        Cancel edit
                      </button>
                    </>
                  )}
                </div>
              )}
            </>
          )}
        </div>
      </div>
    </section>
  );
}
