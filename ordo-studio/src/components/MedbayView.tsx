import React from "react";
import { SystemState } from "../hooks/useSystemState";
import { MechanicMessage } from "../types";

export function MedbayView({
  status,
  transcript,
  draft,
  busy,
  onDraftChange,
  onSubmit,
  onSimulateFailure,
}: {
  status: SystemState;
  transcript: MechanicMessage[];
  draft: string;
  busy: boolean;
  onDraftChange: (value: string) => void;
  onSubmit: (event: React.FormEvent<HTMLFormElement>) => void;
  onSimulateFailure: () => void;
}) {
  return (
    <div className="grid gap-6 xl:grid-cols-[0.88fr_1.12fr]">
      <section className="liquid-panel rounded-[2.6rem] border p-10 text-center">
        <div
          className={`mx-auto flex h-32 w-32 items-center justify-center rounded-full border text-4xl transition-all duration-700 ${
            status === "CRITICAL"
              ? "border-red-400/50 text-red-300 shadow-[0_0_48px_rgba(248,113,113,0.28)]"
              : status === "RESCUE"
                ? "border-amber-400/50 text-amber-300 shadow-[0_0_40px_rgba(251,191,36,0.22)]"
                : status === "PROCESSING"
                  ? "border-blue-400/40 text-blue-200 shadow-[0_0_40px_rgba(96,165,250,0.22)]"
                  : "border-white/10 text-white/20"
          }`}
        >
          RX
        </div>
        <div className="mt-8 text-[11px] font-semibold uppercase tracking-[0.32em] text-slate-500">
          Local mechanic
        </div>
        <h2 className="mt-3 text-4xl font-light text-white">Medbay</h2>
        <p className="mx-auto mt-4 max-w-md text-sm leading-7 text-slate-400">
          The self-healing lane can be nudged manually here before we wire a full production LLM
          mechanic onto the same bus.
        </p>
        <div className="mt-8 flex flex-wrap justify-center gap-3">
          <button
            onClick={onSimulateFailure}
            className="rounded-full border border-white/10 bg-white/5 px-6 py-3 text-xs font-semibold uppercase tracking-[0.18em] text-slate-200 transition hover:bg-white/10"
          >
            Simulate outage
          </button>
          <button
            onClick={() => onDraftChange("scan gateway")}
            className="rounded-full border border-teal-300/30 bg-teal-500/12 px-6 py-3 text-xs font-semibold uppercase tracking-[0.18em] text-teal-200 transition hover:bg-teal-500/18"
          >
            Queue diagnostic
          </button>
        </div>
      </section>

      <section className="liquid-panel rounded-[2.6rem] border p-6">
        <div className="rounded-[1.8rem] border border-white/10 bg-black/55 p-5 font-mono">
          <div className="flex items-center justify-between gap-4 border-b border-white/10 pb-4">
            <div>
              <div className="text-[11px] font-semibold uppercase tracking-[0.34em] text-teal-300">
                Local mechanic terminal
              </div>
              <div className="mt-2 text-sm text-slate-400">
                Issue manual commands without leaving the shell.
              </div>
            </div>
            <div className="rounded-full border border-white/10 px-3 py-1 text-[10px] uppercase tracking-[0.24em] text-slate-400">
              {busy ? "processing" : status.toLowerCase()}
            </div>
          </div>

          <div className="mt-5 h-[22rem] space-y-4 overflow-y-auto pr-2">
            {transcript.map((entry) => (
              <div key={entry.id} className="rounded-2xl border border-white/8 bg-white/[0.03] p-4">
                <div className="flex items-center justify-between gap-4 text-[10px] uppercase tracking-[0.24em] text-slate-500">
                  <span>{entry.role === "user" ? "operator" : "mechanic"}</span>
                  <span>{new Date(entry.timestamp).toLocaleTimeString()}</span>
                </div>
                <pre className="mt-3 whitespace-pre-wrap text-[12px] leading-6 text-slate-200">
                  {entry.content}
                </pre>
              </div>
            ))}
          </div>

          <form onSubmit={onSubmit} className="mt-5 flex gap-3">
            <input
              value={draft}
              onChange={(event) => onDraftChange(event.target.value)}
              placeholder="Try: stabilize gateway"
              className="flex-1 rounded-full border border-white/10 bg-white/[0.04] px-5 py-3 text-sm text-slate-100 outline-none transition placeholder:text-slate-600 focus:border-teal-300/40"
            />
            <button
              type="submit"
              disabled={busy}
              className="rounded-full border border-teal-300/30 bg-teal-500/14 px-5 py-3 text-xs font-semibold uppercase tracking-[0.18em] text-teal-100 transition hover:bg-teal-500/22 disabled:cursor-not-allowed disabled:opacity-50"
            >
              Send
            </button>
          </form>
        </div>
      </section>
    </div>
  );
}
