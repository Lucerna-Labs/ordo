import { SystemState } from "./hooks/useSystemState";

export type TabType =
  | "DASHBOARD"
  | "CONNECTIONS"
  | "RAG_P2P"
  | "HEALING"
  | "CUSTOM_NICHE";

export interface Tab {
  id: string;
  label: string;
  type: TabType;
}

export interface NicheModule {
  id: string;
  label: string;
  type: "CUSTOM_NICHE";
  collectionId: string;
  focus: string;
  status: string;
  accent: string;
  configPath: string;
}

export interface RagCollection {
  id: string;
  label: string;
  group: "SHARED" | "DOMAIN" | "CUSTOM";
  chunkCount: number;
  documentCount: number;
  accent: string;
  summary: string;
}

export interface SwarmNode {
  id: string;
  label: string;
  status: "ONLINE" | "SYNCING" | "DEGRADED" | "ISOLATED";
  transport: string;
  latencyMs: number;
  collections: string[];
  zone: string;
}

export interface P2PStatus {
  mode: string;
  health: string;
  relay: string;
  summary: string;
  connectedPeers: number;
  nodes: SwarmNode[];
}

export interface LibrarySnapshot {
  collections: RagCollection[];
  p2pStatus: P2PStatus;
  lastSync: string;
}

export interface ShellBootstrap {
  systemState: SystemState;
  nicheModules: NicheModule[];
  library: LibrarySnapshot;
  activeNiches: string[];
}

export interface MechanicReply {
  state: SystemState;
  response: string;
  actions: string[];
}

export interface MechanicMessage {
  id: string;
  role: "user" | "mechanic";
  content: string;
  timestamp: string;
}
