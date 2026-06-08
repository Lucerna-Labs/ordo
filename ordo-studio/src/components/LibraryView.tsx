import React from "react";
import { SystemState } from "../hooks/useSystemState";
import { LibrarySnapshot } from "../types";
import { StatusCapsule } from "./ShellPrimitives";

export function LibraryView({
  snapshot,
  status,
}: {
  snapshot: LibrarySnapshot;
  status: SystemState;
}) {
  const size = 380;
  const center = size / 2;
  return (
    <div className="grid gap-6 xl:grid-cols-[1.25fr_0.95fr]">
      <section className="liquid-panel rounded-[2.6rem] border p-8">
        <div className="flex flex-wrap items-center justify-between gap-4">
          <div>
            <div className="text-[11px] font-semibold uppercase tracking-[0.34em] text-cyan-200">
              RAG niche memory
            </div>
            <h2 className="mt-3 text-4xl font-light tracking-tight text-white">
              Carved memory lanes
            </h2>
          </div>
          <StatusCapsule
            label="Last sync"
            value={new Date(snapshot.lastSync).toLocaleTimeString()}
          />
        </div>

        <div className="mt-8 grid gap-8 xl:grid-cols-[420px_1fr]">
          <div className="relative mx-auto grid w-full max-w-[420px] place-items-center">
            <svg
              viewBox={`0 0 ${size} ${size}`}
              className="h-[24rem] w-[24rem] drop-shadow-[0_0_32px_rgba(59,130,246,0.18)]"
            >
              <defs>
                {snapshot.collections.map((collection) => (
                  <linearGradient
                    key={`gradient-${collection.id}`}
                    id={`ring-${collection.id}`}
                    x1="0%"
                    y1="0%"
                    x2="100%"
                    y2="100%"
                  >
                    <stop offset="0%" stopColor={collection.accent} stopOpacity="0.95" />
                    <stop offset="100%" stopColor="#ffffff" stopOpacity="0.35" />
                  </linearGradient>
                ))}
              </defs>

              <circle
                cx={center}
                cy={center}
                r="52"
                fill="rgba(255,255,255,0.04)"
                stroke="rgba(255,255,255,0.14)"
              />
              <circle
                cx={center}
                cy={center}
                r="74"
                fill="none"
                stroke="rgba(255,255,255,0.06)"
                strokeDasharray="3 8"
              />

              {snapshot.collections.map((collection, index) => {
                const radius = 92 + index * 24;
                const circumference = 2 * Math.PI * radius;
                const segmentCount = Math.max(
                  5,
                  Math.min(22, Math.round(collection.chunkCount / 3)),
                );
                const visibleLength =
                  circumference * Math.min(0.82, 0.18 + collection.chunkCount / 95);
                const dash = visibleLength / segmentCount;
                const gap = Math.max(8, (circumference - visibleLength) / segmentCount);
                return (
                  <circle
                    key={collection.id}
                    cx={center}
                    cy={center}
                    r={radius}
                    fill="none"
                    stroke={`url(#ring-${collection.id})`}
                    strokeWidth="10"
                    strokeLinecap="round"
                    strokeDasharray={`${dash.toFixed(1)} ${gap.toFixed(1)}`}
                    transform={`rotate(${index * 16 - 90} ${center} ${center})`}
                    opacity={collection.group === "CUSTOM" ? 0.9 : 0.76}
                  />
                );
              })}

              <text
                x={center}
                y={center - 6}
                fill="rgba(255,255,255,0.95)"
                textAnchor="middle"
                fontSize="16"
                letterSpacing="3"
              >
                ORDO
              </text>
              <text
                x={center}
                y={center + 18}
                fill="rgba(148,163,184,0.9)"
                textAnchor="middle"
                fontSize="11"
                letterSpacing="2"
              >
                {status} MEMORY MESH
              </text>
            </svg>
          </div>

          <div className="space-y-4">
            {snapshot.collections.map((collection) => (
              <article
                key={collection.id}
                className="rounded-[1.5rem] border border-white/10 bg-black/20 p-5"
              >
                <div className="flex flex-wrap items-center justify-between gap-3">
                  <div className="flex items-center gap-3">
                    <span
                      className="h-3.5 w-3.5 rounded-full"
                      style={{ background: collection.accent }}
                    />
                    <div>
                      <div className="text-lg font-medium text-white">{collection.label}</div>
                      <div className="text-[11px] uppercase tracking-[0.28em] text-slate-500">
                        {collection.group} collection
                      </div>
                    </div>
                  </div>
                  <div className="text-right text-xs text-slate-400">
                    <div>{collection.documentCount} docs</div>
                    <div>{collection.chunkCount} chunks</div>
                  </div>
                </div>
                <p className="mt-3 text-sm leading-7 text-slate-400">{collection.summary}</p>
              </article>
            ))}
          </div>
        </div>
      </section>

      <section className="liquid-panel rounded-[2.6rem] border p-8">
        <div className="text-[11px] font-semibold uppercase tracking-[0.34em] text-slate-400">
          P2P swarm map
        </div>
        <h3 className="mt-3 text-3xl font-light text-white">{snapshot.p2pStatus.mode}</h3>
        <p className="mt-3 text-sm leading-7 text-slate-400">{snapshot.p2pStatus.summary}</p>

        <div className="relative mt-8 h-[24rem] overflow-hidden rounded-[2rem] border border-white/10 bg-black/30">
          <svg className="absolute inset-0 h-full w-full" viewBox="0 0 100 100">
            <circle cx="50" cy="50" r="7" fill="rgba(255,255,255,0.14)" />
            {snapshot.p2pStatus.nodes.map((node, index) => {
              const angle =
                (Math.PI * 2 * index) / snapshot.p2pStatus.nodes.length - Math.PI / 2;
              const x = 50 + Math.cos(angle) * 31;
              const y = 50 + Math.sin(angle) * 31;
              return (
                <g key={`line-${node.id}`}>
                  <line
                    x1="50"
                    y1="50"
                    x2={x}
                    y2={y}
                    stroke={
                      node.status === "ONLINE"
                        ? "rgba(45,212,191,0.5)"
                        : node.status === "SYNCING"
                          ? "rgba(96,165,250,0.55)"
                          : node.status === "DEGRADED"
                            ? "rgba(251,191,36,0.52)"
                            : "rgba(248,113,113,0.6)"
                    }
                    strokeWidth="0.5"
                    strokeDasharray="1.5 1.5"
                  />
                  <circle
                    cx={x}
                    cy={y}
                    r="4.2"
                    fill={
                      node.status === "ONLINE"
                        ? "rgba(45,212,191,0.9)"
                        : node.status === "SYNCING"
                          ? "rgba(96,165,250,0.9)"
                          : node.status === "DEGRADED"
                            ? "rgba(251,191,36,0.9)"
                            : "rgba(248,113,113,0.92)"
                    }
                  />
                </g>
              );
            })}
          </svg>

          <div className="absolute left-1/2 top-1/2 grid h-24 w-24 -translate-x-1/2 -translate-y-1/2 place-items-center rounded-full border border-white/15 bg-white/5 text-center backdrop-blur-xl">
            <div>
              <div className="text-[10px] font-semibold uppercase tracking-[0.24em] text-slate-500">
                Hub
              </div>
              <div className="mt-1 text-sm font-semibold text-white">
                {snapshot.p2pStatus.health}
              </div>
            </div>
          </div>

          {snapshot.p2pStatus.nodes.map((node, index) => {
            const angle =
              (Math.PI * 2 * index) / snapshot.p2pStatus.nodes.length - Math.PI / 2;
            const x = 50 + Math.cos(angle) * 31;
            const y = 50 + Math.sin(angle) * 31;
            return (
              <div
                key={node.id}
                className="absolute w-36 -translate-x-1/2 -translate-y-1/2 rounded-[1rem] border border-white/10 bg-black/45 px-3 py-2 backdrop-blur-xl"
                style={{ left: `${x}%`, top: `${y}%` }}
              >
                <div className="text-xs font-semibold text-white">{node.label}</div>
                <div className="mt-1 text-[10px] uppercase tracking-[0.2em] text-slate-500">
                  {node.transport}
                </div>
                <div className="mt-2 text-[11px] text-slate-300">
                  {node.status} Â· {node.latencyMs} ms
                </div>
              </div>
            );
          })}
        </div>
      </section>
    </div>
  );
}
