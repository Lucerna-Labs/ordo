import { SystemState } from "./hooks/useSystemState";
import {
  LibrarySnapshot,
  MechanicReply,
  NicheModule,
  ShellBootstrap,
  SwarmNode,
  Tab,
} from "./types";

export const BASE_TABS: Tab[] = [
  // Assistant is the platform's primary surface — it's where the
  // operator actually talks to Ordo. Everything else is
  // inspection / admin.
  { id: "assistant", label: "Assistant", type: "DASHBOARD" },
  { id: "review", label: "Review", type: "DASHBOARD" },
  // Connections is the operator's canvas for plugging in real
  // backends (OpenAI, local model servers, SSH, etc.). Lives next
  // to Assistant so non-technical users find it without spelunking.
  { id: "connections", label: "Connections", type: "DASHBOARD" },
  { id: "library", label: "Library", type: "RAG_P2P" },
  { id: "capabilities", label: "Capabilities", type: "DASHBOARD" },
  { id: "cloud", label: "Cloud", type: "DASHBOARD" },
  { id: "plugins", label: "Plugins", type: "DASHBOARD" },
  { id: "security", label: "Security", type: "DASHBOARD" },
  { id: "bridges", label: "Bridges", type: "CONNECTIONS" },
  { id: "studio", label: "Studio", type: "DASHBOARD" },
  { id: "medbay", label: "Medbay", type: "HEALING" },
];

export const FALLBACK_BOOTSTRAP: ShellBootstrap = {
  systemState: "HEALTHY",
  nicheModules: [],
  activeNiches: ["Project Research", "Runtime Ops"],
  library: buildFallbackLibrary("HEALTHY", []),
};

export function buildFallbackLibrary(
  status: SystemState,
  modules: NicheModule[],
): LibrarySnapshot {
  const collections = [
    {
      id: "main",
      label: "Main",
      group: "SHARED" as const,
      chunkCount: 58,
      documentCount: 11,
      accent: "#93c5fd",
      summary:
        "Compact shared memory for project notes, runtime state, and operator reference material.",
    },
    {
      id: "project",
      label: "Project",
      group: "DOMAIN" as const,
      chunkCount: 16,
      documentCount: 3,
      accent: "#14b8a6",
      summary: "Local project notes, decisions, and implementation references.",
    },
    ...modules.map((module) => {
      const seed = hashSeed(module.id);
      return {
        id: module.collectionId,
        label: module.label,
        group: "CUSTOM" as const,
        chunkCount: 7 + (seed % 14),
        documentCount: 2 + (seed % 5),
        accent: module.accent,
        summary: `${module.label} is staged as a modular lane with its own config and future retrieval carve-out.`,
      };
    }),
  ];

  const nodes = buildFallbackNodes(status, modules);
  return {
    collections,
    p2pStatus: {
      mode:
        status === "CRITICAL"
          ? "Containment mesh"
          : status === "RESCUE"
            ? "Fallback relay mesh"
            : status === "PROCESSING"
              ? "Adaptive sync mesh"
              : "Post-quantum mesh",
      health:
        status === "CRITICAL"
          ? "CRITICAL"
          : status === "RESCUE"
            ? "DEGRADED"
            : status === "PROCESSING"
              ? "SYNCING"
              : "STABLE",
      relay:
        status === "RESCUE" || status === "CRITICAL"
          ? "Relay preferred"
          : "Direct preferred",
      summary:
        status === "RESCUE"
          ? "The swarm shifted to rescue routing while the mechanic inspects gateway pressure."
          : status === "CRITICAL"
            ? "Containment routing is active. Manual stabilization is recommended."
            : "Peer bridges are synchronized and ready to carry niche memory slices.",
      connectedPeers: nodes.filter((node) => node.status !== "ISOLATED").length,
      nodes,
    },
    lastSync: new Date().toISOString(),
  };
}

function buildFallbackNodes(status: SystemState, modules: NicheModule[]): SwarmNode[] {
  const customCollections = modules.map((module) => module.collectionId);
  return [
    {
      id: "openai-bridge",
      label: "OpenAI Bridge",
      status: status === "CRITICAL" ? "ISOLATED" : "ONLINE",
      transport: "PQ-ACTIVE",
      latencyMs: status === "RESCUE" ? 58 : 32,
      collections: ["main", ...customCollections.slice(0, 1)],
      zone: "Inference",
    },
    {
      id: "anthropic-bridge",
      label: "Anthropic Bridge",
      status: status === "PROCESSING" ? "SYNCING" : "ONLINE",
      transport: "PQ-ACTIVE",
      latencyMs: status === "PROCESSING" ? 64 : 38,
      collections: ["main", "project", "ops"],
      zone: "Analysis",
    },
    {
      id: "hetzner-ssh",
      label: "Hetzner SSH",
      status: status === "RESCUE" ? "DEGRADED" : "ONLINE",
      transport: "TCP-NOISE",
      latencyMs: status === "RESCUE" ? 86 : 52,
      collections: ["infra", "ops"],
      zone: "Remote",
    },
    {
      id: "nat-cloud-p2p",
      label: "NAT Cloud P2P",
      status: status === "RESCUE" ? "SYNCING" : "ONLINE",
      transport: "RELAY-MESH",
      latencyMs: status === "RESCUE" ? 94 : 47,
      collections: ["project", ...customCollections.slice(1, 3)],
      zone: "Mesh",
    },
  ];
}

export function buildFallbackMechanicReply(
  command: string,
  currentState: SystemState,
): MechanicReply {
  const lowered = command.toLowerCase();
  if (lowered.includes("stabilize") || lowered.includes("repair") || lowered.includes("patch")) {
    return {
      state: "HEALTHY",
      response:
        "Manual fix acknowledged. Gateway fallback retired, RAG sync stabilized, and bridge pressure returned to nominal.",
      actions: ["Run `status` to verify healthy posture.", "Review the Engine Room console."],
    };
  }
  if (lowered.includes("scan") || lowered.includes("diagnose")) {
    return {
      state: "PROCESSING",
      response:
        "Deep scan initiated. The mechanic is tracing bridge latency, RAG lane pressure, and the last self-heal fingerprint.",
      actions: [
        "Wait for the next telemetry burst.",
        "Use `replay last fix` if this was a known incident.",
      ],
    };
  }
  if (lowered.includes("replay")) {
    return {
      state: currentState === "CRITICAL" ? "RESCUE" : "HEALTHY",
      response:
        "Replayed the last known stabilization path against the current gateway posture.",
      actions: ["Inspect bridge mesh telemetry.", "Confirm that rescue mode can be cleared."],
    };
  }
  if (lowered.includes("critical")) {
    return {
      state: "CRITICAL",
      response:
        "Containment posture raised. Direct bridges are restricted until an operator stabilizes the runtime.",
      actions: ["Inspect affected lanes in Library.", "Run `stabilize gateway` after review."],
    };
  }
  if (lowered.includes("status")) {
    return {
      state: currentState,
      response: `Current shell posture is ${currentState}. Main mesh health is ${currentState === "HEALTHY" ? "stable" : "elevated"}.`,
      actions: [
        "Use `scan gateway` for a deeper read.",
        "Use `stabilize gateway` to clear rescue posture.",
      ],
    };
  }
  return {
    state: currentState,
    response:
      "Mechanic command parsed, but it needs a clearer directive. Try `status`, `scan gateway`, `stabilize gateway`, or `replay last fix`.",
    actions: [],
  };
}

export function createFallbackModule(name: string): NicheModule {
  const id = slugify(name);
  const accent = accentFromSeed(hashSeed(id));
  return {
    id,
    label: name.trim(),
    type: "CUSTOM_NICHE",
    collectionId: `niche-${id}`,
    focus: `${name.trim()} orchestration lane`,
    status: "CARVING",
    accent,
    configPath: `user-files/niches/${id}.json`,
  };
}

function slugify(value: string) {
  return value
    .toLowerCase()
    .trim()
    .replace(/[^a-z0-9]+/g, "-")
    .replace(/^-+|-+$/g, "");
}

function hashSeed(value: string) {
  let seed = 0;
  for (const char of value) {
    seed = (seed * 31 + char.charCodeAt(0)) % 997;
  }
  return seed;
}

function accentFromSeed(seed: number) {
  const accents = ["#2dd4bf", "#38bdf8", "#22c55e", "#60a5fa", "#06b6d4", "#f59e0b"];
  return accents[seed % accents.length];
}
