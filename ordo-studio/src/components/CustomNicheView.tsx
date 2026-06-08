import React from "react";
import { NicheModule } from "../types";
import { InfoCard } from "./ShellPrimitives";

export function CustomNicheView({ module }: { module: NicheModule }) {
  return (
    <section className="liquid-panel rounded-[2.5rem] border p-10">
      <div className="flex flex-wrap items-center justify-between gap-4">
        <div>
          <div className="text-[11px] font-semibold uppercase tracking-[0.34em] text-slate-400">
            Modular crate active
          </div>
          <h2 className="mt-3 text-4xl font-light capitalize text-white">{module.label}</h2>
        </div>
        <div
          className="rounded-full border px-4 py-2 text-xs font-semibold uppercase tracking-[0.18em]"
          style={{
            borderColor: `${module.accent}55`,
            background: `${module.accent}14`,
            color: module.accent,
          }}
        >
          {module.status}
        </div>
      </div>
      <div className="mt-8 grid gap-4 md:grid-cols-3">
        <InfoCard label="Collection lane" value={module.collectionId} />
        <InfoCard label="Primary focus" value={module.focus} />
        <InfoCard label="Config file" value={module.configPath} />
      </div>
    </section>
  );
}
