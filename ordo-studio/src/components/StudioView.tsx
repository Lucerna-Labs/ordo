import React from "react";
import { SystemState } from "../hooks/useSystemState";

export function StudioView({
  status,
  activeNiches,
}: {
  status: SystemState;
  activeNiches: string[];
}) {
  const pulseBars = [42, 68, 50, 88, 64, 78, 36, 92, 48, 61];
  return (
    <div className="grid gap-6 lg:grid-cols-[1.65fr_1fr]">
      <section className="liquid-panel group relative overflow-hidden rounded-[2.75rem] border p-10">
        <div
          className={`absolute inset-0 opacity-30 transition-all duration-1000 ${
            status === "CRITICAL"
              ? "bg-red-700/50"
              : status === "RESCUE"
                ? "bg-amber-500/40"
                : status === "PROCESSING"
                  ? "bg-blue-500/30"
                  : "bg-teal-500/30"
          }`}
        />
        <div className="relative">
          <div className="text-[11px] font-semibold uppercase tracking-[0.36em] text-slate-400">
            Studio pulse
          </div>
          <h2 className="mt-4 text-5xl font-light tracking-tight text-white">System Pulse</h2>
          <p className="mt-4 max-w-2xl text-sm leading-7 text-slate-300">
            Liquid Glass 2026 keeps routing, retrieval, and repair posture visible while the
            runtime shifts between stable throughput, processing, rescue, and containment.
          </p>
          <div className="mt-10 flex h-40 items-end gap-2">
            {pulseBars.map((height, index) => (
              <div
                key={`${height}-${index}`}
                className="flex-1 rounded-t-[1rem] border border-white/10 bg-white/10 transition-all duration-300 group-hover:bg-teal-300/55"
                style={{ height: `${height}%` }}
              />
            ))}
          </div>
          <div className="mt-8 grid gap-4 sm:grid-cols-3">
            {[
              ["RAG lanes", "Focused collections stay visibly separated."],
              ["Bridge mesh", "PQ and relay posture stay readable at a glance."],
              ["Medbay", "Manual mechanic commands stay inside the shell."],
            ].map(([title, body]) => (
              <article
                key={title}
                className="rounded-[1.5rem] border border-white/10 bg-black/20 p-4 backdrop-blur-xl"
              >
                <div className="text-sm font-semibold text-white">{title}</div>
                <div className="mt-2 text-xs leading-6 text-slate-400">{body}</div>
              </article>
            ))}
          </div>
        </div>
      </section>

      <aside className="liquid-panel rounded-[2.3rem] border p-8">
        <div className="text-[11px] font-semibold uppercase tracking-[0.34em] text-teal-300">
          Active agent niches
        </div>
        <div className="mt-6 space-y-4">
          {activeNiches.map((niche, index) => (
            <div
              key={`${niche}-${index}`}
              className="flex items-center justify-between rounded-[1.25rem] border border-white/10 bg-white/5 px-4 py-4"
            >
              <span className="text-sm text-slate-100">{niche}</span>
              <span className="h-2.5 w-2.5 rounded-full bg-teal-400 shadow-[0_0_16px_rgba(45,212,191,0.85)]" />
            </div>
          ))}
        </div>
      </aside>
    </div>
  );
}
