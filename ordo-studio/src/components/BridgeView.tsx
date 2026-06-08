import React from "react";
import { P2PStatus } from "../types";
import { StatusCapsule } from "./ShellPrimitives";

export function BridgeView({ p2pStatus }: { p2pStatus: P2PStatus }) {
  return (
    <div className="space-y-6">
      <div className="grid gap-6 xl:grid-cols-4">
        {[
          {
            name: "OpenAI",
            note: "External model lane with controlled fallback posture.",
          },
          {
            name: "Anthropic",
            note: "Parallel inference bridge for analytics workloads.",
          },
          {
            name: "Hetzner SSH",
            note: "Remote execution bridge for deployments and machine diagnostics.",
          },
          {
            name: "NAT Cloud P2P",
            note: "Direct mesh with relay fallback and post-quantum handshakes.",
          },
        ].map((bridge, index) => (
          <article
            key={bridge.name}
            className="liquid-panel rounded-[2rem] border p-6 transition-all hover:border-teal-300/30 hover:bg-white/[0.08]"
          >
            <div className="flex items-start justify-between gap-4">
              <div className="grid h-11 w-11 place-items-center rounded-2xl border border-white/10 bg-white/5 text-sm font-semibold text-white">
                {index + 1}
              </div>
              <span className="rounded-md border border-teal-300/20 bg-teal-500/10 px-2 py-1 text-[10px] font-semibold tracking-[0.2em] text-teal-200">
                {p2pStatus.health}
              </span>
            </div>
            <h3 className="mt-6 text-xl font-medium text-white">{bridge.name}</h3>
            <p className="mt-2 text-sm leading-7 text-slate-400">{bridge.note}</p>
            <div className="mt-5 text-xs uppercase tracking-[0.24em] text-slate-500">
              {p2pStatus.mode}
            </div>
          </article>
        ))}
      </div>

      <section className="liquid-panel rounded-[2.2rem] border p-8">
        <div className="flex flex-wrap items-center justify-between gap-4">
          <div>
            <div className="text-[11px] font-semibold uppercase tracking-[0.34em] text-slate-400">
              Bridge mesh telemetry
            </div>
            <h3 className="mt-3 text-3xl font-light text-white">{p2pStatus.summary}</h3>
          </div>
          <div className="flex gap-3">
            <StatusCapsule label="Relay" value={p2pStatus.relay} />
            <StatusCapsule label="Peers" value={`${p2pStatus.connectedPeers.toString()} linked`} />
          </div>
        </div>
        <div className="mt-8 grid gap-4 md:grid-cols-2 xl:grid-cols-4">
          {p2pStatus.nodes.map((node) => (
            <div
              key={node.id}
              className="rounded-[1.5rem] border border-white/10 bg-black/20 p-4"
            >
              <div className="flex items-center justify-between gap-3">
                <div className="text-sm font-semibold text-white">{node.label}</div>
                <span
                  className={`rounded-full px-2 py-1 text-[10px] font-semibold tracking-[0.2em] ${
                    node.status === "ONLINE"
                      ? "bg-teal-500/15 text-teal-200"
                      : node.status === "SYNCING"
                        ? "bg-blue-500/15 text-blue-200"
                        : node.status === "DEGRADED"
                          ? "bg-amber-500/15 text-amber-200"
                          : "bg-red-500/15 text-red-200"
                  }`}
                >
                  {node.status}
                </span>
              </div>
              <div className="mt-3 text-xs uppercase tracking-[0.24em] text-slate-500">
                {node.transport}
              </div>
              <div className="mt-2 text-sm text-slate-300">
                {node.zone} zone · {node.latencyMs} ms
              </div>
              <div className="mt-4 text-xs leading-6 text-slate-400">
                Collections: {node.collections.join(", ")}
              </div>
            </div>
          ))}
        </div>
      </section>
    </div>
  );
}
