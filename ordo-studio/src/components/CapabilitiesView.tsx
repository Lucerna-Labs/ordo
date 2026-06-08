import React, { useEffect, useMemo, useState } from "react";

interface CapabilityDescriptor {
  name: string;
  provider: string;
  description: string;
  tier: "Core" | "Optional" | "Heavy" | string;
  activation: "Eager" | "Lazy" | string;
}

interface CapabilitiesResponse {
  count: number;
  lane_count: number;
  lanes?: unknown;
  descriptors: CapabilityDescriptor[];
}

const CONTROL_API_ORIGIN =
  (typeof window !== "undefined" &&
    (window as Window & { __ORDO_CONTROL_ORIGIN__?: string }).__ORDO_CONTROL_ORIGIN__) ||
  "http://127.0.0.1:4141";

const LANE_ORDER: Array<{ id: string; label: string; prefix: string; accent: string }> = [
  { id: "knowledge", label: "Knowledge", prefix: "knowledge.", accent: "#f87171" },
  { id: "orchestration", label: "Orchestration", prefix: "orchestration.", accent: "#93c5fd" },
  { id: "research", label: "Research", prefix: "research.", accent: "#fbbf24" },
  { id: "ssh", label: "SSH", prefix: "ssh.", accent: "#a78bfa" },
  { id: "api", label: "API", prefix: "api.", accent: "#60a5fa" },
  { id: "rest", label: "REST", prefix: "rest.", accent: "#34d399" },
  { id: "filesystem", label: "Filesystem", prefix: "filesystem.", accent: "#cbd5f5" },
  { id: "memory", label: "Memory", prefix: "memory.", accent: "#fde68a" },
  { id: "self_heal", label: "Self-Heal", prefix: "self_heal.", accent: "#fda4af" },
  { id: "runtime", label: "Runtime", prefix: "runtime.", accent: "#c4b5fd" },
];

function laneFor(capabilityName: string): (typeof LANE_ORDER)[number] {
  const match = LANE_ORDER.find((lane) => capabilityName.startsWith(lane.prefix));
  return match ?? { id: "other", label: "Other", prefix: "", accent: "#94a3b8" };
}

export function CapabilitiesView() {
  const [state, setState] = useState<
    | { status: "loading" }
    | { status: "error"; message: string }
    | { status: "ready"; data: CapabilitiesResponse }
  >({ status: "loading" });
  const [filter, setFilter] = useState("");
  const [activeLane, setActiveLane] = useState<string | null>(null);

  async function refresh() {
    setState({ status: "loading" });
    try {
      const response = await fetch(`${CONTROL_API_ORIGIN}/api/capabilities`);
      if (!response.ok) {
        throw new Error(`control API returned ${response.status}`);
      }
      const data = (await response.json()) as CapabilitiesResponse;
      setState({ status: "ready", data });
    } catch (error) {
      setState({
        status: "error",
        message:
          error instanceof Error
            ? error.message
            : "control API is unreachable at " + CONTROL_API_ORIGIN,
      });
    }
  }

  useEffect(() => {
    void refresh();
  }, []);

  const grouped = useMemo(() => {
    if (state.status !== "ready") {
      return null;
    }
    const buckets = new Map<string, CapabilityDescriptor[]>();
    for (const descriptor of state.data.descriptors) {
      const lane = laneFor(descriptor.name);
      const list = buckets.get(lane.id) ?? [];
      list.push(descriptor);
      buckets.set(lane.id, list);
    }
    for (const list of buckets.values()) {
      list.sort((a, b) => a.name.localeCompare(b.name));
    }
    return buckets;
  }, [state]);

  const visibleLanes = useMemo(() => {
    if (!grouped) {
      return [] as Array<{ lane: (typeof LANE_ORDER)[number]; items: CapabilityDescriptor[] }>;
    }
    const needle = filter.trim().toLowerCase();
    const lanes: Array<{ lane: (typeof LANE_ORDER)[number]; items: CapabilityDescriptor[] }> = [];
    const processed = new Set<string>();
    for (const lane of LANE_ORDER) {
      const items = grouped.get(lane.id);
      if (!items) continue;
      processed.add(lane.id);
      const filtered = needle
        ? items.filter(
            (item) =>
              item.name.toLowerCase().includes(needle) ||
              item.description.toLowerCase().includes(needle),
          )
        : items;
      if (filtered.length === 0) continue;
      if (activeLane && activeLane !== lane.id) continue;
      lanes.push({ lane, items: filtered });
    }
    const otherItems = grouped.get("other");
    if (otherItems && (!activeLane || activeLane === "other")) {
      const filtered = needle
        ? otherItems.filter(
            (item) =>
              item.name.toLowerCase().includes(needle) ||
              item.description.toLowerCase().includes(needle),
          )
        : otherItems;
      if (filtered.length > 0) {
        lanes.push({
          lane: { id: "other", label: "Other", prefix: "", accent: "#94a3b8" },
          items: filtered,
        });
      }
    }
    return lanes;
  }, [grouped, filter, activeLane]);

  return (
    <section className="liquid-panel relative overflow-hidden rounded-[2.75rem] border p-10">
      <div className="flex flex-wrap items-start justify-between gap-6">
        <div>
          <div className="text-[11px] font-semibold uppercase tracking-[0.36em] text-slate-400">
            Capability inventory
          </div>
          <h2 className="mt-3 text-4xl font-light tracking-tight text-white">
            Live capability surface
          </h2>
          <p className="mt-2 max-w-2xl text-sm leading-7 text-slate-400">
            Pulled straight from the local control API at{" "}
            <code className="rounded bg-white/10 px-2 py-0.5 text-xs text-slate-200">
              {CONTROL_API_ORIGIN}/api/capabilities
            </code>
            . Knowledge, local tooling, SSH, generic API, and REST lanes all
            register here alongside the core runtime capabilities.
          </p>
        </div>
        <div className="flex items-center gap-3">
          <input
            value={filter}
            onChange={(event) => setFilter(event.target.value)}
            placeholder="Filter capabilities"
            className="w-64 rounded-full border border-white/10 bg-white/5 px-4 py-2 text-sm text-slate-100 outline-none transition focus:border-teal-300/40 focus:bg-white/10"
          />
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
          Loading capability inventory...
        </div>
      )}
      {state.status === "error" && (
        <div className="mt-8 rounded-2xl border border-rose-400/30 bg-rose-500/10 p-6 text-sm text-rose-200">
          {state.message}
          <div className="mt-2 text-xs text-rose-300/80">
            Start the runtime with <code>cargo run</code> (control API defaults to{" "}
            <code>127.0.0.1:4141</code>).
          </div>
        </div>
      )}

      {state.status === "ready" && grouped && (
        <>
          <div className="mt-6 flex flex-wrap gap-2">
            <button
              onClick={() => setActiveLane(null)}
              className={`rounded-full border px-3 py-1 text-xs tracking-[0.16em] transition ${
                activeLane === null
                  ? "border-teal-300/40 bg-teal-500/15 text-teal-100"
                  : "border-white/10 bg-white/5 text-slate-300 hover:border-white/20"
              }`}
            >
              All ({state.data.count})
            </button>
            {[...LANE_ORDER, { id: "other", label: "Other", prefix: "", accent: "#94a3b8" }]
              .filter((lane) => grouped.has(lane.id))
              .map((lane) => (
                <button
                  key={lane.id}
                  onClick={() => setActiveLane(activeLane === lane.id ? null : lane.id)}
                  className={`rounded-full border px-3 py-1 text-xs tracking-[0.16em] transition ${
                    activeLane === lane.id
                      ? "border-white/30 bg-white/15 text-white"
                      : "border-white/10 bg-white/5 text-slate-300 hover:border-white/20"
                  }`}
                  style={{ color: activeLane === lane.id ? lane.accent : undefined }}
                >
                  {lane.label} ({grouped.get(lane.id)?.length ?? 0})
                </button>
              ))}
          </div>

          <div className="mt-6 grid gap-5 lg:grid-cols-2">
            {visibleLanes.map(({ lane, items }) => (
              <div
                key={lane.id}
                className="rounded-3xl border border-white/10 bg-white/[0.03] p-6"
                style={{ boxShadow: `0 0 0 1px ${lane.accent}15 inset` }}
              >
                <div className="flex items-center justify-between">
                  <h3
                    className="text-sm font-semibold uppercase tracking-[0.28em]"
                    style={{ color: lane.accent }}
                  >
                    {lane.label}
                  </h3>
                  <span className="text-xs text-slate-500">{items.length} caps</span>
                </div>
                <ul className="mt-4 space-y-3">
                  {items.map((descriptor) => (
                    <li
                      key={descriptor.name}
                      className="rounded-2xl border border-white/5 bg-black/20 p-4"
                    >
                      <div className="flex flex-wrap items-center gap-2">
                        <span className="font-mono text-sm text-slate-100">
                          {descriptor.name}
                        </span>
                        <span
                          className="rounded-full border px-2 py-0.5 text-[10px] font-semibold uppercase tracking-[0.18em]"
                          style={{
                            borderColor: `${lane.accent}55`,
                            color: lane.accent,
                            background: `${lane.accent}12`,
                          }}
                        >
                          {descriptor.tier}
                        </span>
                        <span className="rounded-full border border-white/10 bg-white/5 px-2 py-0.5 text-[10px] uppercase tracking-[0.18em] text-slate-400">
                          {descriptor.activation}
                        </span>
                      </div>
                      <p className="mt-2 text-xs leading-6 text-slate-400">
                        {descriptor.description}
                      </p>
                      <div className="mt-2 text-[10px] uppercase tracking-[0.22em] text-slate-500">
                        provider Â· {descriptor.provider}
                      </div>
                    </li>
                  ))}
                </ul>
              </div>
            ))}
            {visibleLanes.length === 0 && (
              <div className="rounded-2xl border border-white/10 bg-white/5 p-6 text-sm text-slate-300 lg:col-span-2">
                No capabilities match that filter.
              </div>
            )}
          </div>
        </>
      )}
    </section>
  );
}
