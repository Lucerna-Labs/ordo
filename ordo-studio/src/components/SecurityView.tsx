import React, { useEffect, useMemo, useState } from "react";

type Severity = "info" | "warn" | "error";
type Verdict = "allow" | "warn" | "block";
type Phase = "pre_call" | "post_call";

interface FindingDecision {
  finding: {
    rule_id: string;
    severity: Severity;
    message: string;
    match_preview: string;
    location: { pointer: string };
  };
  verdict: Verdict;
}

interface AuditEvent {
  id: number;
  timestamp: string;
  phase: Phase;
  plugin: string;
  capability: string;
  verdict: Verdict;
  findings: FindingDecision[];
}

interface RuleDescriptor {
  id: string;
  description: string;
  default_severity: Severity;
  pre_call: boolean;
  post_call: boolean;
}

interface AuditResponse {
  available: boolean;
  count: number;
  events: AuditEvent[];
}

interface RulesResponse {
  available: boolean;
  count: number;
  rules: RuleDescriptor[];
}

const CONTROL_API_ORIGIN =
  (typeof window !== "undefined" &&
    (window as Window & { __ORDO_CONTROL_ORIGIN__?: string }).__ORDO_CONTROL_ORIGIN__) ||
  "http://127.0.0.1:4141";

const VERDICT_STYLES: Record<Verdict, { label: string; color: string; bg: string }> = {
  allow: { label: "allow", color: "#5eead4", bg: "rgba(94, 234, 212, 0.12)" },
  warn: { label: "warn", color: "#fbbf24", bg: "rgba(251, 191, 36, 0.12)" },
  block: { label: "block", color: "#fb7185", bg: "rgba(251, 113, 133, 0.12)" },
};

const SEVERITY_STYLES: Record<Severity, string> = {
  info: "text-teal-300",
  warn: "text-amber-300",
  error: "text-rose-300",
};

export function SecurityView() {
  const [auditState, setAuditState] = useState<
    | { status: "loading" }
    | { status: "error"; message: string }
    | { status: "ready"; data: AuditResponse }
  >({ status: "loading" });
  const [rulesState, setRulesState] = useState<
    | { status: "loading" }
    | { status: "error"; message: string }
    | { status: "ready"; data: RulesResponse }
  >({ status: "loading" });
  const [tab, setTab] = useState<"audit" | "rules">("audit");

  async function refresh() {
    setAuditState({ status: "loading" });
    setRulesState({ status: "loading" });
    try {
      const [auditResponse, rulesResponse] = await Promise.all([
        fetch(`${CONTROL_API_ORIGIN}/api/security/audit?limit=100`),
        fetch(`${CONTROL_API_ORIGIN}/api/security/rules`),
      ]);
      if (!auditResponse.ok) {
        throw new Error(`audit: ${auditResponse.status}`);
      }
      if (!rulesResponse.ok) {
        throw new Error(`rules: ${rulesResponse.status}`);
      }
      setAuditState({
        status: "ready",
        data: (await auditResponse.json()) as AuditResponse,
      });
      setRulesState({
        status: "ready",
        data: (await rulesResponse.json()) as RulesResponse,
      });
    } catch (error) {
      const message =
        error instanceof Error ? error.message : `control API is unreachable`;
      setAuditState({ status: "error", message });
      setRulesState({ status: "error", message });
    }
  }

  useEffect(() => {
    void refresh();
  }, []);

  const verdictCounts = useMemo(() => {
    if (auditState.status !== "ready") return { allow: 0, warn: 0, block: 0 };
    return auditState.data.events.reduce(
      (acc, event) => {
        acc[event.verdict] = (acc[event.verdict] ?? 0) + 1;
        return acc;
      },
      { allow: 0, warn: 0, block: 0 } as Record<Verdict, number>,
    );
  }, [auditState]);

  return (
    <section className="liquid-panel relative overflow-hidden rounded-[2.75rem] border p-10">
      <div className="flex flex-wrap items-start justify-between gap-6">
        <div>
          <div className="text-[11px] font-semibold uppercase tracking-[0.36em] text-slate-400">
            Security
          </div>
          <h2 className="mt-3 text-4xl font-light tracking-tight text-white">
            Classifier-gated tool calls
          </h2>
          <p className="mt-2 max-w-2xl text-sm leading-7 text-slate-400">
            Every plugin tool call is scanned before execution and after it
            returns. Findings with <code>error</code> severity block the call;
            <code>warn</code> records an audit entry but lets it through;{" "}
            <code>info</code> is logged silently. Policy tuning lives in{" "}
            <code>PolicyConfig</code>; per-plugin overrides arrive next.
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

      <div className="mt-6 flex flex-wrap items-center gap-3">
        <div className="flex rounded-full border border-white/10 bg-white/5 p-1">
          {(["audit", "rules"] as const).map((candidate) => (
            <button
              key={candidate}
              onClick={() => setTab(candidate)}
              className={`rounded-full px-4 py-1.5 text-[11px] font-semibold uppercase tracking-[0.2em] transition ${
                tab === candidate
                  ? "bg-teal-500/15 text-teal-100"
                  : "text-slate-400 hover:text-slate-200"
              }`}
            >
              {candidate}
            </button>
          ))}
        </div>
        {auditState.status === "ready" && (
          <div className="flex items-center gap-3 text-xs text-slate-400">
            <span className="text-teal-300">allow {verdictCounts.allow}</span>
            <span className="text-amber-300">warn {verdictCounts.warn}</span>
            <span className="text-rose-300">block {verdictCounts.block}</span>
          </div>
        )}
      </div>

      {tab === "audit" && (
        <div className="mt-6">
          {auditState.status === "loading" && (
            <div className="rounded-2xl border border-white/10 bg-white/5 p-6 text-sm text-slate-300">
              Loading recent audit events...
            </div>
          )}
          {auditState.status === "error" && (
            <div className="rounded-2xl border border-rose-400/30 bg-rose-500/10 p-6 text-sm text-rose-200">
              {auditState.message}
            </div>
          )}
          {auditState.status === "ready" && auditState.data.events.length === 0 && (
            <div className="rounded-2xl border border-white/10 bg-white/5 p-6 text-sm text-slate-300">
              No audit events yet. Gated plugins haven't produced any findings.
            </div>
          )}
          {auditState.status === "ready" && auditState.data.events.length > 0 && (
            <div className="grid gap-3">
              {auditState.data.events
                .slice()
                .reverse()
                .map((event) => {
                  const style = VERDICT_STYLES[event.verdict];
                  return (
                    <article
                      key={event.id}
                      className="rounded-3xl border border-white/10 bg-white/[0.03] p-4"
                    >
                      <div className="flex flex-wrap items-center gap-3">
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
                        <span className="text-sm font-semibold text-slate-100">
                          {event.plugin} Â· {event.capability}
                        </span>
                        <span className="text-[11px] uppercase tracking-[0.22em] text-slate-500">
                          {event.phase === "pre_call" ? "pre-call" : "post-call"}
                        </span>
                        <span className="ml-auto text-[11px] text-slate-500">
                          {new Date(event.timestamp).toLocaleString()}
                        </span>
                      </div>
                      <ul className="mt-3 space-y-1.5 text-xs">
                        {event.findings.map((decision, idx) => (
                          <li key={idx} className="flex gap-2">
                            <span
                              className={`w-14 font-semibold uppercase tracking-[0.2em] ${
                                SEVERITY_STYLES[decision.finding.severity]
                              }`}
                            >
                              {decision.finding.severity}
                            </span>
                            <div>
                              <span className="font-mono text-slate-200">
                                {decision.finding.rule_id}
                              </span>{" "}
                              <span className="text-slate-400">
                                â€” {decision.finding.message}
                              </span>
                              <div className="text-[10px] uppercase tracking-[0.22em] text-slate-500">
                                {decision.finding.location.pointer || "/"} Â·{" "}
                                preview {decision.finding.match_preview}
                              </div>
                            </div>
                          </li>
                        ))}
                      </ul>
                    </article>
                  );
                })}
            </div>
          )}
        </div>
      )}

      {tab === "rules" && (
        <div className="mt-6">
          {rulesState.status === "loading" && (
            <div className="rounded-2xl border border-white/10 bg-white/5 p-6 text-sm text-slate-300">
              Loading rules inventory...
            </div>
          )}
          {rulesState.status === "error" && (
            <div className="rounded-2xl border border-rose-400/30 bg-rose-500/10 p-6 text-sm text-rose-200">
              {rulesState.message}
            </div>
          )}
          {rulesState.status === "ready" && (
            <div className="grid gap-3 md:grid-cols-2">
              {rulesState.data.rules.map((rule) => (
                <article
                  key={rule.id}
                  className="rounded-2xl border border-white/10 bg-white/[0.03] p-4"
                >
                  <div className="flex items-center gap-3">
                    <span className="font-mono text-sm text-slate-100">{rule.id}</span>
                    <span
                      className={`rounded-full border px-2 py-0.5 text-[10px] font-semibold uppercase tracking-[0.18em] ${
                        SEVERITY_STYLES[rule.default_severity]
                      }`}
                    >
                      {rule.default_severity}
                    </span>
                  </div>
                  <p className="mt-2 text-xs text-slate-400">{rule.description}</p>
                  <div className="mt-2 text-[10px] uppercase tracking-[0.22em] text-slate-500">
                    pre-call {rule.pre_call ? "âœ“" : "â€”"} Â· post-call{" "}
                    {rule.post_call ? "âœ“" : "â€”"}
                  </div>
                </article>
              ))}
            </div>
          )}
        </div>
      )}
    </section>
  );
}
