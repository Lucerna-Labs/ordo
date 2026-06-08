import React from "react";

export function AtmosphericAura({
  aura,
}: {
  aura: {
    primaryGlow: string;
    secondaryGlow: string;
    tertiaryGlow: string;
    border: string;
    panelTint: string;
  };
}) {
  return (
    <div aria-hidden className="pointer-events-none absolute inset-0 overflow-hidden">
      <div
        className="absolute -left-20 top-[-8rem] h-[30rem] w-[30rem] rounded-full blur-[120px] transition-all duration-1000"
        style={{ background: aura.primaryGlow }}
      />
      <div
        className="absolute right-[-8rem] top-[6rem] h-[26rem] w-[26rem] rounded-full blur-[120px] transition-all duration-1000"
        style={{ background: aura.secondaryGlow }}
      />
      <div
        className="absolute bottom-[-10rem] left-[26%] h-[24rem] w-[24rem] rounded-full blur-[120px] transition-all duration-1000"
        style={{ background: aura.tertiaryGlow }}
      />
      <div
        className="absolute inset-0 opacity-70"
        style={{
          background:
            "linear-gradient(rgba(255,255,255,0.05) 1px, transparent 1px), linear-gradient(90deg, rgba(255,255,255,0.05) 1px, transparent 1px)",
          backgroundSize: "40px 40px",
          maskImage: "linear-gradient(to bottom, rgba(0,0,0,0.6), transparent 90%)",
        }}
      />
      <div
        className="absolute inset-[18px] rounded-[34px]"
        style={{
          border: `1px solid ${aura.border}`,
          background: aura.panelTint,
          opacity: 0.18,
        }}
      />
    </div>
  );
}

export function StatusCapsule({ label, value }: { label: string; value: string }) {
  return (
    <div className="rounded-full border border-white/10 bg-white/5 px-4 py-2 text-right">
      <div className="text-[10px] font-semibold uppercase tracking-[0.26em] text-slate-500">
        {label}
      </div>
      <div className="text-xs font-medium text-slate-200">{value}</div>
    </div>
  );
}

export function InfoCard({ label, value }: { label: string; value: string }) {
  return (
    <article className="rounded-[1.5rem] border border-white/10 bg-black/20 p-5">
      <div className="text-[11px] font-semibold uppercase tracking-[0.28em] text-slate-500">
        {label}
      </div>
      <div className="mt-3 break-words text-sm leading-7 text-slate-100">{value}</div>
    </article>
  );
}

export function NewNicheComposer({
  open,
  value,
  onChange,
  onClose,
  onSubmit,
}: {
  open: boolean;
  value: string;
  onChange: (value: string) => void;
  onClose: () => void;
  onSubmit: () => void;
}) {
  if (!open) {
    return null;
  }

  return (
    <div className="absolute inset-0 z-[70] grid place-items-center bg-slate-950/50 backdrop-blur-xl">
      <div className="liquid-panel w-[min(32rem,calc(100vw-2rem))] rounded-[2.4rem] border p-8">
        <div className="text-[11px] font-semibold uppercase tracking-[0.34em] text-teal-300">
          New niche
        </div>
        <h2 className="mt-3 text-3xl font-light text-white">Spin up a modular crate lane</h2>
        <p className="mt-3 text-sm leading-7 text-slate-400">
          Add a new niche like 3D Colorist, Motion Finisher, or Luxury Copy Lab. Ordo
          will persist a config file and register the lane in the shell.
        </p>

        <label className="mt-6 block text-sm text-slate-300">
          <span className="mb-2 block text-[11px] font-semibold uppercase tracking-[0.28em] text-slate-500">
            Niche name
          </span>
          <input
            autoFocus
            value={value}
            onChange={(event) => onChange(event.target.value)}
            onKeyDown={(event) => {
              if (event.key === "Enter") {
                event.preventDefault();
                onSubmit();
              }
            }}
            placeholder="3D Colorist"
            className="w-full rounded-[1.3rem] border border-white/10 bg-white/[0.04] px-4 py-3 text-base text-white outline-none transition placeholder:text-slate-600 focus:border-teal-300/40"
          />
        </label>

        <div className="mt-8 flex justify-end gap-3">
          <button
            onClick={onClose}
            className="rounded-full border border-white/10 bg-white/5 px-5 py-3 text-xs font-semibold uppercase tracking-[0.18em] text-slate-200"
          >
            Cancel
          </button>
          <button
            onClick={onSubmit}
            className="rounded-full border border-teal-300/30 bg-teal-500/16 px-5 py-3 text-xs font-semibold uppercase tracking-[0.18em] text-teal-100"
          >
            Create lane
          </button>
        </div>
      </div>
    </div>
  );
}
