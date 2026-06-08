import React, { useEffect, useMemo, useRef, useState } from "react";

interface ToolInvocation {
  invocation_id: string;
  capability: string;
  arguments: unknown;
  result?: unknown;
  error?: string | null;
  duration_ms: number;
}

interface RecalledFact {
  fact: {
    id: string;
    subject: string;
    predicate: string;
    object: string;
    source: string;
    confidence: number;
  };
  score: number;
}

interface RagHitSummary {
  collection: string;
  document_id: string;
  title: string;
  chunk_index: number;
  score: number;
  snippet: string;
}

interface Turn {
  id: string;
  session_id: string;
  index: number;
  created_at: string;
  user_message: string;
  assistant_response: string;
  context: {
    facts: RecalledFact[];
    rag_hits: RagHitSummary[];
    tool_calls: ToolInvocation[];
    history_window: number;
  };
  model: string | null;
  credential_service: string | null;
}

interface Session {
  id: string;
  created_at: string;
  updated_at: string;
  title: string | null;
  turn_count: number;
}

interface SessionWithTurns {
  session: Session;
  turns: Turn[];
}

type LiveEvent =
  | { event: "subscribed"; session_id: string }
  | { event: "turn_started"; session_id: string; user_message: string }
  | {
      event: "context_retrieved";
      session_id: string;
      facts: RecalledFact[];
      rag_hits: RagHitSummary[];
    }
  | {
      event: "tool_call_started";
      session_id: string;
      invocation_id: string;
      capability: string;
      arguments: unknown;
    }
  | {
      event: "tool_call_completed";
      session_id: string;
      invocation_id: string;
      capability: string;
      result: unknown;
    }
  | {
      event: "tool_call_failed";
      session_id: string;
      invocation_id: string;
      capability: string;
      error: string;
    }
  | { event: "turn_completed"; session_id: string; turn: Turn }
  | { event: "turn_failed"; session_id: string; error: string }
  // Push 5â€“6 additions.
  | { event: "token_delta"; session_id: string; delta: string }
  | {
      event: "review_requested";
      session_id: string;
      review_request_id: string;
      draft: string;
    }
  | {
      event: "review_resolved";
      session_id: string;
      outcome: {
        review_request_id: string;
        state: string;
        delivered_content: string;
        note: string | null;
      };
    }
  | { event: "lagged"; skipped: number };

const CONTROL_API_ORIGIN =
  (typeof window !== "undefined" &&
    (window as Window & { __ORDO_CONTROL_ORIGIN__?: string }).__ORDO_CONTROL_ORIGIN__) ||
  "http://127.0.0.1:4141";

function wsUrlFor(origin: string, sessionId: string): string {
  const url = new URL(
    `/ws/assistant/${encodeURIComponent(sessionId)}`,
    origin,
  );
  url.protocol = url.protocol === "https:" ? "wss:" : "ws:";
  return url.toString();
}

export function AssistantView() {
  const [sessions, setSessions] = useState<Session[]>([]);
  const [currentSessionId, setCurrentSessionId] = useState<string | null>(null);
  const [turns, setTurns] = useState<Turn[]>([]);
  const [draft, setDraft] = useState("");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [liveEvents, setLiveEvents] = useState<LiveEvent[]>([]);
  const [profileFacts, setProfileFacts] = useState<RecalledFact["fact"][]>([]);
  const socketRef = useRef<WebSocket | null>(null);

  // Initial load: list sessions and pull profile facts.
  useEffect(() => {
    void loadSessions();
    void loadFacts();
  }, []);

  // Subscribe to /ws/assistant/:session whenever the session changes.
  useEffect(() => {
    socketRef.current?.close();
    setLiveEvents([]);
    if (!currentSessionId) return;
    const socket = new WebSocket(wsUrlFor(CONTROL_API_ORIGIN, currentSessionId));
    socketRef.current = socket;
    socket.onmessage = (event) => {
      try {
        const parsed = JSON.parse(event.data) as LiveEvent;
        setLiveEvents((prev) => [...prev.slice(-49), parsed]);
        if (parsed.event === "turn_completed") {
          // Reload the canonical turn list so the persisted form
          // wins over the streaming preview.
          void loadSession(parsed.session_id);
          void loadFacts();
        }
      } catch {
        // ignore parse failures
      }
    };
    return () => {
      socket.close();
    };
  }, [currentSessionId]);

  async function loadSessions() {
    try {
      const response = await fetch(`${CONTROL_API_ORIGIN}/api/assistant/sessions`);
      if (!response.ok) throw new Error(`sessions returned ${response.status}`);
      const payload = (await response.json()) as { sessions: Session[] };
      setSessions(payload.sessions);
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err));
    }
  }

  async function loadSession(id: string) {
    try {
      const response = await fetch(
        `${CONTROL_API_ORIGIN}/api/assistant/sessions/${encodeURIComponent(id)}`,
      );
      if (!response.ok) throw new Error(`session returned ${response.status}`);
      const payload = (await response.json()) as SessionWithTurns;
      setTurns(payload.turns);
      setSessions((prev) => {
        const without = prev.filter((s) => s.id !== payload.session.id);
        return [payload.session, ...without];
      });
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err));
    }
  }

  async function loadFacts() {
    try {
      const response = await fetch(`${CONTROL_API_ORIGIN}/api/assistant/facts`);
      if (!response.ok) return;
      const payload = (await response.json()) as { facts: RecalledFact["fact"][] };
      setProfileFacts(payload.facts);
    } catch {
      /* noop */
    }
  }

  async function startNewSession() {
    setCurrentSessionId(null);
    setTurns([]);
    setLiveEvents([]);
  }

  async function sendTurn() {
    if (!draft.trim() || busy) return;
    setBusy(true);
    setError(null);
    try {
      const body: Record<string, unknown> = { user_message: draft };
      if (currentSessionId) body.session_id = currentSessionId;
      const response = await fetch(`${CONTROL_API_ORIGIN}/api/assistant/turn`, {
        method: "POST",
        headers: { "content-type": "application/json" },
        body: JSON.stringify(body),
      });
      if (!response.ok) {
        const payload = await response.json().catch(() => ({}));
        throw new Error(
          (payload as { error?: string }).error ??
            `turn returned ${response.status}`,
        );
      }
      const payload = (await response.json()) as {
        session_id: string;
        turn: Turn;
      };
      setCurrentSessionId(payload.session_id);
      setTurns((prev) => [...prev, payload.turn]);
      setDraft("");
      await loadSessions();
      await loadFacts();
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err));
    } finally {
      setBusy(false);
    }
  }

  async function forgetFact(id: string) {
    try {
      await fetch(
        `${CONTROL_API_ORIGIN}/api/assistant/facts/${encodeURIComponent(id)}`,
        { method: "DELETE" },
      );
      await loadFacts();
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err));
    }
  }

  const liveTimeline = useMemo(
    () => liveEvents.slice(-12).reverse(),
    [liveEvents],
  );

  return (
    <section className="liquid-panel relative grid grid-cols-1 gap-4 rounded-[2.75rem] border p-6 lg:grid-cols-[1fr_320px]">
      <div className="flex h-[calc(100vh-200px)] flex-col">
        <header className="flex items-center justify-between border-b border-white/10 pb-3">
          <div>
            <div className="text-[11px] font-semibold uppercase tracking-[0.36em] text-slate-400">
              Assistant
            </div>
            <h2 className="mt-1 text-2xl font-light tracking-tight text-white">
              {currentSessionId
                ? sessions.find((s) => s.id === currentSessionId)?.title ??
                  "Session"
                : "New session"}
            </h2>
          </div>
          <div className="flex gap-2">
            <button
              onClick={() => void startNewSession()}
              className="rounded-full border border-white/15 bg-white/5 px-3 py-1.5 text-[11px] font-semibold uppercase tracking-[0.2em] text-slate-300 transition hover:border-white/30"
            >
              New
            </button>
            <details className="relative">
              <summary className="cursor-pointer list-none rounded-full border border-white/15 bg-white/5 px-3 py-1.5 text-[11px] font-semibold uppercase tracking-[0.2em] text-slate-300 transition hover:border-white/30">
                Sessions ({sessions.length})
              </summary>
              <div className="absolute right-0 top-full z-10 mt-2 max-h-72 w-72 overflow-auto rounded-2xl border border-white/15 bg-[#0c1521] p-2 shadow-xl">
                {sessions.length === 0 && (
                  <div className="px-2 py-1 text-xs text-slate-400">
                    No sessions yet.
                  </div>
                )}
                {sessions.map((session) => (
                  <button
                    key={session.id}
                    onClick={() => {
                      setCurrentSessionId(session.id);
                      void loadSession(session.id);
                    }}
                    className={`block w-full rounded-xl px-2 py-1.5 text-left text-xs transition ${
                      session.id === currentSessionId
                        ? "bg-teal-500/15 text-teal-100"
                        : "text-slate-300 hover:bg-white/5"
                    }`}
                  >
                    <div className="font-semibold">
                      {session.title ?? "(untitled)"}
                    </div>
                    <div className="text-[10px] uppercase tracking-[0.2em] text-slate-500">
                      {session.turn_count} turns Â·{" "}
                      {new Date(session.updated_at).toLocaleString()}
                    </div>
                  </button>
                ))}
              </div>
            </details>
          </div>
        </header>

        <div className="flex-1 overflow-y-auto py-4">
          {turns.length === 0 && (
            <div className="rounded-2xl border border-dashed border-white/10 bg-white/[0.02] p-6 text-center text-sm text-slate-400">
              Start a conversation. The Assistant remembers what you tell it
              across sessions, pulls from the local RAG when relevant, and can
              call platform capabilities on its own.
            </div>
          )}
          {turns.map((turn) => (
            <article key={turn.id} className="mb-6">
              <div className="ml-auto max-w-2xl rounded-2xl border border-white/10 bg-white/[0.04] px-4 py-3 text-sm text-slate-100">
                <div className="text-[10px] uppercase tracking-[0.2em] text-slate-500">
                  you
                </div>
                <div className="mt-1 whitespace-pre-wrap">{turn.user_message}</div>
              </div>
              <div className="mt-2 max-w-2xl rounded-2xl border border-teal-300/20 bg-teal-500/[0.05] px-4 py-3 text-sm text-slate-100">
                <div className="flex items-center justify-between text-[10px] uppercase tracking-[0.2em] text-teal-300">
                  <span>assistant</span>
                  <span className="text-slate-500">
                    {turn.model ?? ""} Â· {turn.credential_service ?? ""}
                  </span>
                </div>
                <div className="mt-1 whitespace-pre-wrap">
                  {turn.assistant_response || "(no reply)"}
                </div>
                {(turn.context.tool_calls.length > 0 ||
                  turn.context.facts.length > 0 ||
                  turn.context.rag_hits.length > 0) && (
                  <div className="mt-3 grid gap-1 border-t border-white/5 pt-2 text-[10px] uppercase tracking-[0.2em] text-slate-500">
                    {turn.context.facts.length > 0 && (
                      <span>
                        grounded on {turn.context.facts.length} fact(s)
                      </span>
                    )}
                    {turn.context.rag_hits.length > 0 && (
                      <span>
                        consulted {turn.context.rag_hits.length} rag hit(s)
                      </span>
                    )}
                    {turn.context.tool_calls.length > 0 && (
                      <span>
                        called {turn.context.tool_calls.length} tool(s):{" "}
                        {turn.context.tool_calls
                          .map((c) => c.capability)
                          .join(", ")}
                      </span>
                    )}
                  </div>
                )}
              </div>
            </article>
          ))}
          {busy && (
            <div className="text-xs text-teal-300">assistant is thinkingâ€¦</div>
          )}
          {error && (
            <div className="rounded-2xl border border-rose-400/30 bg-rose-500/10 p-3 text-xs text-rose-200">
              {error}
            </div>
          )}
        </div>

        <form
          onSubmit={(event) => {
            event.preventDefault();
            void sendTurn();
          }}
          className="flex gap-2 border-t border-white/10 pt-3"
        >
          <textarea
            value={draft}
            onChange={(event) => setDraft(event.target.value)}
            onKeyDown={(event) => {
              if (event.key === "Enter" && !event.shiftKey) {
                event.preventDefault();
                void sendTurn();
              }
            }}
            placeholder="Tell the assistant what to doâ€¦"
            rows={2}
            className="flex-1 rounded-2xl border border-white/10 bg-black/30 px-4 py-2 text-sm text-slate-100 outline-none focus:border-teal-300/40"
          />
          <button
            type="submit"
            disabled={busy || draft.trim().length === 0}
            className="rounded-full border border-teal-300/30 bg-teal-500/15 px-5 py-2 text-xs font-semibold uppercase tracking-[0.2em] text-teal-100 transition hover:border-teal-300/50 hover:bg-teal-500/25 disabled:opacity-50"
          >
            Send
          </button>
        </form>
      </div>

      <aside className="flex h-[calc(100vh-200px)] flex-col gap-4 overflow-y-auto">
        <section className="rounded-2xl border border-white/10 bg-white/[0.02] p-3">
          <div className="text-[10px] uppercase tracking-[0.2em] text-slate-500">
            Live progress
          </div>
          {liveTimeline.length === 0 && (
            <div className="mt-2 text-xs text-slate-400">
              {currentSessionId
                ? "Idle. Send a turn to see what the assistant does."
                : "Start a session and send a message."}
            </div>
          )}
          <ul className="mt-2 space-y-1.5 text-xs">
            {liveTimeline.map((event, idx) => (
              <li key={idx} className="text-slate-300">
                <span className="font-mono text-[10px] uppercase tracking-[0.2em] text-slate-500">
                  {event.event}
                </span>{" "}
                {"capability" in event && (
                  <span className="font-semibold text-teal-200">
                    {event.capability}
                  </span>
                )}
                {"error" in event && event.error && (
                  <span className="text-rose-300"> {event.error}</span>
                )}
              </li>
            ))}
          </ul>
        </section>

        <section className="rounded-2xl border border-white/10 bg-white/[0.02] p-3">
          <div className="flex items-center justify-between">
            <span className="text-[10px] uppercase tracking-[0.2em] text-slate-500">
              Profile facts ({profileFacts.length})
            </span>
            <button
              onClick={() => void loadFacts()}
              className="text-[10px] uppercase tracking-[0.2em] text-slate-400 hover:text-slate-200"
            >
              refresh
            </button>
          </div>
          {profileFacts.length === 0 && (
            <div className="mt-2 text-xs text-slate-400">
              The assistant hasn't been told (or learned) anything about you
              yet.
            </div>
          )}
          <ul className="mt-2 space-y-1.5 text-xs">
            {profileFacts.map((fact) => (
              <li
                key={fact.id}
                className="flex items-start justify-between gap-2 rounded-xl border border-white/5 bg-black/20 p-2"
              >
                <div>
                  <div className="text-[10px] uppercase tracking-[0.2em] text-slate-500">
                    {fact.subject} Â· {fact.predicate} Â·{" "}
                    {(fact.confidence * 100).toFixed(0)}%
                  </div>
                  <div className="mt-1 text-slate-200">{fact.object}</div>
                  <div className="mt-1 text-[10px] uppercase tracking-[0.2em] text-slate-500">
                    {fact.source}
                  </div>
                </div>
                <button
                  onClick={() => void forgetFact(fact.id)}
                  className="text-[10px] uppercase tracking-[0.2em] text-rose-300 hover:text-rose-100"
                >
                  forget
                </button>
              </li>
            ))}
          </ul>
        </section>
      </aside>
    </section>
  );
}
