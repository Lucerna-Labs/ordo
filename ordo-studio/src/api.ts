// Tiny client for Ordo's browser and desktop data planes.
//
// Browser/dev mode talks to the control API. The packaged Tauri shell
// reads desktop-owned state through native commands so UI tabs do not
// depend on a sidecar HTTP runtime being present.

export class ApiError extends Error {
  constructor(public status: number, public body: unknown, message?: string) {
    super(message ?? `HTTP ${status}`);
  }
}

const CONTROL_API_ORIGIN =
  (typeof window !== "undefined" &&
    (window as Window & { __ORDO_CONTROL_ORIGIN__?: string }).__ORDO_CONTROL_ORIGIN__) ||
  "http://127.0.0.1:4141";

const isTauriAssetOrigin = () =>
  typeof window !== "undefined" && window.location.hostname === "tauri.localhost";

const isTauriDevOrigin = () =>
  typeof window !== "undefined" &&
  (window.location.origin.includes("://localhost:1420") ||
    window.location.origin.includes("://127.0.0.1:1420") ||
    window.location.host === "localhost:1420" ||
    window.location.host === "127.0.0.1:1420");

const canUseTauriCommands = () =>
  typeof window !== "undefined" &&
  (isTauriAssetOrigin() ||
    "__TAURI_INTERNALS__" in window ||
    "__TAURI__" in window ||
    "__TAURI_IPC__" in window ||
    "__TAURI_METADATA__" in window);

const shouldTryTauriCommands = () => canUseTauriCommands() || isTauriDevOrigin();

function apiUrl(path: string): string {
  if (/^https?:\/\//i.test(path)) return path;
  if (isTauriAssetOrigin() && (path.startsWith("/api") || path === "/health")) {
    return `${CONTROL_API_ORIGIN}${path}`;
  }
  return path;
}

function websocketUrl(path: string): string {
  if (isTauriAssetOrigin()) {
    return `${CONTROL_API_ORIGIN.replace(/^http:/, "ws:").replace(/^https:/, "wss:")}${path}`;
  }
  const proto = window.location.protocol === "https:" ? "wss:" : "ws:";
  return `${proto}//${window.location.host}${path}`;
}

async function invokeLocal<T>(command: string, args?: Record<string, unknown>, force = false): Promise<T> {
  if (!force && !canUseTauriCommands()) {
    throw new Error(`${command} is only available inside the Ordo desktop shell`);
  }
  const { invoke } = await import("@tauri-apps/api/core");
  return invoke<T>(command, args);
}

async function localOrRemote<T>(
  command: string,
  args: Record<string, unknown> | undefined,
  remote: () => Promise<T>,
  fallback?: () => T,
): Promise<T> {
  if (shouldTryTauriCommands()) {
    try {
      return await invokeLocal<T>(command, args, isTauriDevOrigin());
    } catch (err) {
      if (canUseTauriCommands()) throw err;
      if (isTauriDevOrigin() && fallback) return fallback();
    }
  }
  return remote();
}

async function request<T>(method: string, path: string, body?: unknown): Promise<T> {
  const init: RequestInit = {
    method,
    headers: body !== undefined ? { "Content-Type": "application/json" } : undefined,
    body: body !== undefined ? JSON.stringify(body) : undefined,
  };
  const res = await fetch(apiUrl(path), init);
  const text = await res.text();
  let parsed: unknown = undefined;
  if (text.length > 0) {
    try {
      parsed = JSON.parse(text);
    } catch {
      parsed = text;
    }
  }
  if (!res.ok) {
    throw new ApiError(res.status, parsed, `${method} ${path} → ${res.status}`);
  }
  if (typeof parsed === "string" && path.startsWith("/api/")) {
    throw new ApiError(res.status, parsed, `${method} ${path} returned non-JSON`);
  }
  return parsed as T;
}

export const api = {
  get: <T>(path: string) => request<T>("GET", path),
  post: <T>(path: string, body?: unknown) => request<T>("POST", path, body),
  delete: <T>(path: string, body?: unknown) => request<T>("DELETE", path, body),
  put: <T>(path: string, body?: unknown) => request<T>("PUT", path, body),
  patch: <T>(path: string, body?: unknown) => request<T>("PATCH", path, body),
};

// ─── Capability inventory ────────────────────────────────────────

export interface CapabilityLane {
  group: "domain" | "interface" | "system" | string;
  name: string;
  label: string;
}

export interface CapabilityDescriptor {
  capability: string;
  description: string;
  provider: string;
  tier: "Core" | "Optional" | "Heavy" | string;
  activation: "Eager" | "Lazy" | string;
  lane: CapabilityLane;
  input_schema?: Record<string, unknown>;
}

export interface CapabilitiesResponse {
  count: number;
  descriptors: CapabilityDescriptor[];
}

const normalizeCapabilities = (out: Partial<CapabilitiesResponse>): CapabilitiesResponse => {
  const descriptors = Array.isArray(out.descriptors) ? out.descriptors : [];
  return {
    count: typeof out.count === "number" ? out.count : descriptors.length,
    descriptors,
  };
};

export const fetchCapabilities = async (): Promise<CapabilitiesResponse> => {
  return normalizeCapabilities(
    await localOrRemote<Partial<CapabilitiesResponse>>(
      "list_local_capabilities",
      undefined,
      () => api.get<Partial<CapabilitiesResponse>>("/api/capabilities"),
      () => ({ descriptors: [], count: 0 }),
    ),
  );
};

export const fetchMcpCapabilities = async (): Promise<CapabilitiesResponse> => {
  return normalizeCapabilities(
    await localOrRemote<Partial<CapabilitiesResponse>>(
      "list_local_mcp_capabilities",
      undefined,
      () => api.get<Partial<CapabilitiesResponse>>("/api/mcp/capabilities"),
      () => ({ descriptors: [], count: 0 }),
    ),
  );
};

// ─── Runtime profile + storage ───────────────────────────────────

export interface RuntimeProfile {
  profile: "minimal" | "standard" | "full" | string;
  rag_enabled: boolean;
  rag_activation_mode: "eager" | "lazy" | string;
  knowledge_enabled: boolean;
  knowledge_activation_mode: "eager" | "lazy" | string;
  embedding_backend: string;
  embedding_dimensions: number;
  llama_cpp_configured: boolean;
  control_api_bind: string;
  control_api_enabled: boolean;
}

export interface RuntimeStorage {
  rag_budget_bytes: number;
  memory_pinned_budget_bytes: number;
  memory_working_budget_bytes: number;
  self_heal_history_budget_bytes: number;
  self_heal_model_context_size?: number;
  self_heal_model_max_tokens?: number;
  self_heal_model_temperature?: number;
}

export interface RuntimeSettingsSnapshot {
  effective: RuntimeProfile & RuntimeStorage & Record<string, unknown>;
  persisted: Record<string, unknown>;
}

export interface UpdateSettingsResponse {
  persisted: Record<string, unknown>;
  restart_required: boolean;
}

export const fetchRuntimeProfile = async (): Promise<RuntimeProfile> => {
  return localOrRemote<RuntimeProfile>(
    "get_local_runtime_profile",
    undefined,
    () => api.get<RuntimeProfile>("/api/runtime/profile"),
    () => ({
      profile: "standard",
      rag_enabled: true,
      rag_activation_mode: "lazy",
      knowledge_enabled: true,
      knowledge_activation_mode: "lazy",
      embedding_backend: "local",
      embedding_dimensions: 0,
      llama_cpp_configured: false,
      control_api_bind: "127.0.0.1:4141",
      control_api_enabled: false,
    }),
  );
};

export const fetchRuntimeStorage = async (): Promise<RuntimeStorage> => {
  return localOrRemote<RuntimeStorage>(
    "get_local_runtime_storage",
    undefined,
    () => api.get<RuntimeStorage>("/api/runtime/storage"),
    () => ({
      rag_budget_bytes: 0,
      memory_pinned_budget_bytes: 0,
      memory_working_budget_bytes: 0,
      self_heal_history_budget_bytes: 0,
    }),
  );
};

export const fetchRuntimeSettings = async (): Promise<RuntimeSettingsSnapshot> => {
  return localOrRemote<RuntimeSettingsSnapshot>(
    "get_local_runtime_settings",
    undefined,
    () => api.post<RuntimeSettingsSnapshot>("/api/tools/runtime.describe_settings", {}),
  );
};
export const updateRuntimeSettings = (patch: Record<string, unknown>) =>
  api.post<UpdateSettingsResponse>("/api/tools/runtime.update_settings", patch);

// ─── RAG ─────────────────────────────────────────────────────────

export interface RagCollection {
  name: string;
  label: string;
  group: "shared" | "domain" | "interface" | "custom" | string;
  document_count: number;
  chunk_count: number;
  sample_titles: string[];
}

export const fetchRagCollections = async (): Promise<{ collections: RagCollection[] }> => {
  return localOrRemote<{ collections: RagCollection[] }>(
    "list_local_rag_collections",
    undefined,
    () => api.get<{ collections: RagCollection[] }>("/api/rag/collections"),
    () => ({ collections: [] }),
  );
};

export interface RagPreviewResponse {
  effective_collections: string[];
  effective_collection_labels: string[];
  hit_count: number;
  hits: Array<{
    chunk_index: number;
    collection: string;
    document_id: string;
    score: number;
    snippet: string;
    tags?: string[];
    title?: string;
    uri?: string;
  }>;
}

export const previewRagCollections = async (query: string): Promise<RagPreviewResponse> => {
  return localOrRemote<RagPreviewResponse>(
    "preview_local_rag_collections",
    { query },
    () => api.get<RagPreviewResponse>(`/api/rag/preview?query=${encodeURIComponent(query)}`),
  );
};

// ─── Memory ──────────────────────────────────────────────────────

export type PinnedNote = string;
export type WorkingNote = string;

export const listPinnedMemory = async (limit = 100): Promise<unknown> => {
  return localOrRemote<unknown>(
    "list_local_pinned_memory",
    { limit },
    () => api.post<unknown>("/api/tools/memory.list_pinned", { limit }),
    () => [],
  );
};
export const listWorkingMemory = async (limit = 50): Promise<unknown> => {
  return localOrRemote<unknown>(
    "list_local_working_memory",
    { limit },
    () => api.post<unknown>("/api/tools/memory.list_working", { limit }),
    () => [],
  );
};
export const pinNote = (content: string) =>
  api.post<unknown>("/api/tools/memory.pin_note", { content });
export const unpinNote = (content: string) =>
  api.post<unknown>("/api/tools/memory.unpin_note", { content });

// ─── Cloud credentials ──────────────────────────────────────────

export interface CloudCredentialRow {
  service: string;
  // Some older readers used `endpoint`; the canonical field on the
  // runtime side is `base_url`. Surface both so existing code keeps
  // compiling while we migrate.
  base_url?: string | null;
  endpoint?: string | null;
  auth_style: string;
  label?: string | null;
  model?: string | null;
  has_secret?: boolean;
  enabled?: boolean;
  extras?: Record<string, string>;
  created_at?: string;
  updated_at?: string;
}

export interface CloudCredentialUpsert {
  service: string;
  // Optional on the wire — the runtime's CloudCredentialUpdate uses
  // Option<String> for everything except `service`, so a partial
  // payload (just `service` + `extras`) merges into the existing row
  // without overwriting unspecified fields. Lets the studio bump
  // `extras.timeout_secs` across all providers without having to
  // re-send their full config.
  auth_style?: string;
  // Renamed from `endpoint` to match the runtime's CloudCredentialUpdate.
  base_url?: string;
  secret?: string;
  label?: string;
  // Runtime accepts string-only values in `extras` (anything else is
  // silently dropped). The studio coerces non-strings before sending.
  extras?: Record<string, string>;
}

const LOCAL_CLOUD_CREDENTIALS_KEY = "ordo:local_cloud_credentials";

function readLocalCloudCredentials(): CloudCredentialRow[] {
  if (typeof window === "undefined") return [];
  try {
    const raw = window.localStorage.getItem(LOCAL_CLOUD_CREDENTIALS_KEY);
    if (!raw) return [];
    const parsed = JSON.parse(raw);
    return Array.isArray(parsed) ? parsed as CloudCredentialRow[] : [];
  } catch {
    return [];
  }
}

function writeLocalCloudCredentials(credentials: CloudCredentialRow[]) {
  if (typeof window === "undefined") return;
  window.localStorage.setItem(LOCAL_CLOUD_CREDENTIALS_KEY, JSON.stringify(credentials));
}

function upsertLocalCloudCredential(payload: CloudCredentialUpsert): { saved: true; local: true } {
  const now = new Date().toISOString();
  const existing = readLocalCloudCredentials();
  const prior = existing.find((credential) => credential.service === payload.service);
  const next: CloudCredentialRow = {
    service: payload.service,
    auth_style: payload.auth_style ?? prior?.auth_style ?? "bearer",
    base_url: payload.base_url ?? prior?.base_url ?? null,
    endpoint: payload.base_url ?? prior?.endpoint ?? null,
    label: payload.label ?? prior?.label ?? payload.service,
    has_secret: Boolean(payload.secret) || prior?.has_secret || payload.extras?.auth_source === "environment",
    enabled:
      payload.extras?.enabled === undefined
        ? prior?.enabled ?? prior?.extras?.enabled !== "false"
        : payload.extras.enabled !== "false",
    extras: {
      ...(prior?.extras ?? {}),
      ...(payload.extras ?? {}),
      enabled: payload.extras?.enabled ?? prior?.extras?.enabled ?? "true",
      ...(payload.label ? { name: payload.label } : {}),
    },
    created_at: prior?.created_at ?? now,
    updated_at: now,
  };
  writeLocalCloudCredentials([
    next,
    ...existing.filter((credential) => credential.service !== payload.service),
  ]);
  return { saved: true, local: true };
}

export const listCloudCredentials = async (): Promise<{ credentials: CloudCredentialRow[]; count: number }> => {
  try {
    return await api.get<{ credentials: CloudCredentialRow[]; count: number }>("/api/cloud/credentials");
  } catch (err) {
    if (shouldTryTauriCommands()) {
      try {
        const native = await invokeLocal<{ credentials: CloudCredentialRow[]; count: number }>(
          "list_local_cloud_credentials",
          undefined,
          isTauriDevOrigin(),
        );
        const local = readLocalCloudCredentials();
        const byService = new Map<string, CloudCredentialRow>();
        for (const credential of local) byService.set(credential.service, credential);
        for (const credential of native.credentials ?? []) byService.set(credential.service, credential);
        const credentials = [...byService.values()];
        return { credentials, count: credentials.length };
      } catch {
        const credentials = readLocalCloudCredentials();
        return { credentials, count: credentials.length };
      }
    }
    throw err;
  }
};

export const upsertCloudCredential = async (payload: CloudCredentialUpsert) => {
  try {
    const out = await api.post<unknown>("/api/cloud/credentials", payload);
    if (shouldTryTauriCommands()) upsertLocalCloudCredential(payload);
    return out;
  } catch (err) {
    if (shouldTryTauriCommands()) return upsertLocalCloudCredential(payload);
    throw err;
  }
};

export interface LocalApiKeyInstallResult {
  env_var: string;
  platform: string;
  installed_for: string;
  local_env_path: string;
  current_process_ready: boolean;
  restart_recommended: boolean;
}

export const installLocalApiKeyEnv = (env_var: string, api_key: string) =>
  invokeLocal<LocalApiKeyInstallResult>(
    "install_local_api_key_env",
    { envVar: env_var, apiKey: api_key },
    isTauriDevOrigin(),
  );

// ─── Webhooks ────────────────────────────────────────────────────

export interface WebhookSubscription {
  id: string;
  workspace_id: string;
  target_url: string;
  // Returned in plaintext on register; "<redacted>" on every other read.
  secret: string;
  topics: string[];
  description: string;
  active: boolean;
  created_at: string;
  last_delivery_at?: string | null;
  last_delivery_status?: number | null;
}

export interface RegisterWebhookPayload {
  target_url: string;
  topics?: string[];
  description?: string;
  secret?: string;
  workspace_id?: string;
}

export interface UpdateWebhookPayload {
  target_url?: string;
  topics?: string[];
  description?: string;
  active?: boolean;
}

export const listWebhooks = (workspace_id?: string) => {
  const q = workspace_id
    ? `?workspace_id=${encodeURIComponent(workspace_id)}`
    : "";
  return localOrRemote<{ subscriptions: WebhookSubscription[] }>(
    "list_local_webhooks",
    undefined,
    () => api.get<{ subscriptions: WebhookSubscription[] }>(`/api/webhooks${q}`),
    () => ({ subscriptions: [] }),
  );
};

export const registerWebhook = (payload: RegisterWebhookPayload) =>
  api.post<{ subscription: WebhookSubscription }>("/api/webhooks", payload);

export const updateWebhook = (id: string, patch: UpdateWebhookPayload) =>
  api.patch<{ subscription: WebhookSubscription }>(
    `/api/webhooks/${encodeURIComponent(id)}`,
    patch,
  );

export const deleteWebhook = (id: string) =>
  api.delete<{ removed: boolean }>(
    `/api/webhooks/${encodeURIComponent(id)}`,
  );

// ─── Local LLM auto-detect ──────────────────────────────────────

export type AutomationRiskLevel =
  | "SafeReadOnly"
  | "LocalRead"
  | "LocalWrite"
  | "NetworkRead"
  | "NetworkWrite"
  | "PeripheralMaintenance"
  | "CoreMutationDenied";

export type AutomationTrigger =
  | "Manual"
  | { At: string }
  | { IntervalSeconds: number }
  | { Heartbeat: { every_seconds: number; jitter_seconds: number; resume_thread?: string | null } }
  | { Cron: { expression: string; timezone: string } }
  | { Event: { topic: string } }
  | { Webhook: { path: string } }
  | { LocalSignal: { name: string } };

export type AutomationIntent =
  | { RunCapability: { capability: string; args: unknown; risk: AutomationRiskLevel } }
  | { ConsultMode: { target_mode: string; question: string; max_iterations: number } }
  | { SpawnSubagent: { mode: string; goal: string; max_iterations: number; risk: AutomationRiskLevel } }
  | { DreamingReview: { mode: string; signal_window: string } }
  | { DiagnosticSweep: { profile: string } }
  | {
      CodingAutomation: {
        workspace_path: string;
        mode: string;
        goal: string;
        max_subagents: number;
        write_policy: "InspectOnly" | "ProposeDiff" | "EditWithApproval";
        commit_policy: "NeverCommit" | "CommitWithApproval";
        dependency_policy: "NoDependencyChanges" | "ProposeDependencyChanges" | "InstallWithApproval";
        risk: AutomationRiskLevel;
      };
    }
  | { Maintenance: { capability: string; args: unknown; risk: AutomationRiskLevel } }
  | { Composite: { steps: AutomationIntent[] } };

export type AutomationScope =
  | "Global"
  | { Workspace: { path: string } }
  | { Mode: { mode: string } }
  | "Diagnostic"
  | { Device: { device_id: string } };

export type AutomationApprovalPolicy =
  | "Never"
  | "Always"
  | { AtOrAbove: AutomationRiskLevel }
  | "ManualOnly";

export interface AutomationSpec {
  id: string;
  name: string;
  description: string;
  enabled: boolean;
  trigger: AutomationTrigger;
  intent: AutomationIntent;
  scope: AutomationScope;
  approval: AutomationApprovalPolicy;
  created_at: string;
  updated_at: string;
  metadata: Record<string, string>;
}

export interface AutomationsResponse {
  automations: AutomationSpec[];
  events?: unknown[];
}

export const listAutomations = () =>
  api.get<AutomationsResponse>("/api/automations");

export const createAutomation = (spec: AutomationSpec) =>
  api.post<{ events: unknown[]; automations?: AutomationSpec[] }>("/api/automations", spec);

export const approveAutomation = (id: string) =>
  api.post<{ events: unknown[] }>(`/api/automations/${encodeURIComponent(id)}/approve`);

export const enableAutomation = (id: string) =>
  api.post<{ event: unknown }>(`/api/automations/${encodeURIComponent(id)}/enable`);

export const disableAutomation = (id: string) =>
  api.post<{ event: unknown }>(`/api/automations/${encodeURIComponent(id)}/disable`);

export const deleteAutomation = (id: string) =>
  api.delete<{ event: unknown }>(`/api/automations/${encodeURIComponent(id)}`);

export const tickAutomations = () =>
  api.post<{ events: unknown[] }>("/api/automations/tick");

// ─── Build spine ────────────────────────────────────────────────

export type BuildStep =
  | "intake"
  | "blueprint"
  | "crate_build"
  | "crate_couple"
  | "build_test"
  | "launch_proof";

export type BuildRunStatus = "active" | "halted" | "complete";

export type BuildErrorClass =
  | "bounded_mechanical"
  | "blueprint_amendment"
  | "compile_errors"
  | "compile_warnings"
  | "architectural_violation"
  | "stub_detected"
  | "couple_debt"
  | "launch_proof_missing"
  | "runtime_panic"
  | "unbounded_ownership"
  | "retry_exhausted"
  | "unknown";

export interface BuildArtifactRef {
  path: string;
  sha256_hex: string;
  role: string;
}

export interface BuildGateEvidence {
  summary: string;
  details: string[];
  artifacts: BuildArtifactRef[];
  checked_at: string;
}

export type BuildGateOutcome =
  | { status: "pass"; evidence: BuildGateEvidence }
  | { status: "fail"; error_class: BuildErrorClass; evidence: BuildGateEvidence }
  | { status: "deferred"; reason: string; evidence: BuildGateEvidence };

export interface BuildGateResult {
  build_id: string;
  project_id: string;
  step: BuildStep;
  outcome: BuildGateOutcome;
}

export interface BuildLedger {
  build_id: string;
  project_id: string;
  status: BuildRunStatus;
  current_step: BuildStep;
  autonomous_correction: boolean;
  requirements?: unknown;
  blueprint_versions: unknown[];
  step_outputs: Record<string, unknown>;
  deferred_debt: unknown[];
  couple_markers: string[];
  retry_ledger: unknown[];
  launch_proof?: unknown;
  created_at: string;
  updated_at: string;
}

export interface BuildsResponse {
  builds: BuildLedger[];
  active_builds: string[];
}

export const listBuilds = () =>
  api.get<BuildsResponse>("/api/builds");

export const startBuild = (project_id: string) =>
  api.post<{ build_id: string; ledger: BuildLedger }>("/api/builds", { project_id });

export const getBuild = (id: string) =>
  api.get<{ ledger: BuildLedger }>(`/api/builds/${encodeURIComponent(id)}`);

export const submitBuildGateResult = (id: string, result: BuildGateResult) =>
  api.post<{ decision: string; ledger: BuildLedger }>(
    `/api/builds/${encodeURIComponent(id)}/gate`,
    result,
  );

export interface LocalLlmDiscovery {
  provider: "ollama" | "lmstudio";
  base_url: string;
  reachable: boolean;
  models: string[];
  error?: string;
}

interface OpenAIModelsResponse {
  data?: { id: string }[];
}

const LOCAL_PROVIDER_PROXY: Record<"ollama" | "lmstudio", string> = {
  // The vite proxy forwards these prefixes to the local provider port.
  // Both speak the OpenAI /v1/models shape.
  ollama: "/proxy/ollama",
  lmstudio: "/proxy/lmstudio",
};

const LOCAL_PROVIDER_BASE_URL: Record<"ollama" | "lmstudio", string> = {
  ollama: "http://localhost:11434/v1",
  lmstudio: "http://localhost:1234/v1",
};

// True when a model name is an embedding model and shouldn't be picked
// as the chat default. The list is heuristic but covers the common
// patterns (nomic-embed-*, all-minilm, *bert*, generic *embed*).
export function isEmbeddingModel(name: string): boolean {
  const n = name.toLowerCase();
  return (
    n.includes("embed") ||
    n.includes("minilm") ||
    n.includes("bert") ||
    n.includes("e5-")
  );
}

export async function detectLocalLlm(
  provider: "ollama" | "lmstudio",
): Promise<LocalLlmDiscovery> {
  if (canUseTauriCommands()) {
    return invokeLocal<LocalLlmDiscovery>("detect_local_llm", { provider });
  }
  const proxy = LOCAL_PROVIDER_PROXY[provider];
  const base_url = LOCAL_PROVIDER_BASE_URL[provider];
  try {
    const res = await fetch(`${proxy}/v1/models`, {
      method: "GET",
      headers: { Accept: "application/json" },
      // We're talking to localhost — short timeout via AbortSignal.
      signal: AbortSignal.timeout(2500),
    });
    if (!res.ok) {
      return {
        provider,
        base_url,
        reachable: false,
        models: [],
        error: `${provider}: HTTP ${res.status}`,
      };
    }
    const body: OpenAIModelsResponse = await res.json();
    const models = (body.data ?? []).map((m) => m.id);
    return { provider, base_url, reachable: true, models };
  } catch (err: unknown) {
    return {
      provider,
      base_url,
      reachable: false,
      models: [],
      error: err instanceof Error ? err.message : String(err),
    };
  }
}

// Pick the best chat model from a discovered list. Skips embedding
// models, prefers larger models (those whose name contains a higher
// parameter-size hint like ":35b" or ":70b"), falls back to the first
// non-embedding model, and finally to whatever's there.
export function pickChatModel(models: string[]): string | null {
  const chat = models.filter((m) => !isEmbeddingModel(m));
  if (chat.length === 0) return models[0] ?? null;
  // Score by parameter-size hint embedded in the name (e.g. "qwen3:35b").
  const score = (name: string): number => {
    const lower = name.toLowerCase();
    if (lower.includes("cloud")) return -1; // remote variants should not auto-win local picks
    const m = lower.match(/:(\d+(?:\.\d+)?)\s*b\b/);
    if (m) return parseFloat(m[1]);
    return 1; // unknown size, neutral
  };
  return [...chat].sort((a, b) => score(b) - score(a))[0];
}

export const deleteCloudCredential = async (service: string) => {
  try {
    const out = await api.delete<unknown>("/api/cloud/credentials", { service });
    if (shouldTryTauriCommands()) {
      writeLocalCloudCredentials(readLocalCloudCredentials().filter((credential) => credential.service !== service));
    }
    return out;
  } catch (err) {
    if (shouldTryTauriCommands()) {
      writeLocalCloudCredentials(readLocalCloudCredentials().filter((credential) => credential.service !== service));
      return { deleted: true, local: true };
    }
    throw err;
  }
};

// ─── Plugins ────────────────────────────────────────────────────

export interface PluginStatus {
  name: string;
  version: string;
  description: string;
  state: string;
  tool_count: number;
  expected_lanes: string[];
  enabled: boolean;
  command: string;
  args: string[];
  required_env: string[];
  env: Record<string, string>;
  core_override: boolean;
  manifest_path: string;
  failure?: string | null;
}

export interface PluginManifestDraft {
  name: string;
  version?: string;
  description?: string;
  command: string;
  args?: string[];
  expected_lanes: string[];
  required_env?: string[];
  env?: Record<string, string>;
  core_override?: boolean;
  enabled?: boolean;
}

const normalizePlugins = (out: Partial<{ plugins: PluginStatus[]; count: number }>) => {
  const plugins = Array.isArray(out.plugins) ? out.plugins : [];
  return {
    count: typeof out.count === "number" ? out.count : plugins.length,
    plugins,
  };
};

export const listPlugins = async () => {
  return normalizePlugins(
    await localOrRemote<Partial<{ plugins: PluginStatus[]; count: number }>>(
      "list_local_plugins",
      undefined,
      () => api.get<Partial<{ plugins: PluginStatus[]; count: number }>>("/api/plugins"),
      () => ({ plugins: [], count: 0 }),
    ),
  );
};

export const installPlugin = (manifest: PluginManifestDraft) =>
  canUseTauriCommands()
    ? invokeLocal<PluginStatus>("install_local_plugin", { manifest })
    : api.post<PluginStatus>("/api/plugins", manifest);

export const updatePlugin = (name: string, manifest: PluginManifestDraft) =>
  canUseTauriCommands()
    ? invokeLocal<PluginStatus>("update_local_plugin", { name, manifest })
    : api.put<PluginStatus>(`/api/plugins/${encodeURIComponent(name)}`, manifest);

export const setPluginEnabled = (name: string, enabled: boolean) =>
  canUseTauriCommands()
    ? invokeLocal<PluginStatus>("set_local_plugin_enabled", { name, enabled })
    : api.patch<PluginStatus>(`/api/plugins/${encodeURIComponent(name)}`, { enabled });

export const deletePlugin = (name: string) =>
  canUseTauriCommands()
    ? invokeLocal<{ deleted: boolean; name: string }>("delete_local_plugin", { name })
    : api.delete<{ deleted: boolean; name: string }>(`/api/plugins/${encodeURIComponent(name)}`);

export interface InstalledSkillFile {
  id: string;
  content: string;
  path: string;
}

export const getInstalledSkill = (id: string) =>
  canUseTauriCommands()
    ? invokeLocal<InstalledSkillFile>("get_local_skill", { id })
    : api.get<InstalledSkillFile>(`/api/skills/${encodeURIComponent(id)}`);

export const updateInstalledSkill = (id: string, content: string) =>
  canUseTauriCommands()
    ? invokeLocal<{ id: string; updated: boolean; path: string }>("update_local_skill", { id, content })
    : api.put<{ id: string; updated: boolean; path: string }>(`/api/skills/${encodeURIComponent(id)}`, { content });

export const deleteInstalledSkill = (id: string) =>
  canUseTauriCommands()
    ? invokeLocal<{ deleted: boolean; id: string }>("delete_local_skill", { id })
    : api.delete<{ deleted: boolean; id: string }>(`/api/skills/${encodeURIComponent(id)}`);

// ─── MCP servers ────────────────────────────────────────────────

export interface McpServer {
  server_id: string;
  trust_state: string;
  installed_at: string;
  tool_count: number;
  privilege_tier?: string;
  drift?: string | null;
  lockfile_hash?: string;
}

const normalizeMcpServers = (out: Partial<{ servers: McpServer[]; count: number }>) => {
  const servers = Array.isArray(out.servers) ? out.servers : [];
  return {
    count: typeof out.count === "number" ? out.count : servers.length,
    servers,
  };
};

export const listMcpServers = async () => {
  return normalizeMcpServers(
    await localOrRemote<Partial<{ servers: McpServer[]; count: number }>>(
      "list_local_mcp_servers",
      undefined,
      () => api.get<Partial<{ servers: McpServer[]; count: number }>>("/api/mcp/servers"),
      () => ({ servers: [], count: 0 }),
    ),
  );
};

export const installMcpServer = (payload: Record<string, unknown>) =>
  api.post<unknown>("/api/mcp/servers/install", payload);

export const inspectMcpServer = (server_id: string) =>
  api.post<unknown>("/api/tools/mcp.servers.inspect", { server_id });

export const uninstallMcpServer = (server_id: string) =>
  api.post<unknown>("/api/tools/mcp.servers.uninstall", { server_id });

// ─── Connections ────────────────────────────────────────────────

export interface ConnectionType {
  id: string;
  label: string;
  description?: string;
  service?: string;
  fields?: Array<{ key: string; label: string; required?: boolean; secret?: boolean }>;
}

export const listConnectionTypes = () =>
  localOrRemote<{ types: ConnectionType[] }>(
    "list_local_connection_types",
    undefined,
    () => api.get<{ types: ConnectionType[] }>("/api/connections/types"),
    () => ({ types: [] }),
  );

export const testConnection = (id: string, payload?: Record<string, unknown>) =>
  api.post<unknown>(`/api/connections/${encodeURIComponent(id)}/test`, payload ?? {});

// ─── Apps + Files ───────────────────────────────────────────────

export interface AppRow {
  id: string;
  workspace_id: string;
  slug: string;
  name: string;
  description: string;
  status: "draft" | "published" | "archived" | string;
  created_at: string;
  updated_at: string;
}

export const listApps = (workspace_id = "local") =>
  localOrRemote<{ apps: AppRow[]; count: number }>(
    "list_local_apps",
    undefined,
    () => api.get<{ apps: AppRow[]; count: number }>(
      `/api/apps?workspace_id=${encodeURIComponent(workspace_id)}`,
    ),
    () => ({ apps: [], count: 0 }),
  );

export const createApp = (payload: Record<string, unknown>) =>
  api.post<AppRow>("/api/apps", payload);

export const publishApp = (id: string, actor = "operator") =>
  api.post<AppRow>(`/api/apps/${encodeURIComponent(id)}/publish`, { actor });

export const archiveApp = (id: string, actor = "operator") =>
  api.post<AppRow>(`/api/apps/${encodeURIComponent(id)}/archive`, { actor });

export interface FileRow {
  id: string;
  workspace_id: string;
  original_name: string;
  storage_path: string;
  content_type: string;
  size_bytes: number;
  sha256_hex: string;
  created_at: string;
  created_by: string;
  app_id?: string | null;
}

export const listFiles = (workspace_id = "local") =>
  localOrRemote<{ files: FileRow[]; count: number }>(
    "list_local_files",
    undefined,
    () => api.get<{ files: FileRow[]; count: number }>(
      `/api/files?workspace_id=${encodeURIComponent(workspace_id)}`,
    ),
    () => ({ files: [], count: 0 }),
  );

export const uploadFileBase64 = (payload: {
  original_name: string;
  data_base64: string;
  content_type?: string;
  workspace_id?: string;
  app_id?: string;
  created_by?: string;
}) => api.post<FileRow>("/api/files", payload);

// ─── Security ────────────────────────────────────────────────────

export interface SecurityRule {
  id: string;
  description: string;
  severity: string;
  phases: string;
  enabled: boolean;
}

export interface AuditEntry {
  id: string;
  scope: string;
  capability: string;
  outcome: string;
  severity?: string;
  timestamp: string;
  detail?: string;
}

export const listSecurityRules = () =>
  localOrRemote<{ rules: SecurityRule[]; count: number }>(
    "list_local_security_rules",
    undefined,
    () => api.get<{ rules: SecurityRule[]; count: number }>("/api/security/rules"),
    () => ({ rules: [], count: 0 }),
  );

export const listSecurityAudit = (limit = 100) =>
  localOrRemote<{ entries: AuditEntry[]; count: number }>(
    "list_local_security_audit",
    { limit },
    () => api.get<{ entries: AuditEntry[]; count: number }>(`/api/security/audit?limit=${limit}`),
    () => ({ entries: [], count: 0 }),
  );

// ─── Review ──────────────────────────────────────────────────────

export interface ReviewRequest {
  id: string;
  title: string;
  state: "pending" | "approved" | "denied" | string;
  capability?: string;
  arguments?: unknown;
  created_at: string;
  decided_at?: string | null;
  decided_by?: string | null;
}

// The runtime returns the list under `pending` (not `requests`) on
// the pending endpoint and `recent` on the recent endpoint. Match
// the wire shape exactly so callers don't read `undefined` arrays.
export const listReviewPending = () =>
  localOrRemote<{ pending: ReviewRequest[]; count: number }>(
    "list_local_review_pending",
    undefined,
    () => api.get<{ pending: ReviewRequest[]; count: number }>("/api/review/pending"),
    () => ({ pending: [], count: 0 }),
  );

export const listReviewRecent = (limit = 50) =>
  localOrRemote<{ recent: ReviewRequest[]; count: number }>(
    "list_local_review_recent",
    { limit },
    () => api.get<{ recent: ReviewRequest[]; count: number }>(`/api/review/recent?limit=${limit}`),
    () => ({ recent: [], count: 0 }),
  );

export const approveReview = (id: string, actor = "operator", note?: string) =>
  api.post<unknown>(`/api/review/${encodeURIComponent(id)}/approve`, { actor, note });

export const denyReview = (id: string, actor = "operator", reason?: string) =>
  api.post<unknown>(`/api/review/${encodeURIComponent(id)}/deny`, { actor, reason });

// ─── Self-heal / medbay ──────────────────────────────────────────

export interface SelfHealCase {
  id?: string;
  fingerprint: string;
  symptom: string;
  classified?: string | null;
  actions?: string[];
  pinned?: boolean;
  replay_count?: number;
  last_seen_at?: string;
}

export const listSelfHealCases = (limit = 50) =>
  canUseTauriCommands()
    ? invokeLocal<unknown>("list_local_self_heal_cases", { limit })
    : api.post<unknown>("/api/tools/self_heal.list_cases", { limit });

export const replaySelfHealCase = (id: string) =>
  api.post<unknown>("/api/self-heal/cases/replay", { id });

export const pinSelfHealCase = (id: string) =>
  api.post<unknown>("/api/self-heal/cases/pin", { id });

export const exportSelfHealCase = (id: string) =>
  api.post<unknown>("/api/self-heal/cases/export", { id });

// ─── Assistant ───────────────────────────────────────────────────

/// Multimodal attachment shape on the wire. Mirrors
/// `ordo_protocol::UserAttachment` — kept in sync with the Rust enum
/// so the studio can ship operator-attached images directly to the
/// LLM provider via the runtime's translator. New variants are
/// additive (consumers skip unknown variants), so adding e.g.
/// `audio_base64` later is non-breaking.
export type UserAttachmentPayload =
  | { type: "image_url"; url: string }
  | { type: "image_base64"; data: string; media_type: string };

export interface AssistantTurnRequest {
  user_message: string;
  session_id?: string;
  credential?: string;
  use_rag?: boolean;
  use_memory?: boolean;
  use_tools?: boolean;
  review?: boolean;
  /// When true the assistant streams TokenDelta events on
  /// /ws/assistant/<session_id> while the turn is in flight. The
  /// HTTP response still returns the full final turn.
  stream?: boolean;
  /// Multimodal attachments — typically images. The runtime
  /// translates these into provider-native blocks (OpenAI vision
  /// or Anthropic image blocks).
  attachments?: UserAttachmentPayload[];
  /// Free-form metadata echoed into audit. The studio uses this to
  /// pass operator-side skill toggles (`disabled_skills`,
  /// `custom_skills`, `uploaded_files`) so future runtime versions
  /// can honor them without a wire-shape change.
  metadata?: Record<string, unknown>;
  /**
   * Mode-scoped workspace for THIS request. Only consulted when
   * the request creates a new session (session_id is None and the
   * runtime auto-creates one). For an existing session, the
   * session's stored mode wins; this field is ignored — the
   * architecture doesn't allow mid-session mode changes.
   */
  mode?: string;
}

export interface AssistantSessionRecord {
  id: string;
  title?: string | null;
  created_at: string;
  updated_at?: string;
  turn_count?: number;
  /**
   * Mode-scoped workspace this session is bound to. Fixed at session
   * creation; switching modes in the UXI creates a new session, never
   * rewrites this field. Defaults to "general" for legacy sessions
   * created before modes existed.
   */
  mode?: string;
}

// Tool wrapper for assistant.new_session — returns a fresh session id
// the studio uses to open a WS subscription before sending its first
// streaming turn. The optional `mode` argument binds the new session
// to a mode-scoped workspace (vibe_coding, writing, etc.); falls
// back to the registry's default ("general") on the runtime side
// when omitted.
export const newAssistantSession = async (
  title?: string,
  mode?: string,
): Promise<AssistantSessionRecord> => {
  const body: Record<string, string> = {};
  if (title) body.title = title;
  if (mode) body.mode = mode;
  const out = (await api.post<unknown>(
    "/api/tools/assistant.new_session",
    body,
  )) as { session?: AssistantSessionRecord } | AssistantSessionRecord;
  if ("session" in (out as Record<string, unknown>) && (out as { session: AssistantSessionRecord }).session) {
    return (out as { session: AssistantSessionRecord }).session;
  }
  return out as AssistantSessionRecord;
};

export interface AssistantSessionsResponse {
  count: number;
  sessions: AssistantSessionRecord[];
}

export const listAssistantSessions = (limit = 50) =>
  api.get<AssistantSessionsResponse>(
    `/api/assistant/sessions?limit=${encodeURIComponent(String(limit))}`,
  );

// Open a WebSocket to the per-session event stream. Caller wires the
// onEvent callback. The URL is relative so the vite proxy / Tauri
// origin handles forwarding.
export interface TurnStreamHandle {
  close: () => void;
}

export type TurnEvent =
  | { event: "turn_started"; session_id: string; user_message: string }
  | { event: "context_retrieved"; session_id: string }
  | { event: "tool_call_started"; session_id: string; capability: string }
  | { event: "tool_call_completed"; session_id: string; capability: string }
  | { event: "tool_call_failed"; session_id: string; capability: string; error: string }
  | { event: "token_delta"; session_id: string; delta: string }
  | { event: "turn_completed"; session_id: string; turn: AssistantTurnRecord }
  | { event: "turn_failed"; session_id: string; error: string }
  // Mode-scoped workspace events (ordo-modes step 8). These let the
  // insight trace render "why this turn ran in mode X with these
  // constraints" without re-fetching the manifest.
  | {
      event: "mode_bound";
      session_id: string;
      mode_id: string;
      mode_label: string;
      memory_scope: string[];
      rag_domains: string[];
      allowed_tool_lane_count: number;
      blocked_tool_capability_count: number;
    }
  | {
      event: "mode_memory_scope_applied";
      session_id: string;
      mode_id: string;
      visible_scopes: string[];
      facts_visible: number;
    }
  | {
      event: "mode_tool_filter_applied";
      session_id: string;
      mode_id: string;
      kept_capabilities: number;
      filtered_count: number;
    }
  // Cross-mode consultation events. These are agent-to-agent
  // consultations, not raw RAG or memory borrowing.
  | {
      event: "cross_mode_consult_requested";
      session_id: string;
      active_mode: string;
      target_mode: string;
      reason: string;
      question: string;
    }
  | {
      event: "cross_mode_consult_approved";
      session_id: string;
      active_mode: string;
      target_mode: string;
    }
  | {
      event: "cross_mode_consult_denied";
      session_id: string;
      active_mode: string;
      target_mode: string;
      reason: string;
    }
  | {
      event: "cross_mode_consult_completed";
      session_id: string;
      active_mode: string;
      target_mode: string;
      turn_id: string;
    }
  | { event: string; [k: string]: unknown };

export function openAssistantStream(
  sessionId: string,
  onEvent: (e: TurnEvent) => void,
  onError?: (err: Event) => void,
): TurnStreamHandle {
  // Use the page's protocol so https → wss in production builds.
  const ws = new WebSocket(websocketUrl(`/ws/assistant/${encodeURIComponent(sessionId)}`));
  ws.onmessage = (msg) => {
    try {
      const parsed = JSON.parse(msg.data) as TurnEvent;
      onEvent(parsed);
    } catch {
      // Server may send pings or ws-level frames that aren't JSON.
    }
  };
  if (onError) ws.onerror = onError;
  return {
    close: () => {
      try {
        ws.close();
      } catch {
        // ignore
      }
    },
  };
}

export interface AssistantTurnRecord {
  id: string;
  session_id: string;
  index: number;
  user_message: string;
  assistant_response: string;
  model?: string | null;
  credential_service?: string | null;
  context?: {
    history_window?: number;
    facts?: unknown[];
    rag_hits?: unknown[];
    tool_calls?: Array<{
      capability?: string;
      arguments?: unknown;
      result?: unknown;
    }>;
  };
  created_at: string;
}

export interface AssistantTurnResponse {
  session_id: string;
  turn: AssistantTurnRecord;
  retrieved_facts?: unknown[];
  retrieved_rag?: unknown[];
  review_outcome?: unknown;
}

export const postAssistantTurn = (req: AssistantTurnRequest) =>
  api.post<AssistantTurnResponse>("/api/assistant/turn", req);

export interface VoiceSpeechRequest {
  input: string;
  service?: string;
  model?: string;
  voice?: string;
  format?: string;
  instructions?: string;
  speed?: number;
}

export interface VoiceSpeechResponse {
  blob: Blob;
  contentType: string;
  provider: string | null;
  model: string | null;
  voice: string | null;
  format: string | null;
}

export const postVoiceSpeech = async (
  req: VoiceSpeechRequest,
): Promise<VoiceSpeechResponse> => {
  const res = await fetch(apiUrl("/api/voice/speech"), {
    method: "POST",
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify(req),
  });
  if (!res.ok) {
    let parsed: unknown = undefined;
    const text = await res.text();
    if (text.length > 0) {
      try {
        parsed = JSON.parse(text);
      } catch {
        parsed = text;
      }
    }
    throw new ApiError(res.status, parsed, `POST /api/voice/speech -> ${res.status}`);
  }
  const blob = await res.blob();
  return {
    blob,
    contentType: res.headers.get("content-type") ?? blob.type,
    provider: res.headers.get("x-ordo-tts-provider"),
    model: res.headers.get("x-ordo-tts-model"),
    voice: res.headers.get("x-ordo-tts-voice"),
    format: res.headers.get("x-ordo-tts-format"),
  };
};

export const cancelAssistantTurn = (sessionId: string) =>
  api.post<{ cancelled: boolean }>(
    `/api/assistant/sessions/${encodeURIComponent(sessionId)}/cancel`,
  );

// Session detail (header + ordered turn list). Used by the shell to
// repopulate the chat dock when the operator reopens the studio with
// a previously-stored session id in localStorage. The runtime drops
// sessions on restart, so a 404 here is normal — the shell catches
// it and starts fresh.
export interface AssistantSessionWithTurns {
  session: AssistantSessionRecord;
  turns: AssistantTurnRecord[];
}

export const fetchAssistantSession = (sessionId: string) =>
  api.get<AssistantSessionWithTurns>(
    `/api/assistant/sessions/${encodeURIComponent(sessionId)}`,
  );

// ─── Modes (mode-scoped workspaces) ─────────────────────────────

/**
 * One mode-scoped workspace. Mirrors `ordo_modes::ModeManifest`.
 * The picker in the chat header reads `id`, `label`, `description`;
 * the advanced view reads everything.
 *
 * Fields are optional/nullable because the JSON wire shape uses
 * serde defaults — an absent `default_timeout_secs` means "inherit
 * the operator's global preset," not "0 seconds."
 */
export interface AssistantMode {
  id: string;
  label: string;
  description: string;
  memory_scope: string[];
  rag_domains: string[];
  allowed_tool_lanes: string[];
  blocked_tool_capabilities: string[];
  policies: string[];
  planner_bias: string[];
  persona: string[];
  default_timeout_secs?: number | null;
  default_strictness?: string | null;
  default_credential?: string | null;
  cross_mode_borrow_policy?: string | null;
  cross_mode_consult_policy?: string | null;
  /** Built-in core mode — can't be deleted without force. */
  protected?: boolean;
}

export interface AssistantModesResponse {
  count: number;
  modes: AssistantMode[];
}

const normalizeAssistantModes = (out: Partial<AssistantModesResponse>): AssistantModesResponse => {
  const modes = Array.isArray(out.modes) ? out.modes : [];
  return {
    count: typeof out.count === "number" ? out.count : modes.length,
    modes,
  };
};

/**
 * List all registered modes. Returns an empty array when the
 * runtime has no mode registry attached (legacy / config error);
 * the studio degrades gracefully to "general only" in that case.
 */
export const listAssistantModes = async (): Promise<AssistantModesResponse> => {
  return normalizeAssistantModes(
    await localOrRemote<Partial<AssistantModesResponse>>(
      "list_local_modes",
      undefined,
      () => api.get<Partial<AssistantModesResponse>>("/api/assistant/modes"),
      () => ({ modes: [], count: 0 }),
    ),
  );
};

/** Full manifest for one mode — used by the advanced view. */
export const fetchAssistantMode = (id: string) =>
  api.get<AssistantMode>(`/api/assistant/modes/${encodeURIComponent(id)}`);

/**
 * Create a new (unprotected) mode. Pass just a name — the runtime slugifies
 * the id and fills safe General-like defaults; the operator tunes it after.
 */
export const createAssistantMode = (name: string) =>
  api.post<AssistantMode>("/api/assistant/modes", { name });

/**
 * Delete a mode. Protected core modes are refused unless `force` is set, so
 * an operator can't casually remove `general`, `diagnostic`, etc.
 */
export const deleteAssistantMode = (id: string, force = false) =>
  api.delete<{ deleted: string }>(
    `/api/assistant/modes/${encodeURIComponent(id)}${force ? "?force=true" : ""}`,
  );

/**
 * Update a mode. The runtime expects the FULL manifest (the route injects
 * `id`), so callers fetch the mode, change fields, and send the whole thing
 * back — preserving every field they didn't touch.
 */
export const updateAssistantMode = (id: string, manifest: Record<string, unknown>) =>
  api.patch<AssistantMode>(`/api/assistant/modes/${encodeURIComponent(id)}`, manifest);

// ─── Avatar brain (its own LLM endpoint, bound to the `avatar` mode) ───
//
// The avatar runs as a second assistant on its OWN model so it works
// concurrently with the main assistant — but on the SHARED brain (same
// memory/RAG/skills). We store its endpoint as a dedicated credential
// (`avatar-brain`) in the shared vault and bind it to the `avatar` mode's
// default_credential; the assistant turn then routes avatar-mode turns to it.

export const AVATAR_BRAIN_SERVICE = "avatar-brain";
export const AVATAR_MODE_ID = "avatar";

export interface AvatarBrain {
  kind: "local" | "cloud";
  baseUrl: string;
  model: string;
  /** Cloud only; omitted/blank for local (preserved on save if blank). */
  apiKey?: string;
}

/** Read the current avatar-brain config: the bound credential + its model. */
export const getAvatarBrain = async (): Promise<{
  bound: boolean;
  service: string | null;
  baseUrl: string | null;
  model: string | null;
}> => {
  let bound = AVATAR_BRAIN_SERVICE;
  try {
    const mode = await fetchAssistantMode(AVATAR_MODE_ID);
    bound = mode.default_credential || AVATAR_BRAIN_SERVICE;
  } catch {
    // mode unreadable — fall back to the convention name
  }
  try {
    const { credentials } = await listCloudCredentials();
    const cred = credentials.find((c) => c.service === bound);
    return {
      bound: Boolean(cred),
      service: cred ? bound : null,
      baseUrl: cred?.base_url ?? cred?.endpoint ?? null,
      model: cred?.extras?.model ?? null,
    };
  } catch {
    return { bound: false, service: null, baseUrl: null, model: null };
  }
};

/**
 * Configure the avatar's own LLM endpoint and bind it to the `avatar` mode.
 * Upserts the `avatar-brain` credential (base_url + model in extras), then
 * patches the avatar mode's `default_credential`. An empty apiKey preserves
 * any stored secret (and is fine for local endpoints, which ignore it).
 */
export const setAvatarBrain = async (brain: AvatarBrain): Promise<void> => {
  await upsertCloudCredential({
    service: AVATAR_BRAIN_SERVICE,
    label: "Avatar brain",
    auth_style: "bearer",
    // Local servers (Ollama/llama.cpp) ignore the bearer token; a placeholder
    // keeps the credential valid. Cloud uses the real key (blank = preserve).
    secret: brain.apiKey && brain.apiKey.length > 0
      ? brain.apiKey
      : brain.kind === "local"
        ? "local"
        : "",
    base_url: brain.baseUrl,
    extras: { model: brain.model, avatar_brain: "true", kind: brain.kind },
  });
  const mode = (await fetchAssistantMode(AVATAR_MODE_ID)) as unknown as Record<
    string,
    unknown
  >;
  await updateAssistantMode(AVATAR_MODE_ID, {
    ...mode,
    default_credential: AVATAR_BRAIN_SERVICE,
  });
};

// ─── Operator persona / agent facts ─────────────────────────────

// The runtime stores facts as triples (subject / predicate / object).
// We surface them as a single readable string in the UI; the studio
// does the triple↔string translation. New entries default subject to
// "operator" and predicate to "note" so the user types one line.
export interface AssistantFact {
  id?: string;
  subject?: string;
  predicate?: string;
  object?: string;
  // Studio-side composite for display.
  content: string;
  created_at?: string;
}

const factToString = (f: { subject?: string; predicate?: string; object?: string }): string => {
  const subj = (f.subject ?? "").trim();
  const pred = (f.predicate ?? "").trim();
  const obj = (f.object ?? "").trim();
  if (subj.toLowerCase() === "operator" && pred.toLowerCase() === "note") return obj;
  return [subj, pred, obj].filter(Boolean).join(" · ");
};

function flattenFacts(payload: unknown): AssistantFact[] {
  const raw = (() => {
    if (Array.isArray(payload)) return payload;
    if (typeof payload === "object" && payload !== null) {
      const obj = payload as Record<string, unknown>;
      for (const key of ["facts", "results", "items"]) {
        const v = obj[key];
        if (Array.isArray(v)) return v;
      }
    }
    return [];
  })();
  return raw.map((x: unknown) => {
    if (typeof x === "string") return { content: x };
    const f = x as Record<string, unknown>;
    const subject = typeof f.subject === "string" ? f.subject : undefined;
    const predicate = typeof f.predicate === "string" ? f.predicate : undefined;
    const object = typeof f.object === "string" ? f.object : undefined;
    const id = typeof f.id === "string" ? f.id : undefined;
    const created_at = typeof f.created_at === "string" ? f.created_at : undefined;
    return {
      id,
      subject,
      predicate,
      object,
      created_at,
      content:
        typeof f.content === "string"
          ? (f.content as string)
          : factToString({ subject, predicate, object }),
    };
  });
}

export const listAssistantFacts = async (
  subject?: string,
): Promise<AssistantFact[]> => {
  if (canUseTauriCommands()) {
    const out = await invokeLocal<unknown>("list_local_assistant_facts", { subject: subject ?? null });
    return flattenFacts(out);
  }
  const out = await api.post<unknown>(
    "/api/tools/assistant.list_facts",
    subject ? { subject, limit: 200 } : { limit: 200 },
  );
  return flattenFacts(out);
};

// Free-text persona entries default to (operator, note, <content>);
// callers wanting richer triples can pass them explicitly.
export const rememberFact = (
  content: string,
  triple?: { subject?: string; predicate?: string; object?: string },
) =>
  api.post<unknown>("/api/tools/assistant.remember_fact", {
    subject: triple?.subject ?? "operator",
    predicate: triple?.predicate ?? "note",
    object: triple?.object ?? content,
  });

export const forgetFact = (id: string) =>
  api.post<unknown>("/api/tools/assistant.forget_fact", { id });

// ─── Tool invocation ─────────────────────────────────────────────

// Invoke any capability advertised on the bus. The runtime accepts an
// optional JSON body as the arguments object; absence is treated as
// `null`. Returns whatever the provider returned, untyped.
export const invokeTool = (capability: string, args: unknown) =>
  api.post<unknown>(`/api/tools/${encodeURIComponent(capability)}`, args ?? {});

// ─── Conversation taint (Phase B) ───────────────────────────────

export type TaintSource =
  | { kind: "trusted" }
  | { kind: "user" }
  | { kind: "verified_provider" }
  | {
      kind: "untrusted_web";
      source_url: string;
      fetched_at: string;
    }
  | {
      kind: "untrusted_mcp";
      server_id: string;
      invocation_id: string;
    }
  | { kind: "mixed"; sources: TaintSource[] };

export interface SessionTaintState {
  session_id: string;
  tainted: boolean;
  sources: TaintSource[];
}

export const fetchSessionTaint = (sessionId: string) =>
  localOrRemote<SessionTaintState>(
    "get_local_session_taint",
    { session_id: sessionId },
    () => api.get<SessionTaintState>(
      `/api/assistant/sessions/${encodeURIComponent(sessionId)}/taint`,
    ),
    () => ({ session_id: sessionId, tainted: false, sources: [] }),
  );

export const clearSessionTaint = (sessionId: string) =>
  api.post<{ session_id: string; cleared: boolean }>(
    `/api/assistant/sessions/${encodeURIComponent(sessionId)}/taint/clear`,
  );

// ─── System helpers ─────────────────────────────────────────────

// Walk a small set of candidate paths anchored on the runtime's
// location and return the first existing one. Used by the MCP tab to
// auto-detect the ordo-mcp executable so the operator doesn't have to know where
// the binary lives. `found` is null when nothing matched; `candidates`
// lists what the runtime checked so the UI can hint about it.
export interface FindBinaryResponse {
  name: string;
  found: string | null;
  candidates: string[];
}
export const findBinary = (name: string) =>
  canUseTauriCommands()
    ? invokeLocal<FindBinaryResponse>("find_local_binary", { name })
    : api.get<FindBinaryResponse>(
        `/api/system/find_binary?name=${encodeURIComponent(name)}`,
      );

// ─── Health ───────────────────────────────────────────────────────

export const fetchHealth = () =>
  canUseTauriCommands()
    ? invokeLocal<{ status: string }>("get_local_health")
    : api.get<{ status: string }>("/health");

// ─── Voice-to-text (dictation) ────────────────────────────────────

/**
 * Transcribe recorded audio to text via the agnostic STT endpoint
 * (OpenAI-compatible `/audio/transcriptions`, local or cloud). Used by the
 * Studio chat composer's dictation mic — this is voice-to-TEXT only; it does
 * NOT speak back (that's the avatar's separate voice-to-voice loop).
 */
export const transcribeAudio = (
  audioBase64: string,
  format: string,
  service?: string,
): Promise<{ text: string; provider?: string; model?: string }> =>
  api.post<{ text: string; provider?: string; model?: string }>(
    "/api/voice/transcribe",
    { audio_base64: audioBase64, format, ...(service ? { service } : {}) },
  );

// ─── Avatar pop-out ───────────────────────────────────────────────

/**
 * Open the avatar in its own resizable pop-out window — the intended
 * use is dragging it onto a spare monitor. Inside the Tauri desktop
 * shell this spawns a native OS window via the WebviewWindow API; in a
 * plain browser it falls back to `window.open`. Either way the window
 * loads the avatar page served by the control API, so the page's
 * relative `/sse/avatar` + `/api/avatar/speak` calls stay same-origin.
 *
 * The runtime only emits avatar frames when started with
 * `ORDO_ENABLE_AVATAR=1`; without it the window renders the idle face
 * and the Speak box still drives the lip-sync stub once enabled.
 */
/** Absolute URL of the avatar page served by the control API. Use this
 *  for an inline `<iframe>` preview or to open the pop-out window. */
export function avatarPageUrl(): string {
  return `${CONTROL_API_ORIGIN}/avatar.html`;
}

export async function openAvatarPopout(): Promise<void> {
  const url = avatarPageUrl();
  let tauriErr: unknown = null;
  // In the desktop shell, open the avatar in the SYSTEM BROWSER via a Rust
  // command. The embedded WebView2 denies microphone access (no host-side
  // permission handler), so a real browser is the reliable surface for the
  // voice avatar — and it opens dependably, unlike a native WebviewWindow.
  if (canUseTauriCommands()) {
    try {
      await invokeLocal<void>("open_external_url", { url });
      return;
    } catch (err) {
      tauriErr = err;
      console.warn("[avatar] open_external_url failed; trying window.open:", err);
    }
  }
  // Plain-browser context (and the dev origin): open a spare window directly.
  const opened = window.open(url, "ordo-avatar", "popup,width=380,height=560,resizable=yes");
  if (!opened) {
    throw new Error(
      "Couldn't open the avatar window. " +
        (tauriErr
          ? `Shell error: ${tauriErr instanceof Error ? tauriErr.message : String(tauriErr)}. `
          : "") +
        `Open ${url} directly in a browser as a workaround.`,
    );
  }
}
