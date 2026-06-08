// Ordo by Lucerna Labs — main studio shell.
//
// 41-tab UXI mapping one-to-one onto the ordo-* crate set, plus Bus as
// cross-cutting telemetry. All controls are stubbed; wiring notes:
//
//   - Light signals subscribe to /api/runtime/profile, /api/runtime/storage,
//     and the capability heartbeat stream.
//   - Sliders POST to /api/runtime/settings.
//   - Pinned memory CRUD already maps to existing endpoints.
//   - Bus tab is a websocket subscriber, not a polling loop.
//
// Two surfaces are deliberately stubbed pending design decisions:
//   - Approval render pane above the chat input (artifact preview before
//     user approves an assistant action). Layout slot is reserved below
//     as `<approval-render-slot>`; render content comes later.
//   - Rescue Mode amber flood is wired here at the shell root, gated on
//     the gateway signal flipping to `err`.

import { useEffect, useLayoutEffect, useMemo, useRef, useState } from "react";
import type { CSSProperties, ReactNode, RefObject } from "react";
import { motion, AnimatePresence } from "framer-motion";
import {
  Alert,
  Badge,
  Button,
  Card,
  Checkbox,
  CommandBlock,
  ConfiguredRow,
  CopyableField,
  Dot,
  Field,
  Modal,
  NumberInput,
  SectionHeader,
  Select,
  TabPills,
  TextInput,
  Textarea,
  ToolCard,
  COLORS as UI,
} from "./ui";
import {
  ApiError,
  approveAutomation,
  approveReview,
  archiveApp,
  cancelAssistantTurn,
  createAutomation,
  createApp,
  deleteAutomation,
  deleteCloudCredential,
  deletePlugin,
  deleteInstalledSkill,
  findBinary,
  detectLocalLlm,
  disableAutomation,
  enableAutomation,
  forgetFact,
  getInstalledSkill,
  installLocalApiKeyEnv,
  installPlugin,
  listAssistantFacts,
  pickChatModel,
  rememberFact,
  deleteWebhook,
  listWebhooks,
  registerWebhook,
  updateWebhook,
  denyReview,
  exportSelfHealCase,
  fetchCapabilities,
  fetchMcpCapabilities,
  fetchRuntimeProfile,
  fetchRuntimeSettings,
  fetchRuntimeStorage,
  inspectMcpServer,
  installMcpServer,
  invokeTool,
  listBuilds,
  listAutomations,
  listApps,
  listCloudCredentials,
  listConnectionTypes,
  listFiles,
  listMcpServers,
  listPinnedMemory,
  listPlugins,
  listReviewPending,
  listReviewRecent,
  listSecurityAudit,
  listSecurityRules,
  listSelfHealCases,
  listAssistantSessions,
  listWorkingMemory,
  pinNote,
  pinSelfHealCase,
  newAssistantSession,
  openAssistantStream,
  postAssistantTurn,
  postVoiceSpeech,
  publishApp,
  fetchRagCollections,
  previewRagCollections,
  replaySelfHealCase,
  testConnection,
  tickAutomations,
  unpinNote,
  uninstallMcpServer,
  updateRuntimeSettings,
  uploadFileBase64,
  upsertCloudCredential,
  setPluginEnabled,
  startBuild,
  submitBuildGateResult,
  updateInstalledSkill,
  updatePlugin,
  type AppRow,
  type AuditEntry,
  type BuildErrorClass,
  type BuildGateOutcome,
  type BuildGateResult,
  type BuildLedger,
  type BuildStep,
  type AutomationIntent,
  type AutomationScope,
  type AutomationSpec,
  type AutomationTrigger,
  type CapabilityDescriptor,
  type CloudCredentialRow,
  type LocalApiKeyInstallResult,
  type ConnectionType,
  type FileRow,
  type McpServer,
  type PluginStatus,
  type PluginManifestDraft,
  type RagCollection,
  type ReviewRequest,
  type RuntimeProfile,
  type RuntimeSettingsSnapshot,
  type RuntimeStorage,
  type AssistantFact,
  type AssistantSessionRecord,
  type SecurityRule,
  type TurnStreamHandle,
  type UserAttachmentPayload,
  type WebhookSubscription,
  fetchSessionTaint,
  clearSessionTaint,
  type SessionTaintState,
  type TaintSource,
  fetchAssistantSession,
  type AssistantTurnRecord,
  listAssistantModes,
  type AssistantMode,
  type TurnEvent,
} from "./api";
import { ExtensionsSurface } from "./extensions/ExtensionsSurface";
import {
  Send,
  MessageSquare,
  Database,
  Brain,
  Boxes,
  Network,
  Plug,
  Server,
  FolderOpen,
  ShieldCheck,
  Eye,
  Stethoscope,
  Cpu,
  Radio,
  ChevronUp,
  ChevronDown,
  Pin,
  PinOff,
  RefreshCcw,
  Plus,
  Lock,
  Unlock,
  Check,
  X,
  Cloud,
  Webhook,
  FileText,
  Copy,
  Trash2,
  Wrench,
  Sparkles,
  User,
  Bot,
  BookMarked,
  Pause,
  Play,
  Image as ImageIcon,
  Paperclip,
  FolderUp,
  Mic,
  MicOff,
  Volume2,
  Download,
  Square,
  Settings as SettingsIcon,
  SlidersHorizontal,
  Search,
  Palette,
  Mail,
  Keyboard,
  GitBranch,
  Terminal,
  Archive,
  Zap,
  Briefcase,
  Monitor,
  Globe,
  Laptop,
  Puzzle,
} from "lucide-react";

const FRAUNCES = "'Fraunces', 'Iowan Old Style', Georgia, serif";
const MONO = "'JetBrains Mono', 'SF Mono', Menlo, monospace";

const INK = "var(--ordo-ink)";
const INK_2 = "var(--ordo-ink-2)";
const PARCHMENT = "var(--ordo-parchment)";
const LAMP = "#f4c95d";
const LAMP_HOT = "#f4a13d";
const JADE = "#7fd1c5";
const VIOLET = "#a99af0";
const ROSE = "#f07f9f";
const PEACH = "#f0b67f";
const SLATE = "#9aa4b2";
const RED = "#e85d5d";

type SignalState = "ok" | "warn" | "err" | "off";
type OrdoTheme = "dark" | "bright";

interface SignalDef {
  id: string;
  label: string;
  state: SignalState;
  detail: string;
}

interface TabDef {
  id: string;
  label: string;
  glyph: typeof MessageSquare;
  group: "primary" | "agent" | "knowledge" | "connectivity" | "advanced" | "docs";
}

const TABS: TabDef[] = [
  // Primary is read top-to-bottom as the operator's daily orchestration:
  //   Provider — make sure something's wired up to answer
  //   Assistant — drive the conversation
  //   Review   — decide on artifacts the assistant queued
  // The tab id stays `cloud` (the surface is still CloudSurface and
  // the runtime routes are still /api/cloud/credentials); only the
  // label changes — "Provider" is what the operator actually picks
  // here, and reads less generic than "Cloud" for a tab that also
  // covers local providers (Ollama, LM Studio).
  { id: "cloud", label: "Provider", glyph: Cloud, group: "primary" },
  { id: "assistant", label: "Assistant", glyph: MessageSquare, group: "primary" },
  { id: "modes", label: "Modes", glyph: SlidersHorizontal, group: "primary" },
  { id: "hooks", label: "Hooks", glyph: ShieldCheck, group: "primary" },
  // Review sits below Assistant because it's operator-essential
  // orchestration — the queue where assistant artifacts wait for
  // approve/deny. The Review tab pulses when there's a pending
  // request so the operator sees it without hunting.
  { id: "review", label: "Review", glyph: Eye, group: "primary" },
  { id: "skills", label: "Skills", glyph: Sparkles, group: "agent" },
  { id: "persona", label: "Persona", glyph: User, group: "agent" },
  { id: "agent-persona", label: "Agent Persona", glyph: Bot, group: "agent" },
  { id: "agent-memory", label: "Agent Memory", glyph: BookMarked, group: "agent" },
  // Apps are agent rigging (deployable units the assistant operates).
  // Files are knowledge material (uploaded artifacts the assistant
  // reads from), so they belong in the knowledge group next to RAG
  // and Memory — not bundled with deployable apps.
  { id: "apps", label: "Apps", glyph: Boxes, group: "agent" },
  // Webhooks lives next to the rest of the agent surfaces because it's
  // an operator-configurable interface — registering callbacks the
  // assistant can fire is part of agent rigging, not deep ops.
  { id: "webhooks", label: "Webhooks", glyph: Webhook, group: "agent" },
  // Plugins + MCP extend the agent's reach (new capabilities the
  // planner can call), so they belong with the rest of agent rigging
  // rather than in a generic "connectivity" group.
  { id: "plugins", label: "Plugins", glyph: Plug, group: "agent" },
  { id: "mcp", label: "MCP", glyph: Server, group: "agent" },
  { id: "extensions", label: "Extensions", glyph: Puzzle, group: "agent" },
  { id: "automation", label: "Automation", glyph: Zap, group: "agent" },
  { id: "builds", label: "Builds", glyph: Wrench, group: "agent" },
  { id: "dreaming", label: "Dreaming", glyph: Brain, group: "agent" },
  { id: "diagnostic", label: "Diagnostic", glyph: Stethoscope, group: "agent" },
  { id: "projects", label: "Projects", glyph: Briefcase, group: "agent" },
  { id: "artifacts", label: "Artifacts", glyph: FileText, group: "agent" },
  { id: "rag", label: "RAG", glyph: Database, group: "knowledge" },
  { id: "files", label: "Files", glyph: FolderOpen, group: "knowledge" },
  { id: "memory", label: "Memory", glyph: Brain, group: "knowledge" },
  // Connections are external data sources the assistant grounds
  // against (services it can pull facts and artifacts from), so
  // they're knowledge material — not connectivity plumbing.
  { id: "connectors", label: "Connectors", glyph: Network, group: "knowledge" },
  { id: "settings-general", label: "General", glyph: SettingsIcon, group: "advanced" },
  { id: "settings-profile", label: "Profile", glyph: User, group: "advanced" },
  { id: "settings-appearance", label: "Appearance", glyph: Palette, group: "advanced" },
  { id: "settings-configuration", label: "Configuration", glyph: SlidersHorizontal, group: "advanced" },
  { id: "settings-personalization", label: "Personalization", glyph: Sparkles, group: "advanced" },
  { id: "settings-keyboard", label: "Keyboard shortcuts", glyph: Keyboard, group: "advanced" },
  { id: "settings-mcp", label: "MCP servers", glyph: Paperclip, group: "advanced" },
  { id: "remote-communication", label: "Remote Communication", glyph: Mail, group: "advanced" },
  { id: "settings-browser", label: "Browser", glyph: Globe, group: "advanced" },
  { id: "settings-computer-use", label: "Computer use", glyph: Monitor, group: "advanced" },
  { id: "connections", label: "Connections", glyph: Globe, group: "advanced" },
  { id: "settings-git", label: "Git", glyph: GitBranch, group: "advanced" },
  { id: "settings-environments", label: "Environments", glyph: Terminal, group: "advanced" },
  { id: "settings-worktrees", label: "Worktrees", glyph: FolderUp, group: "advanced" },
  { id: "archived-chats", label: "Archived chats", glyph: Archive, group: "advanced" },
  { id: "capabilities", label: "Capabilities", glyph: Boxes, group: "advanced" },
  // Security & Health is the merged tab: rules, audit ring (security
  // log), and the self-heal log live together. Used to be two tabs
  // (Security + Medbay); both were log-driven and read more honestly
  // together than apart. Tab id stays `security` so any URL or state
  // restore keeps working.
  { id: "security", label: "Security & Health", glyph: ShieldCheck, group: "advanced" },
  // Runtime owns the Bus surface — no separate tab. Bus is a
  // runtime-level system property (where envelopes flow), not a
  // standalone operator orchestration, so it lives as a card inside the
  // Runtime tab next to profiles + budgets + response timeout.
  { id: "runtime", label: "Runtime", glyph: Cpu, group: "advanced" },
  { id: "settings", label: "Settings", glyph: SettingsIcon, group: "primary" },
  { id: "docs", label: "Docs", glyph: BookMarked, group: "docs" },
  { id: "dev-docs", label: "Dev Docs", glyph: FileText, group: "docs" },
];

const LEFT_RAIL_TAB_IDS = new Set([
  "cloud",
  "assistant",
  "modes",
  "hooks",
  "review",
  "skills",
  "plugins",
  "mcp",
  "extensions",
  "automation",
  "builds",
  "dreaming",
  "diagnostic",
  "projects",
  "settings",
  "docs",
  "dev-docs",
]);

const isSettingsManagedTab = (id: string) =>
  id === "settings" || TABS.some((tab) => tab.id === id && tab.group === "advanced");

const tabLabel = (id: string) => TABS.find((tab) => tab.id === id)?.label ?? "Assistant";

type DirectorySectionId = "skills" | "connectors" | "plugins";
type DirectoryTabId = "skills" | "connectors" | "plugins" | "assistant";

const DIRECTORY_SECTIONS: Array<{
  id: DirectorySectionId;
  label: string;
  tabId: DirectoryTabId;
  glyph: typeof Sparkles;
}> = [
  { id: "skills", label: "Skills", tabId: "skills", glyph: Sparkles },
  { id: "connectors", label: "Connectors", tabId: "connectors", glyph: Network },
  { id: "plugins", label: "Plugins", tabId: "plugins", glyph: Plug },
];

const DirectoryFrame = ({
  active,
  search,
  onSearch,
  placeholder,
  onOpen,
  controls,
  children,
}: {
  active: DirectorySectionId;
  search: string;
  onSearch: (value: string) => void;
  placeholder: string;
  onOpen: (tab: DirectoryTabId) => void;
  controls?: ReactNode;
  children: ReactNode;
}) => (
  <div
    className="h-full"
    style={{
      minHeight: 0,
      borderRadius: 16,
      border: `1px solid ${UI.cardBorderStrong}`,
      background: `linear-gradient(180deg, ${INK_2} 0%, ${INK} 100%)`,
      boxShadow: "0 24px 80px rgba(0,0,0,0.42)",
      overflow: "hidden",
    }}
  >
    <div className="h-full grid" style={{ gridTemplateColumns: "240px minmax(0, 1fr)" }}>
      <aside
        style={{
          padding: "22px 20px",
          borderRight: `1px solid ${UI.cardBorder}`,
          background: "rgba(244,201,93,0.025)",
        }}
      >
        <div
          style={{
            fontFamily: FRAUNCES,
            color: UI.parchment,
            fontSize: 28,
            fontWeight: 650,
            lineHeight: 1,
            marginBottom: 28,
          }}
        >
          Directory
        </div>
        <div className="space-y-1">
          {DIRECTORY_SECTIONS.map((section) => {
            const Icon = section.glyph;
            const selected = active === section.id;
            return (
              <button
                key={section.id}
                type="button"
                onClick={() => onOpen(section.tabId)}
                style={{
                  width: "100%",
                  minHeight: 36,
                  display: "flex",
                  alignItems: "center",
                  gap: 12,
                  padding: "0 14px",
                  borderRadius: 8,
                  border: selected ? `1px solid ${UI.primaryBorder}` : "1px solid transparent",
                  background: selected ? UI.primarySoft : "transparent",
                  color: selected ? UI.primary : UI.textMuted,
                  fontWeight: 700,
                  fontSize: 15,
                  cursor: "pointer",
                }}
              >
                <Icon size={17} strokeWidth={1.9} />
                {section.label}
              </button>
            );
          })}
        </div>
      </aside>
      <section className="h-full flex flex-col" style={{ minWidth: 0, minHeight: 0, padding: "22px 26px 0" }}>
        <div className="flex items-center gap-3" style={{ marginBottom: 18 }}>
          <div style={{ position: "relative", flex: 1 }}>
            <Search
              size={18}
              color={UI.textDim}
              style={{ position: "absolute", left: 12, top: "50%", transform: "translateY(-50%)" }}
            />
            <input
              value={search}
              onChange={(event) => onSearch(event.target.value)}
              placeholder={placeholder}
              style={{
                width: "100%",
                height: 40,
                borderRadius: 9,
                border: `1px solid ${UI.inputBorder}`,
                background: UI.inputBg,
                color: UI.parchment,
                outline: "none",
                padding: "0 14px 0 40px",
                fontSize: 15,
              }}
            />
          </div>
          <button
            type="button"
            onClick={() => onOpen("assistant")}
            title="Close directory"
            style={{
              width: 34,
              height: 34,
              display: "grid",
              placeItems: "center",
              border: 0,
              background: "transparent",
              color: UI.textMuted,
              cursor: "pointer",
            }}
          >
            <X size={22} />
          </button>
        </div>
        <div className="flex items-center justify-between gap-3" style={{ marginBottom: 20 }}>
          {controls}
        </div>
        <div className="flex-1 overflow-auto pr-1 pb-6" style={{ minHeight: 0 }}>
          {children}
        </div>
      </section>
    </div>
  </div>
);

const DirectoryPill = ({
  active,
  children,
  onClick,
}: {
  active: boolean;
  children: ReactNode;
  onClick: () => void;
}) => (
  <button
    type="button"
    onClick={onClick}
    style={{
      minHeight: 36,
      padding: "0 18px",
      borderRadius: 18,
      border: active ? "1px solid transparent" : `1px solid ${UI.cardBorder}`,
      background: active ? UI.primary : UI.cardBgRaised,
      color: active ? UI.ink : UI.textMuted,
      fontWeight: 800,
      cursor: "pointer",
    }}
  >
    {children}
  </button>
);

const DirectoryGrid = ({ children }: { children: ReactNode }) => (
  <div
    className="grid gap-4"
    style={{ gridTemplateColumns: "repeat(auto-fit, minmax(330px, 1fr))" }}
  >
    {children}
  </div>
);

const DirectoryCard = ({
  icon,
  title,
  source,
  description,
  badges,
  actions,
  muted = false,
}: {
  icon: ReactNode;
  title: string;
  source?: ReactNode;
  description?: ReactNode;
  badges?: ReactNode;
  actions?: ReactNode;
  muted?: boolean;
}) => (
  <div
    style={{
      minHeight: 136,
      borderRadius: 16,
      border: `1px solid ${UI.cardBorder}`,
      background: UI.cardBg,
      boxShadow: "inset 0 1px 0 rgba(255,255,255,0.035), 0 10px 26px rgba(0,0,0,0.22)",
      padding: 18,
      opacity: muted ? 0.62 : 1,
      display: "flex",
      flexDirection: "column",
      gap: 14,
    }}
  >
    <div className="flex items-start gap-3 justify-between">
      <div className="flex items-start gap-3" style={{ minWidth: 0 }}>
        <div
          style={{
            width: 44,
            height: 44,
            borderRadius: 10,
            border: `1px solid ${UI.primaryBorder}`,
            background: UI.primarySoft,
            color: UI.primary,
            display: "grid",
            placeItems: "center",
            flexShrink: 0,
          }}
        >
          {icon}
        </div>
        <div style={{ minWidth: 0 }}>
          <div
            title={title}
            style={{
              color: UI.parchment,
              fontWeight: 800,
              fontSize: 16,
              lineHeight: 1.25,
              overflow: "hidden",
              textOverflow: "ellipsis",
              whiteSpace: "nowrap",
            }}
          >
            {title}
          </div>
          {source && <div style={{ marginTop: 4, color: UI.textMuted, fontSize: 13 }}>{source}</div>}
        </div>
      </div>
      <div className="flex items-center gap-1.5" style={{ flexShrink: 0 }}>
        {actions}
      </div>
    </div>
    {description && (
      <div
        style={{
          color: UI.textMuted,
          fontSize: 14,
          lineHeight: 1.3,
          display: "-webkit-box",
          WebkitLineClamp: 2,
          WebkitBoxOrient: "vertical",
          overflow: "hidden",
        }}
      >
        {description}
      </div>
    )}
    {badges && <div className="flex items-center gap-1.5 flex-wrap mt-auto">{badges}</div>}
  </div>
);

const DirectorySelect = <T extends string>({
  value,
  onChange,
  options,
}: {
  value: T;
  onChange: (value: T) => void;
  options: Array<{ value: T; label: string }>;
}) => (
  <div style={{ width: 148 }}>
    <Select value={value} onChange={onChange} options={options} />
  </div>
);

const DirectoryEmpty = ({ title, sub }: { title: string; sub: string }) => (
  <div
    style={{
      minHeight: 420,
      display: "flex",
      alignItems: "center",
      justifyContent: "center",
      textAlign: "center",
      color: UI.parchment,
    }}
  >
    <div style={{ maxWidth: 560 }}>
      <Boxes size={68} strokeWidth={1.5} color={UI.primary} style={{ margin: "0 auto 26px", opacity: 0.9 }} />
      <div style={{ fontFamily: FRAUNCES, fontSize: 28, fontWeight: 650, marginBottom: 12 }}>
        {title}
      </div>
      <div style={{ color: UI.textMuted, fontSize: 16, lineHeight: 1.45 }}>{sub}</div>
    </div>
  </div>
);

// Signals shown in the top status bar. The LLM signal is provider-neutral
// — it follows whichever credential the operator has marked as default
// (no hardcoded "anthropic" / "openai" entries). The set is rendered
// dynamically by OrdoShell so the LLM signal can be live-derived.
const STATIC_SIGNAL_DEFS: SignalDef[] = [
  { id: "gateway", label: "gateway", state: "ok", detail: "127.0.0.1:4141" },
  { id: "bus", label: "bus", state: "ok", detail: "tokio · standard" },
  { id: "vault", label: "vault", state: "ok", detail: "sealed" },
  { id: "mcp", label: "mcp", state: "warn", detail: "1 server pending" },
  { id: "embed", label: "embed", state: "ok", detail: "deterministic" },
  { id: "heal", label: "heal", state: "ok", detail: "nominal" },
];

// Kept as a const so the rest of the shell (rescue mode trigger,
// initial signal lookup) can find the gateway entry by id without
// touching every reader.
const SIGNAL_DEFS: SignalDef[] = STATIC_SIGNAL_DEFS;

const SIGNAL_COLOR: Record<SignalState, string> = {
  ok: JADE,
  warn: LAMP,
  err: LAMP_HOT,
  off: SLATE,
};

// ─── Primitives ───
const Glass = ({
  children,
  className = "",
  style = {},
}: {
  children: ReactNode;
  className?: string;
  style?: CSSProperties;
}) => (
  <div
    className={`relative rounded-xl ${className}`}
    style={{
      background: "linear-gradient(180deg, rgba(255,255,255,0.025) 0%, rgba(255,255,255,0.01) 100%)",
      backdropFilter: "blur(20px) saturate(140%)",
      WebkitBackdropFilter: "blur(20px) saturate(140%)",
      border: "1px solid rgba(255,255,255,0.06)",
      boxShadow: "inset 0 1px 0 rgba(255,255,255,0.04), 0 20px 40px -20px rgba(0,0,0,0.5)",
      ...style,
    }}
  >
    {children}
  </div>
);

const Mono = ({
  children,
  size = 11,
  color = "rgba(255,255,255,0.55)",
  upper = false,
  track = 0,
  weight = 400,
  style = {},
}: {
  children: ReactNode;
  size?: number;
  color?: string;
  upper?: boolean;
  track?: number | string;
  weight?: number;
  style?: CSSProperties;
}) => (
  <span
    style={{
      fontFamily: MONO,
      fontSize: size,
      color,
      fontWeight: weight,
      textTransform: upper ? "uppercase" : "none",
      letterSpacing: track,
      ...style,
    }}
  >
    {children}
  </span>
);

const Serif = ({
  children,
  size = 14,
  color = PARCHMENT,
  italic = false,
  weight = 400,
  style = {},
}: {
  children: ReactNode;
  size?: number;
  color?: string;
  italic?: boolean;
  weight?: number;
  style?: CSSProperties;
}) => (
  <span
    style={{
      fontFamily: FRAUNCES,
      fontSize: size,
      color,
      fontStyle: italic ? "italic" : "normal",
      fontWeight: weight,
      ...style,
    }}
  >
    {children}
  </span>
);

const Lamp = () => (
  <div className="relative flex items-center gap-3">
    <div className="relative" style={{ width: 36, height: 36 }}>
      <motion.div
        className="absolute inset-0 rounded-full"
        style={{ background: `radial-gradient(circle, ${LAMP} 0%, rgba(244,201,93,0) 70%)` }}
        animate={{ opacity: [0.5, 0.85, 0.5] }}
        transition={{ duration: 3, repeat: Infinity, ease: "easeInOut" }}
      />
      <div
        className="absolute rounded-full"
        style={{
          top: 12,
          left: 12,
          right: 12,
          bottom: 12,
          background: LAMP,
          boxShadow: `0 0 18px ${LAMP}, 0 0 36px rgba(244,201,93,0.3)`,
        }}
      />
    </div>
    <div style={{ lineHeight: 1.05 }}>
      <Mono size={9} color="rgba(255,255,255,0.45)" upper track="0.3em">
        Lucerna Labs
      </Mono>
      <div
        style={{
          fontFamily: FRAUNCES,
          fontSize: 22,
          color: PARCHMENT,
          fontWeight: 500,
          letterSpacing: "-0.02em",
          marginTop: 1,
        }}
      >
        Ordo
      </div>
    </div>
  </div>
);

const Signal = ({ sig }: { sig: SignalDef }) => {
  const color = SIGNAL_COLOR[sig.state];
  return (
    <div className="flex items-center gap-2 group relative cursor-default">
      <motion.div
        style={{
          width: 7,
          height: 7,
          borderRadius: "50%",
          background: color,
          boxShadow: `0 0 8px ${color}, 0 0 16px ${color}40`,
        }}
        animate={
          sig.state === "warn" || sig.state === "err"
            ? { opacity: [1, 0.4, 1] }
            : { opacity: 1 }
        }
        transition={{ duration: 1.5, repeat: Infinity, ease: "easeInOut" }}
      />
      <Mono size={10} upper track="0.15em" color="rgba(255,255,255,0.7)">
        {sig.label}
      </Mono>
      <div
        className="absolute top-full left-0 mt-2 opacity-0 group-hover:opacity-100 pointer-events-none transition-opacity whitespace-nowrap"
        style={{
          background: "rgba(15,18,24,0.95)",
          border: "1px solid rgba(255,255,255,0.08)",
          padding: "4px 8px",
          borderRadius: 4,
          zIndex: 50,
        }}
      >
        <Mono size={10} color="rgba(255,255,255,0.7)">
          {sig.detail}
        </Mono>
      </div>
    </div>
  );
};

const Slider = ({
  value,
  min = 0,
  max = 100,
  onChange,
  unit = "GB",
  color = LAMP,
}: {
  value: number;
  min?: number;
  max?: number;
  onChange: (v: number) => void;
  unit?: string;
  color?: string;
}) => {
  const pct = ((value - min) / (max - min)) * 100;
  return (
    <div className="space-y-1.5">
      <div className="flex justify-between items-baseline">
        <Mono size={10} upper track="0.2em" color="rgba(255,255,255,0.45)">
          budget
        </Mono>
        <span style={{ fontFamily: MONO, fontSize: 13, color }}>
          {value} <span style={{ color: "rgba(255,255,255,0.4)", fontSize: 11 }}>{unit}</span>
        </span>
      </div>
      <div className="relative rounded-full" style={{ height: 4, background: "rgba(255,255,255,0.06)" }}>
        <div
          className="absolute left-0 top-0 h-full rounded-full"
          style={{
            width: `${pct}%`,
            background: `linear-gradient(90deg, ${color}, ${color}cc)`,
            boxShadow: `0 0 8px ${color}80`,
          }}
        />
        <input
          type="range"
          min={min}
          max={max}
          value={value}
          onChange={(e) => onChange(Number(e.target.value))}
          className="absolute inset-0 w-full opacity-0 cursor-pointer"
          style={{ height: 16, top: -6 }}
        />
      </div>
      <div className="flex justify-between">
        <Mono size={9} color="rgba(255,255,255,0.3)">
          {min}
        </Mono>
        <Mono size={9} color="rgba(255,255,255,0.3)">
          {max}
        </Mono>
      </div>
    </div>
  );
};

const SurfaceTitle = ({
  kicker,
  title,
  sub,
}: {
  kicker: string;
  title: string;
  sub?: string;
}) => (
  <div className="mb-6">
    <Mono size={10} upper track="0.3em" color="rgba(255,255,255,0.4)">
      {kicker}
    </Mono>
    <h2
      style={{
        fontFamily: FRAUNCES,
        fontSize: 28,
        color: PARCHMENT,
        fontWeight: 500,
        letterSpacing: "-0.02em",
        marginTop: 4,
        lineHeight: 1.15,
      }}
    >
      {title}
    </h2>
    {sub && (
      <p
        style={{
          fontFamily: FRAUNCES,
          fontSize: 13,
          color: "rgba(255,255,255,0.5)",
          fontStyle: "italic",
          marginTop: 6,
          maxWidth: 580,
          lineHeight: 1.5,
        }}
      >
        {sub}
      </p>
    )}
  </div>
);

// ─── Surfaces ───

// Three-dot "thinking" pulse shown inside an assistant bubble while
// the LLM has accepted the turn but hasn't emitted a token yet. Tells
// the operator the request is in flight, not stalled.
const ThinkingPulse = () => {
  const dot = (delay: number) => (
    <motion.span
      style={{
        display: "inline-block",
        width: 6,
        height: 6,
        borderRadius: 999,
        background: LAMP,
        boxShadow: `0 0 6px ${LAMP}`,
      }}
      animate={{ opacity: [0.25, 1, 0.25], scale: [0.8, 1, 0.8] }}
      transition={{
        duration: 1.1,
        repeat: Infinity,
        ease: "easeInOut",
        delay,
      }}
    />
  );
  return (
    <span
      style={{
        display: "inline-flex",
        alignItems: "center",
        gap: 6,
        paddingTop: 2,
        paddingBottom: 2,
      }}
      aria-label="thinking"
      role="status"
    >
      {dot(0)}
      {dot(0.18)}
      {dot(0.36)}
    </span>
  );
};

interface ChatMessage {
  role: "user" | "assistant";
  text: string;
  ts: string;
  meta?: string[];
  // True while the assistant message is being filled by TokenDelta
  // events from /ws/assistant/<session>. The dock renders a caret /
  // typing affordance and the stream effect appends to .text in place.
  streaming?: boolean;
}

type MidTaskAction = "steer" | "queue" | "interrupt";

interface QueuedAssistantTurn {
  id: string;
  text: string;
  meta: Record<string, unknown>;
}

// Expandable detail panel for the active mode — renders the full
// manifest in compact tabular form, plus a "recent activity" log
// of mode-related events captured off the per-session WebSocket.
// Hidden by default; the operator reveals it via the INSPECT button
// next to the mode picker. This is the spec's "advanced view" —
// what's loaded, what's enabled, what's blocked, what biases are
// in effect, AND what happened in this conversation so far.
const markdownEscapeFence = (text: string): string => text.replace(/```/g, "``\\`");

const filenameSafe = (value: string): string =>
  value
    .trim()
    .toLowerCase()
    .replace(/[^a-z0-9._-]+/g, "-")
    .replace(/^-+|-+$/g, "")
    .slice(0, 80) || "ordo-session";

const renderChatMarkdown = ({
  messages,
  sessionId,
  modeId,
  modeLabel,
}: {
  messages: ChatMessage[];
  sessionId?: string;
  modeId: string;
  modeLabel: string;
}): string => {
  const exportedAt = new Date().toISOString();
  const lines = [
    "# Ordo Conversation Export",
    "",
    `- Exported: ${exportedAt}`,
    `- Session: ${sessionId ?? "(not yet created)"}`,
    `- Mode: ${modeLabel} (${modeId})`,
    `- Messages: ${messages.length}`,
    "",
    "---",
    "",
  ];
  messages.forEach((message, index) => {
    const role = message.role === "user" ? "User" : "Assistant";
    lines.push(`## ${index + 1}. ${role}`);
    lines.push("");
    lines.push(`- Time: ${message.ts}`);
    if (message.meta && message.meta.length > 0) {
      lines.push(`- Meta: ${message.meta.join(", ")}`);
    }
    if (message.streaming) {
      lines.push("- Status: streaming when exported");
    }
    lines.push("");
    lines.push(markdownEscapeFence(message.text.trim() || "(empty)"));
    lines.push("");
  });
  return `${lines.join("\n").trimEnd()}\n`;
};

const downloadTextFile = (filename: string, text: string, mime = "text/markdown;charset=utf-8") => {
  const blob = new Blob([text], { type: mime });
  const url = URL.createObjectURL(blob);
  const link = document.createElement("a");
  link.href = url;
  link.download = filename;
  document.body.appendChild(link);
  link.click();
  link.remove();
  window.setTimeout(() => URL.revokeObjectURL(url), 1000);
};

const copyTextToClipboard = async (text: string) => {
  if (navigator.clipboard?.writeText) {
    await navigator.clipboard.writeText(text);
    return;
  }
  const textarea = document.createElement("textarea");
  textarea.value = text;
  textarea.style.position = "fixed";
  textarea.style.opacity = "0";
  textarea.style.pointerEvents = "none";
  document.body.appendChild(textarea);
  textarea.select();
  document.execCommand("copy");
  textarea.remove();
};

const copyChatMessageToClipboard = async (
  message: ChatMessage,
  index: number,
  surface: "assistant" | "dock",
) => {
  try {
    await copyTextToClipboard(message.text);
    publishUxiDebugEvent("ordo.assistant", "message_copied", "Chat message copied to clipboard.", {
      role: message.role,
      index,
      surface,
      chars: message.text.length,
    });
  } catch (err: unknown) {
    publishUxiDebugEvent("ordo.assistant", "message_copy_failed", "Chat message copy failed.", {
      role: message.role,
      index,
      surface,
      error: err instanceof Error ? err.message : String(err),
    }, "ERROR");
  }
};

const DEFAULT_CONTEXT_WINDOW_TOKENS = 128000;
const TTS_ENABLED_KEY = "ordo:voice_tts_enabled";
const TTS_MODEL_KEY = "ordo:voice_tts_model";
const TTS_VOICE_KEY = "ordo:voice_tts_voice";
const TTS_FORMAT_KEY = "ordo:voice_tts_format";
const DEFAULT_TTS_MODEL = "gpt-4o-mini-tts";
const DEFAULT_TTS_VOICE = "alloy";
const DEFAULT_TTS_FORMAT = "mp3";
const TTS_MODEL_OPTIONS = ["gpt-4o-mini-tts", "tts-1", "tts-1-hd"];
const TTS_VOICE_OPTIONS = [
  "alloy",
  "ash",
  "ballad",
  "coral",
  "echo",
  "fable",
  "onyx",
  "nova",
  "sage",
  "shimmer",
  "verse",
  "marin",
  "cedar",
];

const readStoredBoolean = (key: string, fallback: boolean): boolean => {
  if (typeof window === "undefined") return fallback;
  try {
    const raw = window.localStorage.getItem(key);
    if (raw === "true") return true;
    if (raw === "false") return false;
  } catch {
    // ignore storage denial
  }
  return fallback;
};

const readStoredString = (key: string, fallback: string): string => {
  if (typeof window === "undefined") return fallback;
  try {
    const raw = window.localStorage.getItem(key);
    return raw && raw.trim() ? raw : fallback;
  } catch {
    return fallback;
  }
};

interface ContextBudgetSignal {
  tokens: number;
  providerLabel: string;
  model: string | null;
  configured: boolean;
}

interface ModelChoiceSignal {
  service: string | null;
  providerLabel: string;
  selected: string;
  options: string[];
  extras: Record<string, string>;
  baseUrl?: string | null;
  authStyle?: string | null;
}

type ThinkingEffort = "off" | "medium" | "high";

type WorkspaceScopeKind = "ordo" | "local" | "cloud";
type CloudWorkspaceProvider = "github" | "huggingface";

interface WorkspaceScope {
  kind: WorkspaceScopeKind;
  label: string;
  localPath: string;
  cloudProvider: CloudWorkspaceProvider;
  cloudRef: string;
  sandboxEnabled: boolean;
  allowWrites: boolean;
}

const WORKSPACE_SCOPE_KEY = "ordo:workspace_scope";

const DEFAULT_WORKSPACE_SCOPE: WorkspaceScope = {
  kind: "ordo",
  label: "Ordo internal",
  localPath: "",
  cloudProvider: "github",
  cloudRef: "",
  sandboxEnabled: true,
  allowWrites: false,
};

const normalizeWorkspaceScope = (raw: Partial<WorkspaceScope> | null | undefined): WorkspaceScope => ({
  ...DEFAULT_WORKSPACE_SCOPE,
  ...(raw ?? {}),
  cloudProvider: raw?.cloudProvider === "huggingface" ? "huggingface" : "github",
  kind: raw?.kind === "local" || raw?.kind === "cloud" ? raw.kind : "ordo",
});

const loadWorkspaceScope = (): WorkspaceScope => {
  if (typeof window === "undefined") return DEFAULT_WORKSPACE_SCOPE;
  try {
    const raw = window.localStorage.getItem(WORKSPACE_SCOPE_KEY);
    return normalizeWorkspaceScope(raw ? (JSON.parse(raw) as Partial<WorkspaceScope>) : null);
  } catch {
    return DEFAULT_WORKSPACE_SCOPE;
  }
};

const saveWorkspaceScope = (scope: WorkspaceScope) => {
  if (typeof window === "undefined") return;
  window.localStorage.setItem(WORKSPACE_SCOPE_KEY, JSON.stringify(scope));
};

const workspaceScopeLabel = (scope: WorkspaceScope): string => {
  if (scope.kind === "local") {
    return scope.label || scope.localPath.split(/[\\/]/).filter(Boolean).pop() || "Local project";
  }
  if (scope.kind === "cloud") {
    const provider = scope.cloudProvider === "huggingface" ? "Hugging Face" : "GitHub";
    return scope.cloudRef ? `${provider}: ${scope.cloudRef}` : `${provider} workspace`;
  }
  return "Ordo internal";
};

const workspaceScopeToMetadata = (scope: WorkspaceScope) => ({
  kind: scope.kind,
  label: workspaceScopeLabel(scope),
  local_path: scope.kind === "local" ? scope.localPath : null,
  cloud_provider: scope.kind === "cloud" ? scope.cloudProvider : null,
  cloud_ref: scope.kind === "cloud" ? scope.cloudRef : null,
  sandbox: {
    enabled: scope.kind !== "ordo" && scope.sandboxEnabled,
    root: scope.kind === "local" ? scope.localPath : null,
    allow_writes: scope.kind !== "ordo" && scope.allowWrites,
    deny_parent_traversal: true,
    deny_outside_root: true,
  },
  retrieval: {
    source: scope.kind === "ordo" ? "ordo_internal" : "selected_workspace",
    disable_internal_rag_by_default: scope.kind !== "ordo",
  },
});

const readStoredThinkingEffort = (): ThinkingEffort => {
  if (typeof window === "undefined") return "off";
  const stored = window.localStorage.getItem("ordo:thinking_effort");
  return stored === "medium" || stored === "high" || stored === "off"
    ? stored
    : "off";
};

const parseContextWindowTokens = (value: unknown): number | null => {
  if (typeof value === "number" && Number.isFinite(value) && value > 0) {
    return Math.floor(value);
  }
  if (typeof value !== "string") return null;
  const normalized = value.replace(/[, _]/g, "").trim();
  if (!normalized) return null;
  const parsed = Number.parseInt(normalized, 10);
  return Number.isFinite(parsed) && parsed > 0 ? parsed : null;
};

const estimateTextTokens = (text: string): number => {
  if (!text.trim()) return 0;
  return Math.ceil(text.length / 4);
};

const estimateChatTokens = (messages: ChatMessage[], draftInput: string): number => {
  const messageTokens = messages.reduce((total, message) => {
    const metadataTokens = message.meta ? estimateTextTokens(message.meta.join(" ")) : 0;
    return total + 8 + estimateTextTokens(message.text) + metadataTokens;
  }, 0);
  return messageTokens + estimateTextTokens(draftInput);
};

const formatTokenCount = (tokens: number): string => {
  if (tokens >= 1_000_000) {
    return `${(tokens / 1_000_000).toFixed(tokens >= 10_000_000 ? 0 : 1)}m`;
  }
  if (tokens >= 1_000) {
    return `${(tokens / 1_000).toFixed(tokens >= 100_000 ? 0 : 1)}k`;
  }
  return String(tokens);
};

const contextUsageTone = (percent: number): { color: string; label: string } => {
  if (percent >= 90) return { color: RED, label: "critical" };
  if (percent >= 70) return { color: LAMP_HOT, label: "high" };
  return { color: JADE, label: "healthy" };
};

const ContextUsageIndicator = ({
  usedTokens,
  budget,
  modelChoice,
  modelSaving,
  onModelChange,
  thinkingEffort,
  onThinkingEffortChange,
}: {
  usedTokens: number;
  budget: ContextBudgetSignal;
  modelChoice: ModelChoiceSignal;
  modelSaving: boolean;
  onModelChange: (model: string) => void;
  thinkingEffort: ThinkingEffort;
  onThinkingEffortChange: (effort: ThinkingEffort) => void;
}) => {
  const percent = budget.tokens > 0 ? Math.min(100, Math.round((usedTokens / budget.tokens) * 100)) : 0;
  const tone = contextUsageTone(percent);
  const title = [
    `Context estimate: ~${formatTokenCount(usedTokens)} / ${formatTokenCount(budget.tokens)} tokens (${percent}%)`,
    budget.configured
      ? `Provider: ${budget.providerLabel}${budget.model ? `, model: ${budget.model}` : ""}`
      : "Provider context window unknown, using 128k fallback",
    "Compactor: auto mechanical prompt compaction is built in",
  ].join("\n");
  return (
    <div
      className="flex items-center gap-2"
      title={title}
      aria-label={`Context used ${percent} percent`}
      style={{
        width: "100%",
        minWidth: 0,
      }}
    >
      <div
        className="flex items-center gap-2"
        style={{
          flexShrink: 0,
          minWidth: 220,
          padding: "5px 8px",
          borderRadius: 999,
          border: `1px solid ${tone.color}33`,
          background: `${tone.color}0f`,
        }}
      >
        <Mono size={9} upper track="0.14em" color={tone.color}>
          CTX {percent}%
        </Mono>
        <div
          style={{
            flex: 1,
            minWidth: 80,
            height: 4,
            borderRadius: 999,
            background: "rgba(255,255,255,0.12)",
            overflow: "hidden",
          }}
        >
          <div
            style={{
              width: `${percent}%`,
              height: "100%",
              borderRadius: 999,
              background: tone.color,
              boxShadow: `0 0 8px ${tone.color}66`,
            }}
          />
        </div>
        <Mono size={8} upper track="0.12em" color="rgba(255,255,255,0.38)">
          {formatTokenCount(usedTokens)} / {formatTokenCount(budget.tokens)}
        </Mono>
      </div>
      <label
        className="flex items-center gap-2"
        title={
          modelChoice.service
            ? `Active model for ${modelChoice.providerLabel}`
            : "Configure a provider before choosing a model"
        }
        style={{
          flexShrink: 1,
          minWidth: 180,
          maxWidth: 360,
          padding: "5px 8px",
          borderRadius: 999,
          border: "1px solid rgba(255,255,255,0.10)",
          background: "rgba(255,255,255,0.035)",
        }}
      >
        <Mono size={8} upper track="0.12em" color="rgba(255,255,255,0.42)">
          model
        </Mono>
        <select
          value={modelChoice.selected}
          disabled={!modelChoice.service || modelSaving || modelChoice.options.length === 0}
          onChange={(event) => onModelChange(event.currentTarget.value)}
          onClick={(event) => event.stopPropagation()}
          style={{
            minWidth: 0,
            flex: 1,
            appearance: "none",
            background: "transparent",
            border: "none",
            color: PARCHMENT,
            fontFamily: MONO,
            fontSize: 10,
            fontWeight: 700,
            outline: "none",
            cursor:
              !modelChoice.service || modelSaving || modelChoice.options.length === 0
                ? "not-allowed"
                : "pointer",
            opacity:
              !modelChoice.service || modelChoice.options.length === 0 ? 0.55 : 1,
          }}
        >
          {modelChoice.options.length > 0 ? (
            modelChoice.options.map((model) => (
              <option key={model} value={model}>
                {model}
              </option>
            ))
          ) : (
            <option value="">no model</option>
          )}
        </select>
      </label>
      <label
        className="flex items-center gap-2"
        title="Thinking effort for future assistant turns"
        style={{
          flexShrink: 0,
          padding: "5px 8px",
          borderRadius: 999,
          border: "1px solid rgba(244,201,93,0.24)",
          background: "rgba(255,255,255,0.035)",
        }}
      >
        <Mono size={8} upper track="0.12em" color="rgba(255,255,255,0.42)">
          thinking
        </Mono>
        <select
          value={thinkingEffort}
          onChange={(event) =>
            onThinkingEffortChange(event.currentTarget.value as ThinkingEffort)
          }
          onClick={(event) => event.stopPropagation()}
          style={{
            appearance: "none",
            background: "transparent",
            border: "none",
            color: PARCHMENT,
            fontFamily: MONO,
            fontSize: 10,
            fontWeight: 700,
            outline: "none",
            cursor: "pointer",
            paddingRight: 2,
          }}
        >
          <option value="off">off</option>
          <option value="medium">medium</option>
          <option value="high">high</option>
        </select>
      </label>
      <div style={{ flex: 1, minWidth: 12 }} />
      <Mono size={8} upper track="0.12em" color="rgba(255,255,255,0.38)" style={{ flexShrink: 0 }}>
        auto compact
      </Mono>
    </div>
  );
};

const ModeInspectorPanel = ({
  manifest,
  events,
}: {
  manifest: AssistantMode;
  events: ModeEventLogEntry[];
}) => {
  const list = (value: unknown): string[] => (Array.isArray(value) ? value : []);
  // Each section is an array of strings rendered as a labeled list.
  // Empty sections are dropped so the panel stays terse.
  const sections: { label: string; items: string[]; emptyHint?: string }[] = [
    { label: "memory scopes", items: list(manifest.memory_scope) },
    { label: "RAG domains", items: list(manifest.rag_domains), emptyHint: "(no RAG access)" },
    { label: "allowed tool lanes", items: list(manifest.allowed_tool_lanes), emptyHint: "(no tools)" },
    { label: "blocked tools", items: list(manifest.blocked_tool_capabilities) },
    { label: "policies", items: list(manifest.policies) },
    { label: "planner bias", items: list(manifest.planner_bias) },
    { label: "persona", items: list(manifest.persona) },
  ];
  const overrideRows: { label: string; value: string }[] = [];
  if (manifest.default_timeout_secs != null) {
    overrideRows.push({
      label: "timeout override",
      value: `${manifest.default_timeout_secs}s`,
    });
  }
  if (manifest.default_strictness) {
    overrideRows.push({
      label: "strictness override",
      value: manifest.default_strictness,
    });
  }
  if (manifest.default_credential) {
    overrideRows.push({
      label: "credential override",
      value: manifest.default_credential,
    });
  }
  return (
    <div
      className="rounded-md px-3 py-3 mb-2"
      style={{
        background: "rgba(255,255,255,0.015)",
        border: "1px solid rgba(255,255,255,0.06)",
      }}
    >
      <div className="grid grid-cols-2 gap-x-6 gap-y-2">
        {sections.map((s) => (
          <div key={s.label}>
            <Mono size={9} upper track="0.25em" color={UI.textMuted}>
              {s.label}
            </Mono>
            {s.items.length > 0 ? (
              <div className="mt-1 flex flex-wrap gap-1">
                {s.items.map((it) => (
                  <span
                    key={it}
                    style={{
                      fontFamily: MONO,
                      fontSize: 10,
                      padding: "2px 8px",
                      borderRadius: 3,
                      background: "rgba(255,255,255,0.04)",
                      color: "rgba(255,255,255,0.65)",
                      border: "1px solid rgba(255,255,255,0.06)",
                    }}
                  >
                    {it}
                  </span>
                ))}
              </div>
            ) : (
              <Serif size={11} italic color={UI.textMuted}>
                {s.emptyHint ?? "(none)"}
              </Serif>
            )}
          </div>
        ))}
      </div>
      {overrideRows.length > 0 && (
        <div className="mt-3 pt-3" style={{ borderTop: "1px solid rgba(255,255,255,0.06)" }}>
          <Mono size={9} upper track="0.25em" color={UI.textMuted}>
            mode-specific overrides
          </Mono>
          <div className="mt-1 flex flex-wrap gap-3">
            {overrideRows.map((r) => (
              <div key={r.label}>
                <Mono size={9} color={UI.textMuted}>
                  {r.label}
                </Mono>
                <span style={{ marginLeft: 6, fontFamily: MONO, fontSize: 11 }}>
                  {r.value}
                </span>
              </div>
            ))}
          </div>
        </div>
      )}
      {events.length > 0 && (
        <div className="mt-3 pt-3" style={{ borderTop: "1px solid rgba(255,255,255,0.06)" }}>
          <Mono size={9} upper track="0.25em" color={UI.textMuted}>
            recent activity (mode events)
          </Mono>
          <div className="mt-1 max-h-64 overflow-auto pr-1">
            {events.map((entry, i) => (
              <ModeEventRow key={i} entry={entry} dividerAbove={i > 0} />
            ))}
          </div>
        </div>
      )}
    </div>
  );
};

// One row in the inspector's event log. Click anywhere on the
// summary line to toggle the raw-JSON disclosure; the underlying
// event payload (TurnEvent) renders pretty-printed for the
// operator who needs to verify the exact field values that the
// summary condensed.
const ModeEventRow = ({
  entry,
  dividerAbove,
}: {
  entry: ModeEventLogEntry;
  dividerAbove: boolean;
}) => {
  const [expanded, setExpanded] = useState(false);
  return (
    <div
      style={{
        borderTop: dividerAbove ? "1px dotted rgba(255,255,255,0.04)" : "none",
      }}
    >
      <div
        className="flex items-baseline gap-2 py-0.5"
        onClick={() => setExpanded((v) => !v)}
        style={{ cursor: "pointer" }}
        title={expanded ? "click to collapse" : "click to show raw JSON"}
      >
        <span style={{ fontFamily: MONO, fontSize: 9, color: UI.textMuted, minWidth: 36 }}>
          {entry.ts}
        </span>
        <span
          style={{
            fontFamily: MONO,
            fontSize: 9,
            padding: "1px 6px",
            borderRadius: 2,
            background:
              entry.kind === "cross_mode_consult_denied"
                ? `${LAMP_HOT}1f`
                : entry.kind.startsWith("cross_mode")
                ? `${LAMP}14`
                : "rgba(255,255,255,0.04)",
            color:
              entry.kind === "cross_mode_consult_denied"
                ? LAMP_HOT
                : "rgba(255,255,255,0.55)",
            border: "1px solid rgba(255,255,255,0.06)",
            minWidth: 80,
            textAlign: "center",
          }}
        >
          {entry.kind.replace(/_/g, " ")}
        </span>
        <span style={{ fontFamily: MONO, fontSize: 10, color: "rgba(255,255,255,0.7)", flex: 1 }}>
          {entry.summary}
        </span>
        <span
          style={{
            fontFamily: MONO,
            fontSize: 9,
            color: UI.textMuted,
            paddingLeft: 4,
          }}
          aria-hidden
        >
          {expanded ? "▼" : "▸"}
        </span>
      </div>
      {expanded && (
        <pre
          style={{
            fontFamily: MONO,
            fontSize: 10,
            color: "rgba(255,255,255,0.55)",
            background: "rgba(0,0,0,0.25)",
            border: "1px solid rgba(255,255,255,0.04)",
            borderRadius: 3,
            padding: "6px 8px",
            margin: "4px 0 6px 38px",
            overflowX: "auto",
            whiteSpace: "pre-wrap",
            wordBreak: "break-word",
          }}
        >
          {JSON.stringify(entry.raw, null, 2)}
        </pre>
      )}
    </div>
  );
};

const AssistantSurface = ({
  messages,
  scrollRef,
  endRef,
  onTranscriptScroll,
  taint,
  onClearTaint,
  newChatBusy,
  sessions,
  activeSessionId,
  sessionsBusy,
  onSessionChange,
  modes,
  activeMode,
  onModeChange,
  collaboratorRequest,
  onCollaboratorRequestChange,
  workspaceScope,
  onOpenWorkspace,
  inspectorOpen,
  onToggleInspector,
  modeEvents,
}: {
  messages: ChatMessage[];
  scrollRef: RefObject<HTMLDivElement | null>;
  endRef: RefObject<HTMLDivElement | null>;
  onTranscriptScroll: () => void;
  taint: SessionTaintState | null;
  onClearTaint: () => void;
  newChatBusy: boolean;
  sessions: AssistantSessionRecord[];
  activeSessionId?: string;
  sessionsBusy: boolean;
  onSessionChange: (sessionId: string) => void;
  modes: AssistantMode[];
  activeMode: string;
  onModeChange: (next: string) => void;
  collaboratorRequest: string;
  onCollaboratorRequestChange: (next: string) => void;
  workspaceScope: WorkspaceScope;
  onOpenWorkspace: () => void;
  inspectorOpen: boolean;
  onToggleInspector: () => void;
  modeEvents: ModeEventLogEntry[];
}) => {
  // Render-time fallback list. When the runtime hasn't returned its
  // registered modes yet (or has none — config error), the picker
  // still shows General so the operator isn't staring at an empty
  // dropdown. Backend's mode-resolution treats unknown ids as
  // General anyway, so this stays consistent.
  const pickerOptions: AssistantMode[] =
    modes.length > 0
      ? modes
      : [
          {
            id: "general",
            label: "General Assistant",
            description: "Loading workspaces…",
            memory_scope: ["global"],
            rag_domains: [],
            allowed_tool_lanes: [],
            blocked_tool_capabilities: [],
            policies: [],
            planner_bias: [],
            persona: [],
          },
        ];
  const activeManifest =
    pickerOptions.find((m) => m.id === activeMode) ?? pickerOptions[0];
  const collaboratorOptions = pickerOptions.filter((mode) => mode.id !== activeManifest.id);

  return (
    <div className="h-full min-h-0 flex flex-col" style={{ marginTop: -8 }}>
      <Mono
        size={10}
        upper
        track="0.3em"
        color="rgba(255,255,255,0.34)"
        style={{ marginBottom: 8, display: "block" }}
      >
        ordo planner
      </Mono>
    {/* Mode picker — operator-visible workspace switcher. Each
        mode is a bounded operating environment (own memory scope,
        own tool surface, own persona). Switching minted a NEW
        session in the chosen mode; the previous chat stays in the
        runtime's DB but the studio takes the foreground to the
        fresh one. The architectural rule (mode is fixed per
        session) makes this an unambiguous boundary, not a sneaky
        in-place rewrite. */}
    <div
      className="flex items-center gap-3 rounded-md px-3 py-2 mb-2"
      style={{
        background: "rgba(255,255,255,0.025)",
        border: "1px solid rgba(255,255,255,0.06)",
        flexShrink: 0,
      }}
    >
      <Mono size={9} upper track="0.25em" color={UI.textMuted}>
        session
      </Mono>
      <select
        value={activeSessionId ?? ""}
        onChange={(e) => onSessionChange(e.target.value)}
        disabled={newChatBusy || sessionsBusy}
        title="Switch between new and older chat sessions"
        style={{
          fontFamily: MONO,
          fontSize: 11,
          background: "rgba(255,255,255,0.04)",
          color: "rgba(255,255,255,0.9)",
          border: "1px solid rgba(255,255,255,0.1)",
          borderRadius: 4,
          padding: "4px 8px",
          cursor: newChatBusy || sessionsBusy ? "not-allowed" : "pointer",
          minWidth: 230,
          maxWidth: 300,
        }}
      >
        <option value="__new__">{newChatBusy ? "Starting new session..." : "New session..."}</option>
        {activeSessionId && !sessions.some((session) => session.id === activeSessionId) && (
          <option value={activeSessionId}>Current session - {activeSessionId.slice(0, 8)}</option>
        )}
        {sessions.map((session) => (
          <option key={session.id} value={session.id}>
            {sessionOptionLabel(session, pickerOptions)}
          </option>
        ))}
      </select>
      <Mono size={9} upper track="0.25em" color={UI.textMuted}>
        mode
      </Mono>
      <select
        value={activeMode}
        onChange={(e) => onModeChange(e.target.value)}
        style={{
          fontFamily: MONO,
          fontSize: 11,
          background: "rgba(255,255,255,0.04)",
          color: "rgba(255,255,255,0.9)",
          border: "1px solid rgba(255,255,255,0.1)",
          borderRadius: 4,
          padding: "4px 8px",
          cursor: "pointer",
          minWidth: 160,
        }}
      >
        {pickerOptions.map((m) => (
          <option key={m.id} value={m.id}>
            {m.label}
          </option>
        ))}
      </select>
      <Mono size={9} upper track="0.18em" color={UI.textMuted}>
        workspace
      </Mono>
      <button
        type="button"
        onClick={onOpenWorkspace}
        title="Select local or cloud workspace"
        style={{
          fontFamily: MONO,
          fontSize: 11,
          background: workspaceScope.kind === "ordo" ? "rgba(255,255,255,0.04)" : `${LAMP}14`,
          color: workspaceScope.kind === "ordo" ? "rgba(255,255,255,0.78)" : LAMP,
          border: `1px solid ${workspaceScope.kind === "ordo" ? "rgba(255,255,255,0.1)" : `${LAMP}44`}`,
          borderRadius: 4,
          padding: "4px 8px",
          cursor: "pointer",
          maxWidth: 240,
          overflow: "hidden",
          textOverflow: "ellipsis",
          whiteSpace: "nowrap",
        }}
      >
        {workspaceScopeLabel(workspaceScope)}
      </button>
      {collaboratorOptions.length > 0 && (
        <>
          <Mono size={9} upper track="0.18em" color={UI.textMuted}>
            consult
          </Mono>
          <select
            value={collaboratorRequest}
            onChange={(e) => onCollaboratorRequestChange(e.target.value)}
            title="Request another mode for the next turn"
            style={{
              fontFamily: MONO,
              fontSize: 11,
              background: "rgba(255,255,255,0.04)",
              color: "rgba(255,255,255,0.9)",
              border: "1px solid rgba(255,255,255,0.1)",
              borderRadius: 4,
              padding: "4px 8px",
              cursor: "pointer",
              minWidth: 190,
            }}
          >
            <option value="">No requested collaborator</option>
            {collaboratorOptions.map((m) => (
              <option key={m.id} value={m.id}>
                {m.label}
              </option>
            ))}
          </select>
          {collaboratorRequest && (
            <button
              type="button"
              onClick={() => onCollaboratorRequestChange("")}
              title="Clear collaborator request"
              style={{
                background: "transparent",
                border: "none",
                color: UI.textMuted,
                cursor: "pointer",
                display: "inline-flex",
                alignItems: "center",
                padding: 0,
              }}
            >
              <X size={13} />
            </button>
          )}
        </>
      )}
      <div
        style={{ flex: 1, overflow: "hidden", textOverflow: "ellipsis", whiteSpace: "nowrap" }}
        title={activeManifest.description}
      >
        <Serif size={11} italic color={UI.textMuted}>
          {activeManifest.description}
        </Serif>
      </div>
      <button
        onClick={() => onToggleInspector()}
        style={{
          fontFamily: MONO,
          fontSize: 10,
          padding: "4px 10px",
          borderRadius: 4,
          background: "transparent",
          color: "rgba(255,255,255,0.6)",
          border: "1px solid rgba(255,255,255,0.12)",
          cursor: "pointer",
        }}
        aria-label={inspectorOpen ? "hide mode details" : "show mode details"}
      >
        {inspectorOpen ? "HIDE" : "INSPECT"}
      </button>
    </div>
    {inspectorOpen && (
      <ModeInspectorPanel manifest={activeManifest} events={modeEvents} />
    )}
    {/* Taint indicator — only shows when the conversation has
        ingested untrusted web content. Tooltip carries the source
        URLs; the inline button clears taint so sensitive actions
        re-enable. Same lamp-hot color as the rescue pulse so the
        operator parses "this conversation has been compromised by
        external content" at a glance. */}
    {taint?.tainted && (
      <div
        className="flex items-center gap-2 rounded-md px-3 py-2 mb-2"
        style={{
          background: `${LAMP_HOT}14`,
          border: `1px solid ${LAMP_HOT}55`,
        }}
        title={
          taint.sources
            .filter(
              (s): s is Extract<TaintSource, { kind: "untrusted_web" }> =>
                s.kind === "untrusted_web",
            )
            .map((s) => `${s.source_url} (${s.fetched_at})`)
            .join("\n") || "untrusted content in this conversation"
        }
      >
        <Mono size={9} upper track="0.25em" color={LAMP_HOT}>
          tainted conversation
        </Mono>
        <Serif size={11} italic color={UI.textMuted} style={{ flex: 1 }}>
          web-fetched content has entered context. Sensitive actions
          (writes, dispatches, memory pins) are gated until cleared.
        </Serif>
        <button
          onClick={onClearTaint}
          style={{
            fontFamily: MONO,
            fontSize: 10,
            padding: "4px 10px",
            borderRadius: 4,
            background: "transparent",
            color: LAMP_HOT,
            border: `1px solid ${LAMP_HOT}88`,
            cursor: "pointer",
          }}
        >
          CLEAR TAINT
        </button>
      </div>
    )}
    <div
      ref={scrollRef as RefObject<HTMLDivElement>}
      onScroll={onTranscriptScroll}
      className="flex-1 overflow-auto pr-2 space-y-4"
      style={{ minHeight: 0 }}
    >
      {messages.map((m, i) => (
        <div key={i} className={`flex ${m.role === "user" ? "justify-end" : "justify-start"}`}>
          <div
            className="rounded-2xl px-5 py-4"
            style={{
              maxWidth: "78%",
              minWidth: 0,
              overflow: "hidden",
              background:
                m.role === "user"
                  ? `linear-gradient(180deg, ${LAMP}1f, ${LAMP}08)`
                  : "linear-gradient(180deg, rgba(255,255,255,0.04), rgba(255,255,255,0.015))",
              border: m.role === "user" ? `1px solid ${LAMP}33` : "1px solid rgba(255,255,255,0.07)",
            }}
          >
            <Mono
              size={9}
              upper
              track="0.25em"
              color={m.role === "user" ? `${LAMP}b3` : "rgba(255,255,255,0.4)"}
            >
              {m.role === "user" ? "operator" : "assistant"} · {m.ts}
            </Mono>
            <button
              type="button"
              onClick={() => void copyChatMessageToClipboard(m, i, "assistant")}
              disabled={!m.text.trim()}
              title={m.text.trim() ? "Copy message" : "Message is still empty"}
              aria-label="copy message"
              style={{
                marginTop: 8,
                display: "inline-flex",
                alignItems: "center",
                gap: 5,
                border: "1px solid rgba(255,255,255,0.08)",
                borderRadius: 4,
                padding: "3px 7px",
                background: "rgba(255,255,255,0.03)",
                color: "rgba(255,255,255,0.5)",
                cursor: m.text.trim() ? "pointer" : "not-allowed",
                opacity: m.text.trim() ? 1 : 0.42,
              }}
            >
              <Copy size={12} />
              <Mono size={9} upper track="0.12em" color="currentColor">copy</Mono>
            </button>
            <div style={{ marginTop: 8 }}>
              {m.streaming && !m.text.trim() ? (
                <ThinkingPulse />
              ) : (
                <Serif
                  size={15}
                  style={{
                    lineHeight: 1.55,
                    display: "block",
                    maxWidth: "100%",
                    whiteSpace: "pre-wrap",
                    overflowWrap: "anywhere",
                    wordBreak: "break-word",
                  }}
                >
                  {m.text}
                </Serif>
              )}
            </div>
            {m.meta && (
              <div className="mt-3 flex flex-wrap gap-1.5">
                {m.meta.map((tag, j) => (
                  <span
                    key={j}
                    style={{
                      fontFamily: MONO,
                      fontSize: 10,
                      padding: "2px 8px",
                      borderRadius: 3,
                      background: "rgba(255,255,255,0.04)",
                      color: "rgba(255,255,255,0.55)",
                      border: "1px solid rgba(255,255,255,0.06)",
                    }}
                  >
                    {tag}
                  </span>
                ))}
              </div>
            )}
          </div>
        </div>
      ))}
      <div ref={endRef as RefObject<HTMLDivElement>} aria-hidden="true" />
    </div>
  </div>
  );
};

// ─── shared helpers used across multiple surfaces ───────────────

const RAG_LANE_TINT: Record<string, string> = {
  main: LAMP,
  knowledge: PEACH,
  orchestration: JADE,
  research: VIOLET,
};

const LANE_TINTS: Record<string, string> = {
  knowledge: PEACH,
  orchestration: JADE,
  research: VIOLET,
  cloud: LAMP,
  self_heal: LAMP_HOT,
};

const tintForLane = (laneName: string): string => LANE_TINTS[laneName] ?? SLATE;

// Bytes ↔ GB for slider UIs; runtime persists bytes natively.
const BYTES_PER_GB = 1024 * 1024 * 1024;
const bytesToGb = (b: number): number => Math.round((b / BYTES_PER_GB) * 100) / 100;
const gbToBytes = (g: number): number => Math.round(g * BYTES_PER_GB);

// Coerce the runtime's memory.list_* result into a flat list of strings.
function flattenMemoryNotes(payload: unknown): string[] {
  if (Array.isArray(payload)) return payload.map(stringifyNote);
  if (typeof payload === "object" && payload !== null) {
    const obj = payload as Record<string, unknown>;
    for (const key of ["pinned", "working", "notes", "results", "items"]) {
      const v = obj[key];
      if (Array.isArray(v)) return v.map(stringifyNote);
    }
  }
  return [];
}

function stringifyNote(x: unknown): string {
  if (typeof x === "string") return x;
  if (typeof x === "object" && x !== null) {
    const obj = x as Record<string, unknown>;
    return (obj.content as string | undefined) ?? (obj.note as string | undefined) ?? JSON.stringify(x);
  }
  return String(x);
}

interface SelfHealCaseRow {
  id?: string;
  fingerprint?: string;
  symptom?: string;
  classified?: string | null;
  classification?: string | null;
  fix?: string | null;
  actions?: string[];
  pinned?: boolean;
  replays?: number;
  replay_count?: number;
  last_seen_at?: string;
  [k: string]: unknown;
}

function flattenCases(payload: unknown): SelfHealCaseRow[] {
  if (Array.isArray(payload)) return payload as SelfHealCaseRow[];
  if (typeof payload === "object" && payload !== null) {
    const obj = payload as Record<string, unknown>;
    for (const key of ["cases", "results", "items"]) {
      const v = obj[key];
      if (Array.isArray(v)) return v as SelfHealCaseRow[];
    }
  }
  return [];
}

const PROFILES: { id: "minimal" | "standard" | "full"; note: string }[] = [
  { id: "minimal", note: "core only · no rag" },
  { id: "standard", note: "rag lazy · default" },
  { id: "full", note: "everything eager" },
];

// Tiny stat-line used by the Runtime surface's effective-state grid.
const RuntimeStat = ({ label, value }: { label: string; value: string }) => (
  <div className="flex items-baseline gap-3">
    <Mono size={9} upper track="0.2em" color="rgba(255,255,255,0.4)" style={{ minWidth: 110 }}>
      {label}
    </Mono>
    <Mono size={11} color={PARCHMENT}>
      {value}
    </Mono>
  </div>
);

// ─── Skills ─────────────────────────────────────────────────────
//
// Operator-facing skill catalog. Three sources flow into one view:
//
//   1. Built-in capabilities from /api/capabilities (filtered to
//      domain + interface lanes — the ones the operator drives).
//      These can't be uninstalled (they're crate code) but CAN be
//      paused — a paused skill is sent in TurnRequest.metadata so
//      the runtime can honor it as skill-toggling support lands.
//
//   2. Custom skills the operator adds locally via the Install
//      button. These capture intent / wishlist entries — names and
//      descriptions of skills the operator wants. They don't yet
//      bind to runtime tools (use the MCP tab to install actual
//      MCP-server-provided tools), but they ride along in turn
//      metadata as a hint for the planner.
//
// State for paused + custom is persisted to localStorage so the
// operator's catalog stays put across reloads. The Capabilities tab
// is the raw, ungrouped inventory; this surface is the read+manage
// face on top of it.

const SKILLS_PAUSED_KEY = "ordo:disabled_skills";
const SKILLS_CUSTOM_KEY = "ordo:custom_skills";

// ─── Response timeout preset ────────────────────────────────────
//
// Operator-facing preset for how long the runtime should wait on an
// LLM call before giving up. The runtime already honors a
// per-credential override via `extras.timeout_secs`; this preset is
// what the studio writes into that field whenever it creates or
// updates a credential, so the operator can pick once (in the
// Runtime tab) and have it apply everywhere without editing each
// provider individually.
//
// 5 minutes is the default — generous enough for local reasoning
// models on consumer hardware, tight enough that a hung connection
// surfaces as an error in a reasonable window. The 30 / 60 min
// presets at the long end exist primarily for the Vibe Coding and
// Research modes, where deep-context reasoning, multi-step tool
// loops, and slow-host inference can legitimately run past 10 min.
// Those modes will set 30 min as their default via the mode manifest;
// this list just makes the option available globally.
const TIMEOUT_PRESET_KEY = "ordo:llm_timeout_secs";
const DEFAULT_TIMEOUT_SECS = 300;
const TIMEOUT_PRESETS: { secs: number; label: string; sub: string }[] = [
  { secs: 180, label: "3 min", sub: "fast cloud models, fail-fast" },
  { secs: 300, label: "5 min", sub: "balanced — recommended default" },
  { secs: 600, label: "10 min", sub: "long-context reasoning, slow hosts" },
  { secs: 1800, label: "30 min", sub: "coding / research deep loops (default for those modes)" },
  { secs: 3600, label: "60 min", sub: "very long agentic runs; use sparingly" },
];

const loadTimeoutPreset = (): number => {
  if (typeof window === "undefined") return DEFAULT_TIMEOUT_SECS;
  const raw = window.localStorage.getItem(TIMEOUT_PRESET_KEY);
  if (!raw) return DEFAULT_TIMEOUT_SECS;
  const parsed = Number.parseInt(raw, 10);
  return Number.isFinite(parsed) && parsed > 0 ? parsed : DEFAULT_TIMEOUT_SECS;
};
const saveTimeoutPreset = (secs: number): void => {
  if (typeof window === "undefined") return;
  window.localStorage.setItem(TIMEOUT_PRESET_KEY, String(secs));
};

// ─── Untrusted-content strictness preset ─────────────────────────
//
// Operator-facing preset for how strictly the assistant enforces
// the boundary rule on `<untrusted_web_content>` blocks (output of
// the Strainer). The runtime reads this from
// `TurnRequest.metadata.untrusted_strictness` per turn and assembles
// the bootstrap system prompt accordingly. Default is "medium" —
// the doc's recommended baseline.
//
//   off    → no rule appended (DEBUG ONLY; do not ship). Useful for
//            verifying strainer output reaches the assistant intact.
//   low    → soft hint, model prefers treating untrusted as data.
//   medium → strict treatment, decline if asked to follow embedded
//            directives (default).
//   high   → strict + announce; model must call out embedded
//            instructions before answering.
const STRICTNESS_KEY = "ordo:untrusted_strictness";
type StrictnessPreset = "off" | "low" | "medium" | "high";
const DEFAULT_STRICTNESS: StrictnessPreset = "medium";
const STRICTNESS_PRESETS: {
  id: StrictnessPreset;
  label: string;
  sub: string;
  warn?: boolean;
}[] = [
  {
    id: "off",
    label: "Off",
    sub: "DEBUG — no rule. Strainer output passes through uninterpreted.",
    warn: true,
  },
  {
    id: "low",
    label: "Low",
    sub: "Soft hint. Treat untrusted as data; gentle preference.",
  },
  {
    id: "medium",
    label: "Medium",
    sub: "Recommended. Strict + decline if asked to follow embedded directives.",
  },
  {
    id: "high",
    label: "High",
    sub: "Strict + announce. Model calls out embedded instructions visibly.",
  },
];

const loadStrictnessPreset = (): StrictnessPreset => {
  if (typeof window === "undefined") return DEFAULT_STRICTNESS;
  const raw = window.localStorage.getItem(STRICTNESS_KEY);
  if (!raw) return DEFAULT_STRICTNESS;
  if (raw === "off" || raw === "low" || raw === "medium" || raw === "high") {
    return raw;
  }
  return DEFAULT_STRICTNESS;
};
const saveStrictnessPreset = (id: StrictnessPreset): void => {
  if (typeof window === "undefined") return;
  window.localStorage.setItem(STRICTNESS_KEY, id);
};

interface CustomSkill {
  id: string;          // local uuid — stable handle for uninstall
  capability: string;  // operator-chosen name, e.g. "myapp.send_email"
  description: string; // free-form, surfaces in the card body
  lane: string;        // operator-chosen lane label (default "custom")
  installed_at: string;
}

const loadPausedSkills = (): Set<string> => {
  if (typeof window === "undefined") return new Set();
  try {
    const raw = window.localStorage.getItem(SKILLS_PAUSED_KEY);
    if (!raw) return new Set();
    const parsed = JSON.parse(raw);
    return Array.isArray(parsed) ? new Set(parsed.filter((v) => typeof v === "string")) : new Set();
  } catch {
    return new Set();
  }
};
const savePausedSkills = (paused: Set<string>): void => {
  if (typeof window === "undefined") return;
  window.localStorage.setItem(SKILLS_PAUSED_KEY, JSON.stringify(Array.from(paused)));
};

const loadCustomSkills = (): CustomSkill[] => {
  if (typeof window === "undefined") return [];
  try {
    const raw = window.localStorage.getItem(SKILLS_CUSTOM_KEY);
    if (!raw) return [];
    const parsed = JSON.parse(raw);
    return Array.isArray(parsed) ? (parsed as CustomSkill[]) : [];
  } catch {
    return [];
  }
};
const saveCustomSkills = (list: CustomSkill[]): void => {
  if (typeof window === "undefined") return;
  window.localStorage.setItem(SKILLS_CUSTOM_KEY, JSON.stringify(list));
};

const SkillsSurface = ({ onOpenDirectoryTab }: { onOpenDirectoryTab: (tab: DirectoryTabId) => void }) => {
  const [caps, setCaps] = useState<CapabilityDescriptor[] | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [filter, setFilter] = useState("");
  const [scope, setScope] = useState<"catalog" | "user">("catalog");
  const [sort, setSort] = useState<"name" | "lane" | "status">("name");
  const [paused, setPaused] = useState<Set<string>>(() => loadPausedSkills());
  const [custom, setCustom] = useState<CustomSkill[]>(() => loadCustomSkills());
  const [installOpen, setInstallOpen] = useState(false);
  const [draftCap, setDraftCap] = useState("");
  const [draftDesc, setDraftDesc] = useState("");
  const [draftLane, setDraftLane] = useState("custom");
  const [editingCustomId, setEditingCustomId] = useState<string | null>(null);
  const [editingSkillId, setEditingSkillId] = useState<string | null>(null);
  const [skillDraft, setSkillDraft] = useState("");
  const [skillBusy, setSkillBusy] = useState<string | null>(null);
  const [toast, setToast] = useState<string | null>(null);
  const cancelled = useCancelledRef();

  const refreshCapabilities = async () => {
    try {
      const res = await fetchCapabilities();
      if (cancelled.current) return;
      setCaps(res.descriptors);
      setError(null);
    } catch (err: unknown) {
      if (cancelled.current) return;
      setError(err instanceof Error ? err.message : String(err));
    }
  };

  useEffect(() => {
    void refreshCapabilities();
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [cancelled]);

  const togglePause = (capability: string) => {
    setPaused((prev) => {
      const next = new Set(prev);
      if (next.has(capability)) next.delete(capability);
      else next.add(capability);
      savePausedSkills(next);
      return next;
    });
  };

  const resetCustomDraft = () => {
    setDraftCap("");
    setDraftDesc("");
    setDraftLane("custom");
    setEditingCustomId(null);
  };

  const openInstallCustom = () => {
    resetCustomDraft();
    setInstallOpen(true);
  };

  const openEditCustom = (skill: CustomSkill) => {
    setEditingCustomId(skill.id);
    setDraftCap(skill.capability);
    setDraftDesc(skill.description);
    setDraftLane(skill.lane);
    setInstallOpen(true);
  };

  const saveCustom = () => {
    const cap = draftCap.trim();
    if (!cap) {
      setToast("Capability name is required.");
      return;
    }
    if (
      caps?.some((c) => c.capability === cap) ||
      custom.some((c) => c.capability === cap && c.id !== editingCustomId)
    ) {
      setToast(`A skill named "${cap}" is already in your catalog.`);
      return;
    }
    const entry: CustomSkill = editingCustomId
      ? {
          ...(custom.find((c) => c.id === editingCustomId) as CustomSkill),
          capability: cap,
          description: draftDesc.trim() || "(no description)",
          lane: draftLane.trim() || "custom",
        }
      : {
      id:
        typeof crypto !== "undefined" && "randomUUID" in crypto
          ? crypto.randomUUID()
          : `${Date.now()}-${Math.random().toString(16).slice(2)}`,
      capability: cap,
      description: draftDesc.trim() || "(no description)",
      lane: draftLane.trim() || "custom",
      installed_at: new Date().toISOString(),
    };
    const next = editingCustomId
      ? custom.map((c) => (c.id === editingCustomId ? entry : c))
      : [...custom, entry];
    setCustom(next);
    saveCustomSkills(next);
    resetCustomDraft();
    setInstallOpen(false);
    setToast(`${editingCustomId ? "updated" : "installed"} "${cap}".`);
  };

  const uninstallCustom = (id: string) => {
    const target = custom.find((c) => c.id === id);
    if (!target) return;
    if (!confirm(`Uninstall "${target.capability}"?`)) return;
    const next = custom.filter((c) => c.id !== id);
    setCustom(next);
    saveCustomSkills(next);
    // Also clear any paused state keyed on the same capability so a
    // re-install starts clean.
    if (paused.has(target.capability)) {
      const np = new Set(paused);
      np.delete(target.capability);
      setPaused(np);
      savePausedSkills(np);
    }
    setToast(`uninstalled "${target.capability}".`);
  };

  const openEditInstalledSkill = async (id: string) => {
    setSkillBusy(`read:${id}`);
    setToast(null);
    try {
      const skill = await getInstalledSkill(id);
      setEditingSkillId(id);
      setSkillDraft(skill.content);
    } catch (err: unknown) {
      setToast(`open failed: ${err instanceof Error ? err.message : String(err)}`);
    } finally {
      setSkillBusy(null);
    }
  };

  const saveInstalledSkill = async () => {
    if (!editingSkillId) return;
    setSkillBusy(`save:${editingSkillId}`);
    setToast(null);
    try {
      await updateInstalledSkill(editingSkillId, skillDraft);
      setToast(`updated "${editingSkillId}".`);
      setEditingSkillId(null);
      setSkillDraft("");
      await refreshCapabilities();
    } catch (err: unknown) {
      setToast(`save failed: ${err instanceof Error ? err.message : String(err)}`);
    } finally {
      setSkillBusy(null);
    }
  };

  const removeInstalledSkill = async (id: string) => {
    if (!confirm(`Delete installed skill "${id}"?`)) return;
    setSkillBusy(`delete:${id}`);
    setToast(null);
    try {
      await deleteInstalledSkill(id);
      setToast(`deleted "${id}".`);
      if (paused.has(id)) {
        const nextPaused = new Set(paused);
        nextPaused.delete(id);
        setPaused(nextPaused);
        savePausedSkills(nextPaused);
      }
      await refreshCapabilities();
    } catch (err: unknown) {
      setToast(`delete failed: ${err instanceof Error ? err.message : String(err)}`);
    } finally {
      setSkillBusy(null);
    }
  };

  const filteredCustom = useMemo(() => {
    const q = filter.trim().toLowerCase();
    if (!q) return custom;
    return custom.filter(
      (c) =>
        c.capability.toLowerCase().includes(q) ||
        c.description.toLowerCase().includes(q),
    );
  }, [custom, filter]);

  const grouped = useMemo(() => {
    const q = filter.trim().toLowerCase();
    const groups = new Map<string, Map<string, CapabilityDescriptor[]>>();
    for (const cap of caps ?? []) {
      if (cap.lane.group !== "domain" && cap.lane.group !== "interface") continue;
      if (q && !cap.capability.toLowerCase().includes(q) && !cap.description.toLowerCase().includes(q)) {
        continue;
      }
      const groupName = cap.lane.group === "domain" ? "domain skills" : "interface skills";
      const lanes = groups.get(groupName) ?? new Map<string, CapabilityDescriptor[]>();
      const items = lanes.get(cap.lane.label) ?? [];
      items.push(cap);
      lanes.set(cap.lane.label, items);
      groups.set(groupName, lanes);
    }
    return groups;
  }, [caps, filter]);

  const builtInSkills = useMemo(() => {
    const q = filter.trim().toLowerCase();
    const list = (caps ?? []).filter((c) => {
      if (c.lane.group !== "domain" && c.lane.group !== "interface") return false;
      if (c.provider === "ordo-skill") return false;
      return !q || c.capability.toLowerCase().includes(q) || c.description.toLowerCase().includes(q);
    });
    return [...list].sort((a, b) => {
      if (sort === "lane") return `${a.lane.label}.${a.capability}`.localeCompare(`${b.lane.label}.${b.capability}`);
      if (sort === "status") {
        const ap = paused.has(a.capability) ? 1 : 0;
        const bp = paused.has(b.capability) ? 1 : 0;
        return ap - bp || a.capability.localeCompare(b.capability);
      }
      return a.capability.localeCompare(b.capability);
    });
  }, [caps, filter, paused, sort]);

  const installedSkills = useMemo(() => {
    const q = filter.trim().toLowerCase();
    const list = (caps ?? []).filter((c) => {
      if (c.provider !== "ordo-skill") return false;
      return !q || c.capability.toLowerCase().includes(q) || c.description.toLowerCase().includes(q);
    });
    return [...list].sort((a, b) => {
      if (sort === "lane") return `${a.lane.label}.${a.capability}`.localeCompare(`${b.lane.label}.${b.capability}`);
      if (sort === "status") {
        const ap = paused.has(a.capability) ? 1 : 0;
        const bp = paused.has(b.capability) ? 1 : 0;
        return ap - bp || a.capability.localeCompare(b.capability);
      }
      return a.capability.localeCompare(b.capability);
    });
  }, [caps, filter, paused, sort]);

  const totalCount = (caps?.filter((c) => c.lane.group === "domain" || c.lane.group === "interface").length ?? 0) + custom.length;
  const pausedCount = paused.size;
  const userSkillCount = filteredCustom.length + installedSkills.length;

  return (
    <div className="h-full flex flex-col gap-4 overflow-auto pb-4">
      <DirectoryFrame
        active="skills"
        search={filter}
        onSearch={setFilter}
        placeholder="Search skills..."
        onOpen={onOpenDirectoryTab}
        controls={
          <div className="flex items-center justify-between gap-3 w-full">
            <div className="flex items-center gap-2">
              <DirectoryPill active={scope === "catalog"} onClick={() => setScope("catalog")}>
                Ordo Skills
              </DirectoryPill>
              <DirectoryPill active={scope === "user"} onClick={() => setScope("user")}>
                User Added
              </DirectoryPill>
            </div>
            <div className="flex items-center gap-2">
              <DirectorySelect
                value={sort}
                onChange={setSort}
                options={[
                  { value: "name", label: "Sort by name" },
                  { value: "lane", label: "Sort by lane" },
                  { value: "status", label: "Sort by status" },
                ]}
              />
              <Button onClick={openInstallCustom} variant="primary" size="md">
                <span style={{ display: "inline-flex", alignItems: "center", gap: 6 }}>
                  <Plus size={13} strokeWidth={2.5} /> Install skill
                </span>
              </Button>
            </div>
          </div>
        }
      >
        <div className="space-y-4">
          {error && <Alert variant="danger">{error}</Alert>}
          {toast && <Alert variant="success">{toast}</Alert>}
          <Mono size={11} upper track="0.18em" color={UI.textMuted}>
            {totalCount} skills catalogued{pausedCount > 0 ? ` / ${pausedCount} paused` : ""}
          </Mono>

          {scope === "catalog" && builtInSkills.length > 0 && (
            <DirectoryGrid>
              {builtInSkills.map((skill) => {
                const isPaused = paused.has(skill.capability);
                return (
                  <DirectoryCard
                    key={skill.capability}
                    icon={<Wrench size={20} />}
                    title={skill.capability}
                    source={`${skill.lane.label} / ${skill.provider}`}
                    description={skill.description}
                    muted={isPaused}
                    actions={
                      <Button
                        onClick={() => togglePause(skill.capability)}
                        size="sm"
                        variant={isPaused ? "primary" : "ghost"}
                        title={isPaused ? "Resume skill" : "Pause skill"}
                      >
                        {isPaused ? <Play size={14} /> : <Pause size={14} />}
                      </Button>
                    }
                    badges={
                      <>
                        <Badge variant="neutral">{skill.tier}</Badge>
                        <Badge variant="info">{skill.lane.group}</Badge>
                        {isPaused && <Badge variant="warn">paused</Badge>}
                      </>
                    }
                  />
                );
              })}
            </DirectoryGrid>
          )}

          {scope === "user" && userSkillCount > 0 && (
            <DirectoryGrid>
              {filteredCustom.map((skill) => {
                const isPaused = paused.has(skill.capability);
                return (
                  <DirectoryCard
                    key={`custom:${skill.id}`}
                    icon={<Sparkles size={20} />}
                    title={skill.capability}
                    source={`custom / ${skill.lane}`}
                    description={skill.description}
                    muted={isPaused}
                    actions={
                      <>
                        <Button onClick={() => openEditCustom(skill)} size="sm" title="Edit skill">
                          <Wrench size={14} />
                        </Button>
                        <Button
                          onClick={() => togglePause(skill.capability)}
                          size="sm"
                          variant={isPaused ? "primary" : "ghost"}
                          title={isPaused ? "Resume skill" : "Pause skill"}
                        >
                          {isPaused ? <Play size={14} /> : <Pause size={14} />}
                        </Button>
                        <Button
                          onClick={() => uninstallCustom(skill.id)}
                          size="sm"
                          variant="danger"
                          title="Delete skill"
                        >
                          <Trash2 size={14} />
                        </Button>
                      </>
                    }
                    badges={
                      <>
                        <Badge variant="info">custom</Badge>
                        {isPaused && <Badge variant="warn">paused</Badge>}
                      </>
                    }
                  />
                );
              })}
              {installedSkills.map((skill) => {
                const isPaused = paused.has(skill.capability);
                return (
                  <DirectoryCard
                    key={`installed:${skill.capability}`}
                    icon={<FileText size={20} />}
                    title={skill.capability}
                    source={`${skill.lane.label} / installed skill.md`}
                    description={skill.description}
                    muted={isPaused}
                    actions={
                      <>
                        <Button
                          onClick={() => void openEditInstalledSkill(skill.capability)}
                          size="sm"
                          disabled={skillBusy === `read:${skill.capability}`}
                          title="Edit skill.md"
                        >
                          <Wrench size={14} />
                        </Button>
                        <Button
                          onClick={() => togglePause(skill.capability)}
                          size="sm"
                          variant={isPaused ? "primary" : "ghost"}
                          title={isPaused ? "Resume skill" : "Pause skill"}
                        >
                          {isPaused ? <Play size={14} /> : <Pause size={14} />}
                        </Button>
                        <Button
                          onClick={() => void removeInstalledSkill(skill.capability)}
                          size="sm"
                          variant="danger"
                          disabled={skillBusy === `delete:${skill.capability}`}
                          title="Delete skill"
                        >
                          <Trash2 size={14} />
                        </Button>
                      </>
                    }
                    badges={
                      <>
                        <Badge variant="info">user</Badge>
                        <Badge variant="neutral">{skill.tier}</Badge>
                        {isPaused && <Badge variant="warn">paused</Badge>}
                      </>
                    }
                  />
                );
              })}
            </DirectoryGrid>
          )}

          {caps === null && <DirectoryEmpty title="Loading skills" sub="Reading the runtime capability catalog." />}
          {caps !== null && scope === "catalog" && builtInSkills.length === 0 && !error && (
            <DirectoryEmpty
              title="No matching skills"
              sub={filter ? "Try a different search." : "No built-in skills are registered."}
            />
          )}
          {caps !== null && scope === "user" && userSkillCount === 0 && !error && (
            <DirectoryEmpty
              title="No user skills yet"
              sub="Install a skill to add your own reasoning or orchestration capability."
            />
          )}
        </div>
      </DirectoryFrame>


      <Modal
        open={installOpen}
        onClose={() => {
          setInstallOpen(false);
          resetCustomDraft();
        }}
        title={editingCustomId ? "Edit custom skill" : "Install a skill"}
        sub="Custom skills capture intent — what you want the agent to be able to do. They show up in your catalog and ride along in turn metadata. For tools that actually execute on the runtime, install an MCP server in the MCP tab."
        width={560}
        footer={
          <>
            <Button onClick={() => {
              setInstallOpen(false);
              resetCustomDraft();
            }}>Cancel</Button>
            <Button onClick={saveCustom} variant="primary" disabled={!draftCap.trim()}>
              {editingCustomId ? "Save" : "Install"}
            </Button>
          </>
        }
      >
        <div className="space-y-4">
          <Field label="Capability name" required hint="A dotted id like 'myapp.send_email' or 'finance.run_audit'.">
            <TextInput value={draftCap} onChange={setDraftCap} placeholder="myapp.do_thing" autoFocus />
          </Field>
          <Field label="Description" hint="One sentence — what the skill does, when to use it.">
            <Textarea value={draftDesc} onChange={setDraftDesc} rows={2} placeholder="Posts a daily digest to the team channel." />
          </Field>
          <Field label="Lane" hint="Group label this skill appears under in the catalog.">
            <TextInput value={draftLane} onChange={setDraftLane} placeholder="custom" />
          </Field>
        </div>
      </Modal>
      <Modal
        open={editingSkillId !== null}
        onClose={() => {
          setEditingSkillId(null);
          setSkillDraft("");
        }}
        title={`Edit ${editingSkillId ?? "skill"}`}
        sub="This edits the installed skill.md file directly under user-files/skills."
        width={760}
        footer={
          <>
            <Button
              onClick={() => {
                setEditingSkillId(null);
                setSkillDraft("");
              }}
              disabled={Boolean(skillBusy?.startsWith("save:"))}
            >
              Cancel
            </Button>
            <Button
              onClick={() => void saveInstalledSkill()}
              variant="primary"
              disabled={!skillDraft.trim() || Boolean(skillBusy?.startsWith("save:"))}
            >
              {skillBusy?.startsWith("save:") ? "Saving..." : "Save"}
            </Button>
          </>
        }
      >
        <Field label="skill.md" required>
          <Textarea value={skillDraft} onChange={setSkillDraft} rows={18} spellCheck={false} />
        </Field>
      </Modal>
    </div>
  );
};

// ─── Persona (operator self) ────────────────────────────────────
//
// The operator's facts about themselves — voice, brand, working
// preferences. Stored as assistant facts (assistant.list_facts /
// remember_fact / forget_fact) which are what the assistant turn
// already pulls into context.

const PersonaSurface = () => {
  const [facts, setFacts] = useState<AssistantFact[] | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState<string | null>(null);
  const [toast, setToast] = useState<string | null>(null);
  const [draft, setDraft] = useState("");

  const refresh = async () => {
    try {
      const list = await listAssistantFacts();
      setFacts(list);
      setError(null);
    } catch (err: unknown) {
      setError(err instanceof Error ? err.message : String(err));
    }
  };

  useEffect(() => {
    void refresh();
  }, []);

  const add = async () => {
    if (!draft.trim()) return;
    setBusy("add");
    setToast(null);
    try {
      await rememberFact(draft.trim());
      setToast("fact saved.");
      setDraft("");
      await refresh();
    } catch (err: unknown) {
      setToast(`save failed: ${err instanceof Error ? err.message : String(err)}`);
    } finally {
      setBusy(null);
    }
  };

  const remove = async (f: AssistantFact) => {
    if (!f.id) {
      setToast("can't forget a fact without an id (was it persisted?)");
      return;
    }
    setBusy(f.id);
    setToast(null);
    try {
      await forgetFact(f.id);
      setToast("fact removed.");
      await refresh();
    } catch (err: unknown) {
      setToast(`remove failed: ${err instanceof Error ? err.message : String(err)}`);
    } finally {
      setBusy(null);
    }
  };

  return (
    <div className="h-full flex flex-col gap-4 overflow-auto pb-4">
      <SectionHeader
        icon={<User size={22} />}
        title="Persona"
        sub="Facts about you. Voice, brand, working preferences — anything Ordo should know about the operator. Pulled into every assistant turn as system context."
      />
      {error && <Alert variant="danger">{error}</Alert>}
      {toast && <Alert variant="success">{toast}</Alert>}

      <Card padded={false}>
        <div style={{ padding: "12px 14px", display: "flex", gap: 8, alignItems: "stretch" }}>
          <div style={{ flex: 1 }}>
            <Textarea
              value={draft}
              onChange={setDraft}
              rows={2}
              placeholder='e.g. "I write in a Dorothy / Maude / Julia hybrid voice — withering when warranted."'
            />
          </div>
          <Button
            onClick={() => void add()}
            disabled={busy !== null || !draft.trim()}
            variant="primary"
            size="md"
          >
            {busy === "add" ? "Saving…" : "Save fact"}
          </Button>
        </div>
      </Card>

      <Mono size={11} upper track="0.18em" color={UI.textMuted}>
        operator facts · {facts?.length ?? 0}
      </Mono>
      <div className="space-y-2">
        {(facts ?? []).map((f, i) => {
          const key = f.id ?? `${i}-${f.content.slice(0, 32)}`;
          return (
            <Card key={key} padded={false}>
              <div style={{ padding: "12px 16px", display: "flex", gap: 12, alignItems: "flex-start" }}>
                <User size={14} color={UI.primary} style={{ marginTop: 3, flexShrink: 0 }} />
                <div style={{ flex: 1, minWidth: 0 }}>
                  <Serif size={13} color={UI.parchment} style={{ lineHeight: 1.55 }}>
                    {f.content}
                  </Serif>
                  {f.created_at && (
                    <div className="flex items-center gap-2 mt-2 flex-wrap">
                      {f.predicate && f.predicate !== "note" && (
                        <Badge variant="info">{f.predicate}</Badge>
                      )}
                      <Mono size={9} color={UI.textDim}>
                        {f.created_at}
                      </Mono>
                    </div>
                  )}
                </div>
                <Button
                  onClick={() => void remove(f)}
                  disabled={!f.id || busy === f.id}
                  variant="ghost"
                  size="sm"
                  title="forget"
                >
                  <Trash2 size={13} color={UI.slate} />
                </Button>
              </div>
            </Card>
          );
        })}
        {(facts ?? []).length === 0 && (
          <Card>
            <div style={{ textAlign: "center", padding: "16px 0" }}>
              <Serif size={14} italic color={UI.textMuted}>
                No persona facts yet. Add one above and Ordo will fold it into every turn.
              </Serif>
            </div>
          </Card>
        )}
      </div>
    </div>
  );
};

// ─── Agent Persona (Ordo's character) ───────────────────────────
//
// The agent's behavioral persona — how Ordo should sound when it
// speaks, what tone to default to, what to avoid. Stored as a single
// assistant.fact (subject="agent", predicate="persona") which the
// turn loop already pulls into context via the recalled-facts
// system message.

const AGENT_PERSONA_SUBJECT = "agent";
const AGENT_PERSONA_PREDICATE = "persona";

const AgentPersonaSurface = () => {
  const [existing, setExisting] = useState<AssistantFact | null>(null);
  const [text, setText] = useState("");
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const [toast, setToast] = useState<string | null>(null);

  const refresh = async () => {
    try {
      const all = await listAssistantFacts(AGENT_PERSONA_SUBJECT);
      const personaFact =
        all.find((f) => f.predicate === AGENT_PERSONA_PREDICATE) ?? null;
      setExisting(personaFact);
      setText(personaFact?.object ?? "");
      setError(null);
    } catch (err: unknown) {
      setError(err instanceof Error ? err.message : String(err));
    }
  };

  useEffect(() => {
    void refresh();
  }, []);

  const save = async () => {
    setBusy(true);
    setToast(null);
    try {
      // Replace strategy: forget the existing persona fact (if any)
      // then remember the new one. Single-entry semantics.
      if (existing?.id) {
        await forgetFact(existing.id);
      }
      const trimmed = text.trim();
      if (trimmed.length > 0) {
        await rememberFact(trimmed, {
          subject: AGENT_PERSONA_SUBJECT,
          predicate: AGENT_PERSONA_PREDICATE,
          object: trimmed,
        });
      }
      setToast(trimmed.length > 0 ? "agent persona saved." : "agent persona cleared.");
      await refresh();
    } catch (err: unknown) {
      setToast(`save failed: ${err instanceof Error ? err.message : String(err)}`);
    } finally {
      setBusy(false);
    }
  };

  const dirty = (existing?.object ?? "") !== text;

  return (
    <div className="h-full flex flex-col gap-4 overflow-auto pb-4">
      <SectionHeader
        icon={<Bot size={22} />}
        title="Agent Persona"
        sub="How Ordo should speak and behave. Stored as a single agent fact, durable across sessions, folded into every turn's context."
        trailing={
          <Button
            onClick={() => void save()}
            disabled={busy || !dirty}
            variant="primary"
            size="md"
          >
            {busy ? "Saving…" : dirty ? "Save persona" : "Saved"}
          </Button>
        }
      />
      {error && <Alert variant="danger">{error}</Alert>}
      {toast && <Alert variant="success">{toast}</Alert>}

      <Card>
        <Field
          label="Behavioral persona"
          hint='Examples: "Speak directly, no hedging." · "Default tone: thoughtful, contrarian when warranted." · "Avoid emojis unless the operator opts in."'
        >
          <Textarea
            value={text}
            onChange={setText}
            rows={12}
            placeholder="Describe how the agent should sound, default tone, what to avoid…"
          />
        </Field>
      </Card>

      <Mono size={10} color={UI.textDim} style={{ fontStyle: "italic" }}>
        Stored as <code style={{ fontFamily: MONO }}>assistant.fact</code> with{" "}
        <code style={{ fontFamily: MONO }}>subject="agent"</code>,{" "}
        <code style={{ fontFamily: MONO }}>predicate="persona"</code>. Replaces atomically on save.
        {existing?.created_at && (
          <>
            {" · last updated "}
            {existing.created_at}
          </>
        )}
      </Mono>
    </div>
  );
};

// ─── Agent Memory (operator-curated durable facts) ───────────────
//
// "What the user wants the agent to ALWAYS remember." Stored as
// assistant.facts with subject="agent", predicate="ground_truth".
// Distinct from the Memory tab (which is a system-level view of
// pinned + working memory + budgets) and from the Persona tab
// (operator self-description).

const AGENT_MEMORY_SUBJECT = "agent";
const AGENT_MEMORY_PREDICATE = "ground_truth";

const AgentMemorySurface = () => {
  const [items, setItems] = useState<AssistantFact[] | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState<string | null>(null);
  const [toast, setToast] = useState<string | null>(null);
  const [draft, setDraft] = useState("");

  const refresh = async () => {
    try {
      const all = await listAssistantFacts(AGENT_MEMORY_SUBJECT);
      setItems(all.filter((f) => f.predicate === AGENT_MEMORY_PREDICATE));
      setError(null);
    } catch (err: unknown) {
      setError(err instanceof Error ? err.message : String(err));
    }
  };

  useEffect(() => {
    void refresh();
  }, []);

  const add = async () => {
    if (!draft.trim()) return;
    setBusy("add");
    setToast(null);
    try {
      await rememberFact(draft.trim(), {
        subject: AGENT_MEMORY_SUBJECT,
        predicate: AGENT_MEMORY_PREDICATE,
        object: draft.trim(),
      });
      setToast("remembered.");
      setDraft("");
      await refresh();
    } catch (err: unknown) {
      setToast(`add failed: ${err instanceof Error ? err.message : String(err)}`);
    } finally {
      setBusy(null);
    }
  };

  const remove = async (f: AssistantFact) => {
    if (!f.id) return;
    setBusy(f.id);
    setToast(null);
    try {
      await forgetFact(f.id);
      setToast("forgotten.");
      await refresh();
    } catch (err: unknown) {
      setToast(`remove failed: ${err instanceof Error ? err.message : String(err)}`);
    } finally {
      setBusy(null);
    }
  };

  return (
    <div className="h-full flex flex-col gap-4 overflow-auto pb-4">
      <SectionHeader
        icon={<BookMarked size={22} />}
        title="Agent Memory"
        sub="What you want the agent to always remember. Each entry is stored as an agent fact and folded into every turn's context."
        trailing={
          <Button onClick={() => void refresh()} size="sm">
            <span style={{ display: "inline-flex", alignItems: "center", gap: 4 }}>
              <RefreshCcw size={11} /> Refresh
            </span>
          </Button>
        }
      />
      {error && <Alert variant="danger">{error}</Alert>}
      {toast && <Alert variant="success">{toast}</Alert>}

      <Card padded={false}>
        <div style={{ padding: "12px 14px", display: "flex", gap: 8, alignItems: "stretch" }}>
          <div style={{ flex: 1 }}>
            <Textarea
              value={draft}
              onChange={setDraft}
              rows={2}
              placeholder='e.g. "ordo-protocol message schemas are sacred — never mutate them in place."'
            />
          </div>
          <Button
            onClick={() => void add()}
            disabled={busy !== null || !draft.trim()}
            variant="primary"
            size="md"
          >
            {busy === "add" ? "Saving…" : "Remember"}
          </Button>
        </div>
      </Card>

      <Mono size={11} upper track="0.18em" color={UI.textMuted}>
        ground truth · {items?.length ?? 0}
      </Mono>
      <div className="space-y-2">
        {(items ?? []).map((f) => (
          <Card key={f.id ?? f.content} padded={false}>
            <div style={{ padding: "12px 16px", display: "flex", gap: 12, alignItems: "flex-start" }}>
              <BookMarked size={14} color={UI.primary} style={{ marginTop: 3, flexShrink: 0 }} />
              <div style={{ flex: 1, minWidth: 0 }}>
                <Serif size={13} color={UI.parchment} style={{ lineHeight: 1.55 }}>
                  {f.object ?? f.content}
                </Serif>
                {f.created_at && (
                  <div style={{ marginTop: 4 }}>
                    <Mono size={9} color={UI.textDim}>
                      {f.created_at}
                    </Mono>
                  </div>
                )}
              </div>
              <Button
                onClick={() => void remove(f)}
                disabled={!f.id || busy === f.id}
                variant="ghost"
                size="sm"
                title="forget"
              >
                <Trash2 size={13} color={UI.slate} />
              </Button>
            </div>
          </Card>
        ))}
        {(items ?? []).length === 0 && (
          <Card>
            <div style={{ textAlign: "center", padding: "16px 0" }}>
              <Serif size={14} italic color={UI.textMuted}>
                Nothing in agent memory yet. Add a fact above and Ordo will treat it as ground truth on every turn.
              </Serif>
            </div>
          </Card>
        )}
      </div>
    </div>
  );
};

const RagSurface = () => {
  const [collections, setCollections] = useState<RagCollection[] | null>(null);
  const [storage, setStorage] = useState<RuntimeStorage | null>(null);
  const [budgetGb, setBudgetGb] = useState(0);
  const [previewGoal, setPreviewGoal] = useState("summarize the current project notes");
  const [previewLanes, setPreviewLanes] = useState<string[] | null>(null);
  const [previewHits, setPreviewHits] = useState<number | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const [toast, setToast] = useState<string | null>(null);

  const refresh = async () => {
    try {
      const [cols, s] = await Promise.all([fetchRagCollections(), fetchRuntimeStorage()]);
      setCollections(cols.collections);
      setStorage(s);
      setBudgetGb(bytesToGb(s.rag_budget_bytes));
      setError(null);
    } catch (err: unknown) {
      setError(err instanceof Error ? err.message : String(err));
    }
  };

  useEffect(() => {
    void refresh();
  }, []);

  const runPreview = async () => {
    if (!previewGoal.trim()) return;
    setBusy(true);
    try {
      const res = await previewRagCollections(previewGoal);
      setPreviewLanes(res.effective_collections);
      setPreviewHits(res.hit_count);
    } catch (err: unknown) {
      setToast(`preview failed: ${err instanceof Error ? err.message : String(err)}`);
    } finally {
      setBusy(false);
    }
  };

  const saveBudget = async () => {
    setBusy(true);
    setToast(null);
    try {
      await updateRuntimeSettings({ rag_budget_bytes: gbToBytes(budgetGb) });
      setToast(`rag budget saved (${budgetGb} GB).`);
      await refresh();
    } catch (err: unknown) {
      setToast(`save failed: ${err instanceof Error ? err.message : String(err)}`);
    } finally {
      setBusy(false);
    }
  };

  return (
    <div className="h-full flex flex-col gap-4 overflow-auto pb-4">
      <SectionHeader
        icon={<Database size={22} />}
        title="RAG"
        sub="Retrieval lanes. main is always included. Specialized collections activate by inferred goal."
        trailing={
          <Button onClick={() => void saveBudget()} disabled={busy} variant="primary" size="md">
            {busy ? "Saving…" : "Save budget"}
          </Button>
        }
      />
      {error && <Alert variant="danger">{error}</Alert>}
      {toast && <Alert variant="success">{toast}</Alert>}

      <div className="grid grid-cols-2 gap-3">
        <Card>
          <Mono size={11} upper track="0.18em" color={UI.textMuted}>
            RAG storage budget
          </Mono>
          <div style={{ marginTop: 12 }}>
            <Slider value={budgetGb} max={500} onChange={setBudgetGb} color={UI.primary} unit="GB" />
          </div>
          <div style={{ marginTop: 10, paddingTop: 10, borderTop: `1px solid ${UI.cardBorder}` }}>
            <Mono size={10} color={UI.textDim}>
              rag_budget_bytes · persisted in ordo.db ·{" "}
              {storage ? gbToBytes(budgetGb).toLocaleString() : "—"} bytes
            </Mono>
          </div>
        </Card>
        <Card>
          <Mono size={11} upper track="0.18em" color={UI.textMuted}>
            Inferred lanes preview
          </Mono>
          <div style={{ marginTop: 10, display: "flex", gap: 6 }}>
            <div style={{ flex: 1 }}>
              <TextInput
                value={previewGoal}
                onChange={setPreviewGoal}
                placeholder="describe a goal…"
              />
            </div>
            <Button onClick={() => void runPreview()} disabled={busy} variant="primary" size="md">
              Preview
            </Button>
          </div>
          <div
            style={{
              display: "flex",
              alignItems: "center",
              gap: 6,
              marginTop: 10,
              flexWrap: "wrap",
            }}
          >
            {(previewLanes ?? []).map((lane) => {
              const c = RAG_LANE_TINT[lane] ?? UI.slate;
              return (
                <span
                  key={lane}
                  style={{
                    fontFamily: MONO,
                    fontSize: 11,
                    padding: "3px 10px",
                    borderRadius: 4,
                    background: `${c}1a`,
                    color: c,
                    border: `1px solid ${c}33`,
                  }}
                >
                  {lane}
                </span>
              );
            })}
            {previewLanes !== null && previewLanes.length === 0 && (
              <Serif size={11} italic color={UI.textDim}>
                no specialized lanes inferred (main only)
              </Serif>
            )}
            {previewLanes === null && (
              <Serif size={11} italic color={UI.textDim}>
                preview a goal to see which lanes engage
              </Serif>
            )}
            {previewHits !== null && (
              <Mono size={10} color={UI.textDim} style={{ marginLeft: "auto" }}>
                {previewHits} hits
              </Mono>
            )}
          </div>
        </Card>
      </div>

      <SectionHeader
        icon={<Database size={20} />}
        title={`Collections · ${collections?.length ?? 0}`}
        sub="Each collection is its own retrieval lane. Counts reflect indexed chunks."
      />

      <div className="grid grid-cols-4 gap-2">
        {(collections ?? []).map((c) => {
          const tint = RAG_LANE_TINT[c.name] ?? UI.slate;
          const active = c.document_count > 0 || c.name === "main";
          return (
            <div
              key={c.name}
              style={{
                position: "relative",
                background: active ? `linear-gradient(180deg, ${tint}1f, ${tint}06)` : UI.cardBg,
                border: `1px solid ${active ? `${tint}40` : UI.cardBorder}`,
                borderRadius: 10,
                padding: "12px 14px",
                overflow: "hidden",
              }}
              title={c.sample_titles.length ? c.sample_titles.slice(0, 3).join("\n") : c.label}
            >
              {active && (
                <motion.div
                  className="absolute inset-x-0 top-0"
                  style={{
                    height: 1.5,
                    background: `linear-gradient(90deg, transparent, ${tint}, transparent)`,
                  }}
                  animate={{ x: ["-100%", "100%"] }}
                  transition={{ duration: 3, repeat: Infinity, ease: "linear" }}
                />
              )}
              <div className="flex items-start justify-between">
                <Mono size={10} upper track="0.2em" color={active ? tint : UI.textDim}>
                  {c.name}
                </Mono>
                {c.name === "main" && <Pin size={10} color={UI.primary} />}
              </div>
              <div
                style={{
                  fontFamily: FRAUNCES,
                  fontSize: 22,
                  fontWeight: 600,
                  color: active ? UI.parchment : UI.textDim,
                  marginTop: 6,
                  lineHeight: 1,
                }}
              >
                {c.chunk_count}
              </div>
              <Mono size={10} color={UI.textDim} style={{ fontStyle: "italic" }}>
                chunks · {c.document_count} docs
              </Mono>
            </div>
          );
        })}
      </div>
    </div>
  );
};

const MemorySurface = () => {
  const [pinned, setPinned] = useState<string[] | null>(null);
  const [working, setWorking] = useState<string[] | null>(null);
  const [storage, setStorage] = useState<RuntimeStorage | null>(null);
  const [pinnedGb, setPinnedGb] = useState(0);
  const [workingGb, setWorkingGb] = useState(0);
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const [toast, setToast] = useState<string | null>(null);
  const [draft, setDraft] = useState("");

  const refresh = async () => {
    try {
      const [p, w, s] = await Promise.all([
        listPinnedMemory(200),
        listWorkingMemory(50),
        fetchRuntimeStorage(),
      ]);
      setPinned(flattenMemoryNotes(p));
      setWorking(flattenMemoryNotes(w));
      setStorage(s);
      setPinnedGb(bytesToGb(s.memory_pinned_budget_bytes));
      setWorkingGb(bytesToGb(s.memory_working_budget_bytes));
      setError(null);
    } catch (err: unknown) {
      setError(err instanceof Error ? err.message : String(err));
    }
  };

  useEffect(() => {
    void refresh();
  }, []);

  const addNote = async () => {
    if (!draft.trim()) return;
    setBusy(true);
    setToast(null);
    try {
      await pinNote(draft.trim());
      setToast("note pinned.");
      setDraft("");
      await refresh();
    } catch (err: unknown) {
      setToast(`pin failed: ${err instanceof Error ? err.message : String(err)}`);
    } finally {
      setBusy(false);
    }
  };

  const removeNote = async (content: string) => {
    setBusy(true);
    setToast(null);
    try {
      await unpinNote(content);
      setToast("note unpinned.");
      await refresh();
    } catch (err: unknown) {
      setToast(`unpin failed: ${err instanceof Error ? err.message : String(err)}`);
    } finally {
      setBusy(false);
    }
  };

  const saveBudgets = async () => {
    setBusy(true);
    setToast(null);
    try {
      await updateRuntimeSettings({
        memory_pinned_budget_bytes: gbToBytes(pinnedGb),
        memory_working_budget_bytes: gbToBytes(workingGb),
      });
      setToast(`memory budgets saved (pinned ${pinnedGb} GB · working ${workingGb} GB).`);
      await refresh();
    } catch (err: unknown) {
      setToast(`save failed: ${err instanceof Error ? err.message : String(err)}`);
    } finally {
      setBusy(false);
    }
  };

  return (
    <div className="h-full flex flex-col gap-4 overflow-auto pb-4">
      <SectionHeader
        icon={<Brain size={22} />}
        title="Memory"
        sub="Pinned memory is always available. Working memory is the active session shadow."
        trailing={
          <Button
            onClick={() => void saveBudgets()}
            disabled={busy}
            variant="primary"
            size="md"
          >
            {busy ? "Saving…" : "Save budgets"}
          </Button>
        }
      />
      {error && <Alert variant="danger">{error}</Alert>}
      {toast && <Alert variant="success">{toast}</Alert>}

      <div className="grid grid-cols-2 gap-3">
        <Card>
          <div className="flex items-center justify-between mb-3">
            <Mono size={11} upper track="0.18em" color={UI.textMuted}>
              Pinned budget
            </Mono>
            <Pin size={12} color={UI.primary} />
          </div>
          <Slider value={pinnedGb} max={100} onChange={setPinnedGb} color={UI.primary} unit="GB" />
        </Card>
        <Card>
          <div className="flex items-center justify-between mb-3">
            <Mono size={11} upper track="0.18em" color={UI.textMuted}>
              Working budget
            </Mono>
            <Brain size={12} color={UI.jade} />
          </div>
          <Slider value={workingGb} max={100} onChange={setWorkingGb} color={UI.jade} unit="GB" />
        </Card>
      </div>

      <SectionHeader
        icon={<Pin size={20} />}
        title={`Pinned · ${pinned?.length ?? 0}`}
        sub="Always-available context — your constitution."
      />

      <div className="space-y-2">
        {(pinned ?? []).slice(0, 50).map((p, i) => (
          <Card key={`${i}-${p.slice(0, 32)}`} padded={false}>
            <div
              style={{
                padding: "12px 16px",
                display: "flex",
                alignItems: "flex-start",
                gap: 12,
              }}
            >
              <Pin size={14} color={UI.primary} style={{ marginTop: 3, flexShrink: 0 }} />
              <div style={{ flex: 1, minWidth: 0 }}>
                <Serif size={13} italic color={UI.parchment} style={{ lineHeight: 1.55 }}>
                  {p}
                </Serif>
              </div>
              <Button
                onClick={() => void removeNote(p)}
                disabled={busy}
                variant="ghost"
                size="sm"
                title="unpin"
              >
                <PinOff size={13} color={UI.slate} />
              </Button>
            </div>
          </Card>
        ))}

        <Card padded={false}>
          <div style={{ padding: "10px 12px", display: "flex", gap: 8, alignItems: "center" }}>
            <div style={{ flex: 1 }}>
              <input
                type="text"
                value={draft}
                onChange={(e) => setDraft(e.target.value)}
                onKeyDown={(e) => {
                  if (e.key === "Enter" && !e.shiftKey) {
                    e.preventDefault();
                    void addNote();
                  }
                }}
                placeholder="Pin a new note (Enter to save)…"
                style={{
                  width: "100%",
                  padding: "8px 10px",
                  borderRadius: 6,
                  background: "rgba(0,0,0,0.25)",
                  border: `1px solid ${UI.inputBorder}`,
                  fontFamily: FRAUNCES,
                  fontSize: 13,
                  fontStyle: "italic",
                  color: UI.parchment,
                  outline: "none",
                }}
              />
            </div>
            <Button
              onClick={() => void addNote()}
              disabled={busy || !draft.trim()}
              variant="primary"
              size="sm"
            >
              <Plus size={13} strokeWidth={2.5} />
            </Button>
          </div>
        </Card>
      </div>

      <SectionHeader
        icon={<Brain size={20} />}
        title={`Working memory · ${working?.length ?? 0}`}
        sub="The active session shadow — fades after the session ends."
      />

      <div className="space-y-1.5">
        {(working ?? []).slice(0, 30).map((w, i) => (
          <Card key={i} padded={false}>
            <div style={{ padding: "10px 14px" }}>
              <Serif size={12} color={UI.parchment}>
                {w}
              </Serif>
            </div>
          </Card>
        ))}
        {(working ?? []).length === 0 && (
          <Card>
            <div style={{ textAlign: "center", padding: "12px 0" }}>
              <Serif size={12} italic color={UI.textMuted}>
                Working memory is empty for this session.
              </Serif>
            </div>
          </Card>
        )}
      </div>
    </div>
  );
};

const CapabilitiesSurface = () => {
  const [filter, setFilter] = useState("all");
  const [search, setSearch] = useState("");
  const [caps, setCaps] = useState<CapabilityDescriptor[] | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [loading, setLoading] = useState(true);

  useEffect(() => {
    let cancelled = false;
    setLoading(true);
    fetchCapabilities()
      .then((res) => {
        if (cancelled) return;
        setCaps(res.descriptors);
        setError(null);
      })
      .catch((err) => {
        if (cancelled) return;
        setError(err.message ?? String(err));
      })
      .finally(() => {
        if (!cancelled) setLoading(false);
      });
    return () => {
      cancelled = true;
    };
  }, []);

  const lanes = useMemo(() => {
    const set = new Set<string>(["all"]);
    for (const c of caps ?? []) set.add(c.lane.name);
    return Array.from(set);
  }, [caps]);

  const filtered = useMemo(() => {
    if (!caps) return [];
    const byLane = filter === "all" ? caps : caps.filter((c) => c.lane.name === filter);
    if (!search.trim()) return byLane;
    const q = search.toLowerCase();
    return byLane.filter(
      (c) =>
        c.capability.toLowerCase().includes(q) || c.description.toLowerCase().includes(q),
    );
  }, [caps, filter, search]);

  return (
    <div className="h-full flex flex-col gap-4 overflow-auto pb-4">
      <SectionHeader
        icon={<Boxes size={22} />}
        title="Capabilities"
        sub="What is advertising itself to the bus. The planner routes by capability, never by hardwired provider."
        trailing={
          <div style={{ width: 240 }}>
            <TextInput value={search} onChange={setSearch} placeholder="filter…" />
          </div>
        }
      />
      {error && <Alert variant="danger">failed to load capabilities: {error}</Alert>}

      <Card padded={false}>
        <div
          style={{
            padding: "12px 16px",
            display: "flex",
            flexWrap: "wrap",
            gap: 6,
            alignItems: "center",
          }}
        >
          {lanes.map((f) => (
            <button
              key={f}
              onClick={() => setFilter(f)}
              style={{
                padding: "5px 12px",
                borderRadius: 6,
                border: `1px solid ${filter === f ? UI.primaryBorder : UI.cardBorder}`,
                background: filter === f ? UI.primarySoft : "rgba(255,255,255,0.02)",
                color: filter === f ? UI.primary : UI.textMuted,
                fontFamily: MONO,
                fontSize: 11,
                fontWeight: filter === f ? 600 : 400,
                cursor: "pointer",
              }}
            >
              {f}
            </button>
          ))}
          <span style={{ flex: 1 }} />
          <Mono size={10} color={UI.textDim}>
            {loading ? "loading…" : `${filtered.length} of ${caps?.length ?? 0}`}
          </Mono>
        </div>
      </Card>

      <div className="space-y-2">
        {filtered.map((c) => (
          <CapabilityRow key={c.capability} cap={c} />
        ))}
        {filtered.length === 0 && !loading && (
          <Card>
            <div style={{ textAlign: "center", padding: "16px 0" }}>
              <Serif size={14} italic color={UI.textMuted}>
                No capabilities match.
              </Serif>
            </div>
          </Card>
        )}
      </div>
    </div>
  );
};

// Individual capability row — collapsible, with a JSON args editor and
// an Invoke button that POSTs through to /api/tools/<capability>.
const CapabilityRow = ({ cap }: { cap: CapabilityDescriptor }) => {
  const [open, setOpen] = useState(false);
  const tint = tintForLane(cap.lane.name);
  const eager = cap.activation === "Eager";
  const seedArgs = useMemo(() => {
    if (!cap.input_schema) return "{}";
    return JSON.stringify(seedFromSchema(cap.input_schema), null, 2);
  }, [cap.input_schema]);
  const [args, setArgs] = useState<string>(seedArgs);
  const [busy, setBusy] = useState(false);
  const [result, setResult] = useState<{ ok: boolean; body: string } | null>(null);

  // If a different cap re-uses the same row (after filter change), reset.
  useEffect(() => {
    setArgs(seedArgs);
    setResult(null);
  }, [seedArgs]);

  const invoke = async () => {
    setBusy(true);
    setResult(null);
    try {
      const parsed: unknown = args.trim() === "" ? {} : JSON.parse(args);
      const out = await invokeTool(cap.capability, parsed);
      setResult({ ok: true, body: JSON.stringify(out, null, 2) });
    } catch (err: unknown) {
      const msg =
        err instanceof Error
          ? err.message
          : typeof err === "string"
          ? err
          : JSON.stringify(err);
      setResult({ ok: false, body: msg });
    } finally {
      setBusy(false);
    }
  };

  return (
    <div
      className="rounded-md"
      style={{
        background: open ? "rgba(255,255,255,0.035)" : "rgba(255,255,255,0.02)",
        border: open ? `1px solid ${tint}55` : "1px solid rgba(255,255,255,0.04)",
      }}
    >
      <button
        onClick={() => setOpen((v) => !v)}
        className="w-full text-left rounded-md px-3 py-2.5 flex items-center gap-3"
        style={{ background: "transparent", border: "none", cursor: "pointer" }}
      >
        <motion.div
          style={{ width: 6, height: 6, borderRadius: 3, background: tint, flexShrink: 0 }}
          animate={{ opacity: [1, 0.3, 1] }}
          transition={{ duration: 2 + Math.random() * 2, repeat: Infinity }}
        />
        <div style={{ flex: 1, minWidth: 0 }}>
          <Mono size={12} color={PARCHMENT}>
            {cap.capability}
          </Mono>
          <div style={{ marginTop: 1 }}>
            <Mono size={10} color="rgba(255,255,255,0.4)">
              {cap.provider} · {cap.lane.label}
            </Mono>
          </div>
        </div>
        <Mono
          size={9}
          upper
          track="0.15em"
          color={cap.tier === "Core" ? `${LAMP}cc` : "rgba(255,255,255,0.45)"}
        >
          {cap.tier.toLowerCase()}
        </Mono>
        <Mono size={9} upper track="0.15em" color={eager ? JADE : "rgba(255,255,255,0.35)"}>
          {eager ? "eager" : "lazy"}
        </Mono>
        {open ? <ChevronDown size={12} color={SLATE} /> : <ChevronUp size={12} color={SLATE} />}
      </button>
      <AnimatePresence>
        {open && (
          <motion.div
            initial={{ height: 0, opacity: 0 }}
            animate={{ height: "auto", opacity: 1 }}
            exit={{ height: 0, opacity: 0 }}
            transition={{ duration: 0.2 }}
            style={{ overflow: "hidden" }}
          >
            <div className="px-4 py-3 space-y-3" style={{ borderTop: "1px solid rgba(255,255,255,0.05)" }}>
              <Serif size={13} italic color="rgba(244,236,216,0.85)" style={{ lineHeight: 1.5 }}>
                {cap.description}
              </Serif>
              <div>
                <Mono size={10} upper track="0.2em" color="rgba(255,255,255,0.4)">
                  arguments (JSON)
                </Mono>
                <textarea
                  value={args}
                  onChange={(e) => setArgs(e.target.value)}
                  spellCheck={false}
                  rows={Math.min(12, Math.max(3, args.split("\n").length))}
                  className="w-full mt-1 rounded-md p-3 outline-none"
                  style={{
                    fontFamily: MONO,
                    fontSize: 12,
                    color: PARCHMENT,
                    background: "rgba(0,0,0,0.35)",
                    border: "1px solid rgba(255,255,255,0.08)",
                    resize: "vertical",
                  }}
                />
              </div>
              {cap.input_schema && (
                <details>
                  <summary
                    style={{
                      cursor: "pointer",
                      fontFamily: MONO,
                      fontSize: 10,
                      color: "rgba(255,255,255,0.4)",
                      textTransform: "uppercase",
                      letterSpacing: "0.2em",
                    }}
                  >
                    input schema
                  </summary>
                  <pre
                    style={{
                      fontFamily: MONO,
                      fontSize: 11,
                      color: "rgba(255,255,255,0.7)",
                      background: "rgba(0,0,0,0.25)",
                      padding: 10,
                      borderRadius: 6,
                      marginTop: 6,
                      overflow: "auto",
                      maxHeight: 200,
                    }}
                  >
                    {JSON.stringify(cap.input_schema, null, 2)}
                  </pre>
                </details>
              )}
              <div className="flex items-center gap-2">
                <button
                  onClick={invoke}
                  disabled={busy}
                  className="rounded-md px-3 py-1.5 transition-all"
                  style={{
                    background: busy
                      ? "rgba(255,255,255,0.05)"
                      : `linear-gradient(180deg, ${LAMP}, #c89a3d)`,
                    color: busy ? SLATE : INK,
                    border: "none",
                    cursor: busy ? "wait" : "pointer",
                    fontFamily: MONO,
                    fontSize: 11,
                    fontWeight: 600,
                    letterSpacing: "0.05em",
                  }}
                >
                  {busy ? "invoking…" : "invoke"}
                </button>
                <Mono size={9} upper track="0.15em" color="rgba(255,255,255,0.35)">
                  POST /api/tools/{cap.capability}
                </Mono>
              </div>
              {result && (
                <div
                  className="rounded-md p-3"
                  style={{
                    background: result.ok ? `${JADE}10` : `${RED}10`,
                    border: `1px solid ${result.ok ? `${JADE}33` : `${RED}33`}`,
                  }}
                >
                  <Mono size={9} upper track="0.2em" color={result.ok ? JADE : RED}>
                    {result.ok ? "result" : "error"}
                  </Mono>
                  <pre
                    style={{
                      fontFamily: MONO,
                      fontSize: 11,
                      color: PARCHMENT,
                      marginTop: 4,
                      whiteSpace: "pre-wrap",
                      wordBreak: "break-word",
                      maxHeight: 280,
                      overflow: "auto",
                    }}
                  >
                    {result.body}
                  </pre>
                </div>
              )}
            </div>
          </motion.div>
        )}
      </AnimatePresence>
    </div>
  );
};

// Build a starter argument object from a JSON Schema. Conservative —
// fills only `required` fields (or all top-level properties if there
// is no `required` list) with placeholder values keyed off `type`.
function seedFromSchema(schema: Record<string, unknown>): unknown {
  const props = (schema.properties as Record<string, Record<string, unknown>>) ?? {};
  const required = (schema.required as string[]) ?? Object.keys(props);
  const out: Record<string, unknown> = {};
  for (const key of required) {
    const p = props[key];
    if (!p) continue;
    out[key] = seedValue(p);
  }
  return out;
}

function seedValue(p: Record<string, unknown>): unknown {
  if ("default" in p) return p.default;
  if (Array.isArray(p.enum)) return (p.enum as unknown[])[0];
  switch (p.type) {
    case "string":
      return "";
    case "integer":
    case "number":
      return 0;
    case "boolean":
      return false;
    case "array":
      return [];
    case "object":
      return {};
    default:
      return null;
  }
}

// ─── Cloud / LLM Providers ──────────────────────────────────────
//
// Modeled on the BrowserOS LLM Providers screen: section header,
// default-provider selector, quick template grid, configured list
// with Test/Edit/Delete, and a proper Configure modal.

interface ProviderTemplate {
  id: string;
  label: string;
  auth_style: "bearer" | "basic" | "api_key_header" | "api_key_query" | "anthropic";
  endpoint: string;
  default_model: string;
  default_context_window: number;
  supports_images: boolean;
  api_key_required: boolean;
  secret_source?: "vault" | "environment";
  env_var?: string;
  setup_url?: string;
  letter_color: string;
}

type OllamaConnectionMode = "local" | "signin" | "api-key";

const PROVIDER_TEMPLATES: ProviderTemplate[] = [
  {
    id: "anthropic",
    label: "Anthropic",
    auth_style: "anthropic",
    endpoint: "https://api.anthropic.com",
    default_model: "claude-sonnet-4-5",
    default_context_window: 200000,
    supports_images: true,
    api_key_required: true,
    setup_url: "https://console.anthropic.com/settings/keys",
    letter_color: "#cc785c",
  },
  {
    id: "anthropic-env",
    label: "Anthropic Local Env",
    auth_style: "anthropic",
    endpoint: "https://api.anthropic.com",
    default_model: "claude-sonnet-4-5",
    default_context_window: 200000,
    supports_images: true,
    api_key_required: false,
    secret_source: "environment",
    env_var: "ANTHROPIC_API_KEY",
    setup_url: "https://platform.claude.com/docs/en/api/authentication/overview",
    letter_color: "#cc785c",
  },
  {
    id: "openai",
    label: "OpenAI API",
    auth_style: "bearer",
    endpoint: "https://api.openai.com/v1",
    default_model: "gpt-5.1",
    default_context_window: 128000,
    supports_images: true,
    api_key_required: false,
    secret_source: "environment",
    env_var: "OPENAI_API_KEY",
    setup_url: "https://platform.openai.com/api-keys",
    letter_color: "#10a37f",
  },
  {
    id: "codex",
    label: "Codex / OpenAI Env",
    auth_style: "bearer",
    endpoint: "https://api.openai.com/v1",
    default_model: "gpt-5-codex",
    default_context_window: 128000,
    supports_images: true,
    api_key_required: false,
    secret_source: "environment",
    env_var: "OPENAI_API_KEY",
    setup_url: "https://platform.openai.com/docs",
    letter_color: "#10a37f",
  },
  {
    id: "google",
    label: "Google Gemini",
    auth_style: "api_key_query",
    endpoint: "https://generativelanguage.googleapis.com",
    default_model: "gemini-2.0-flash",
    default_context_window: 1048576,
    supports_images: true,
    api_key_required: true,
    setup_url: "https://aistudio.google.com/apikey",
    letter_color: "#4285f4",
  },
  {
    id: "openrouter",
    label: "OpenRouter Env",
    auth_style: "bearer",
    endpoint: "https://openrouter.ai/api/v1",
    default_model: "anthropic/claude-sonnet-4-5",
    default_context_window: 200000,
    supports_images: true,
    api_key_required: false,
    secret_source: "environment",
    env_var: "OPENROUTER_API_KEY",
    setup_url: "https://openrouter.ai/keys",
    letter_color: "#a98ad6",
  },
  {
    id: "ollama",
    label: "Ollama",
    auth_style: "bearer",
    endpoint: "http://localhost:11434/v1",
    // Empty so the field comes up blank when auto-detect fails — the
    // operator picks from whatever they actually have loaded. Picking
    // a name like "llama3.2" by default would silently misroute calls
    // when the operator has any other model loaded.
    default_model: "",
    // 14k tokens — modest default sized for typical desktop RAM
    // running a local model. Operators can bump in the modal if
    // their hardware can carry more without paging.
    default_context_window: 14336,
    supports_images: false,
    api_key_required: false,
    setup_url: "https://ollama.com/download",
    letter_color: "#000000",
  },
  {
    id: "ollama-cloud",
    label: "Ollama Cloud via Local Sign-In",
    auth_style: "bearer",
    // Cloud models are reached through a signed-in local Ollama daemon here.
    // The separate `ollama-cloud-api` provider instead talks directly to
    // Ollama's OpenAI-compatible cloud surface at https://ollama.com/v1.
    endpoint: "http://localhost:11434/v1",
    default_model: "gpt-oss:120b-cloud",
    default_context_window: 128000,
    supports_images: false,
    api_key_required: false,
    setup_url: "https://docs.ollama.com/api/authentication",
    letter_color: "#d6b25e",
  },
  {
    id: "ollama-cloud-api",
    label: "Ollama Cloud API",
    auth_style: "bearer",
    // OpenAI-compatible cloud surface. Chat posts {base}/chat/completions
    // and discovery hits {base}/models, so the base MUST be /v1 (the
    // native /api surface has no /chat/completions and 404s).
    endpoint: "https://ollama.com/v1",
    default_model: "gpt-oss:120b",
    default_context_window: 128000,
    supports_images: false,
    api_key_required: false,
    secret_source: "environment",
    env_var: "OLLAMA_API_KEY",
    setup_url: "https://docs.ollama.com/api/authentication",
    letter_color: "#d6b25e",
  },
  {
    id: "lmstudio",
    label: "LM Studio",
    auth_style: "bearer",
    endpoint: "http://localhost:1234/v1",
    default_model: "",
    // Same 14k default as Ollama — modest size for local hardware.
    default_context_window: 14336,
    supports_images: false,
    api_key_required: false,
    setup_url: "https://lmstudio.ai",
    letter_color: "#5865f2",
  },
  {
    id: "azure",
    label: "Azure OpenAI",
    auth_style: "api_key_header",
    endpoint: "https://YOUR-RESOURCE.openai.azure.com",
    default_model: "gpt-4o",
    default_context_window: 128000,
    supports_images: true,
    api_key_required: true,
    setup_url: "https://learn.microsoft.com/en-us/azure/ai-services/openai/",
    letter_color: "#0078d4",
  },
  {
    id: "bedrock",
    label: "Amazon Bedrock",
    auth_style: "bearer",
    endpoint: "https://bedrock-runtime.us-east-1.amazonaws.com",
    default_model: "anthropic.claude-sonnet-4-5",
    default_context_window: 200000,
    supports_images: true,
    api_key_required: true,
    setup_url: "https://docs.aws.amazon.com/bedrock/",
    // Was the Amazon orange (#ff9900); recolored to the Ordo peach
    // tone so the provider grid stays inside the brand palette.
    letter_color: "#f0b67f",
  },
  {
    id: "moonshot",
    label: "Moonshot AI",
    auth_style: "bearer",
    endpoint: "https://api.moonshot.cn/v1",
    default_model: "moonshot-v1-128k",
    default_context_window: 128000,
    supports_images: false,
    api_key_required: true,
    setup_url: "https://platform.moonshot.cn/",
    letter_color: "#7b68ee",
  },
  {
    id: "qwen",
    label: "Qwen",
    auth_style: "bearer",
    endpoint: "https://dashscope.aliyuncs.com/compatible-mode/v1",
    default_model: "qwen-max",
    default_context_window: 32768,
    supports_images: true,
    api_key_required: true,
    setup_url: "https://dashscope.console.aliyun.com/",
    // Was Qwen's orange (#ff7a00); recolored to the Ordo rose tone so
    // the provider grid stays inside the brand palette.
    letter_color: "#f07f9f",
  },
  {
    id: "groq",
    label: "Groq Env",
    auth_style: "bearer",
    endpoint: "https://api.groq.com/openai/v1",
    default_model: "llama-3.3-70b-versatile",
    default_context_window: 128000,
    supports_images: false,
    api_key_required: false,
    secret_source: "environment",
    env_var: "GROQ_API_KEY",
    setup_url: "https://console.groq.com/keys",
    letter_color: "#f55036",
  },
  {
    id: "openai-compatible",
    label: "Compatible Endpoint",
    auth_style: "bearer",
    endpoint: "https://your-server/v1",
    default_model: "your-model",
    default_context_window: 32768,
    supports_images: false,
    api_key_required: false,
    secret_source: "environment",
    env_var: "OPENAI_API_KEY",
    letter_color: "#888888",
  },
];

interface CredentialDraft {
  // Provider type (preset id, or "custom" / arbitrary string).
  service: string;
  // Display name (defaults to template label, editable). Stored in extras.name.
  name: string;
  auth_style: ProviderTemplate["auth_style"];
  endpoint: string;
  secret: string;
  model: string;
  context_window: number;
  temperature: number;
  supports_images: boolean;
  setup_url?: string;
  api_key_required?: boolean;
  secret_source?: "vault" | "environment";
  env_var?: string;
  letter_color?: string;
  enabled: boolean;
}

const blankDraft = (): CredentialDraft => ({
  service: "",
  name: "",
  auth_style: "bearer",
  endpoint: "",
  secret: "",
  model: "",
  context_window: 128000,
  temperature: 0.2,
  supports_images: false,
  secret_source: "vault",
  enabled: true,
});

const draftFromTemplate = (t: ProviderTemplate): CredentialDraft => ({
  service: t.id,
  name: t.label,
  auth_style: t.auth_style,
  endpoint: t.endpoint,
  secret: "",
  model: t.default_model,
  context_window: t.default_context_window,
  temperature: 0.2,
  supports_images: t.supports_images,
  setup_url: t.setup_url,
  api_key_required: t.api_key_required,
  secret_source: t.secret_source ?? "vault",
  env_var: t.env_var,
  letter_color: t.letter_color,
  enabled: true,
});

const credentialIsEnabled = (credential: CloudCredentialRow): boolean =>
  credential.enabled !== false && credential.extras?.enabled !== "false";

// Treat the runtime's redaction sentinel ("***") as "not set" so we
// never round-trip a placeholder back into storage. Operator-visible
// extras (model, name, context_window) are not redacted post-allowlist,
// but legacy credentials saved before the allowlist landed may still
// have literal "***" persisted; defensively skip those here.
const liveOrFallback = (
  raw: string | null | undefined,
  fallback: string,
): string => {
  if (!raw) return fallback;
  const trimmed = raw.trim();
  if (!trimmed || trimmed === "***") return fallback;
  return trimmed;
};

const defaultEnvVarForProvider = (
  provider: { id?: string; service?: string; auth_style: ProviderTemplate["auth_style"]; env_var?: string },
): string => {
  const service = (provider.service ?? provider.id ?? "").toLowerCase();
  if (provider.env_var?.trim()) return provider.env_var.trim();
  if (provider.auth_style === "anthropic") return "ANTHROPIC_API_KEY";
  if (provider.auth_style === "api_key_query" && (service.includes("google") || service.includes("gemini"))) {
    return "GOOGLE_API_KEY";
  }
  if (provider.auth_style === "api_key_header" && service.includes("azure")) {
    return "AZURE_OPENAI_API_KEY";
  }
  if (service.includes("openrouter")) return "OPENROUTER_API_KEY";
  if (service.includes("groq")) return "GROQ_API_KEY";
  if (service.includes("moonshot")) return "MOONSHOT_API_KEY";
  if (service.includes("qwen") || service.includes("dashscope")) return "DASHSCOPE_API_KEY";
  return "OPENAI_API_KEY";
};

const providerNeedsManualDetails = (t: ProviderTemplate): boolean =>
  t.endpoint.includes("YOUR-") ||
  t.endpoint.includes("your-server") ||
  t.default_model.includes("your-model") ||
  t.default_model.trim().length === 0;

const splitModelOptions = (raw: string | null | undefined): string[] => {
  if (!raw || raw.trim() === "***") return [];
  const trimmed = raw.trim();
  try {
    const parsed = JSON.parse(trimmed);
    if (Array.isArray(parsed)) {
      return parsed.filter((v): v is string => typeof v === "string");
    }
  } catch {
    // Plain comma/newline lists are accepted below.
  }
  return trimmed
    .split(/[\n,]/)
    .map((value) => value.trim())
    .filter(Boolean);
};

const uniqueModels = (models: Array<string | null | undefined>): string[] =>
  Array.from(
    new Set(
      models
        .map((model) => (model ?? "").trim())
        .filter((model) => model.length > 0 && model !== "***"),
    ),
  );

const cloudOllamaModels = (models: string[]) =>
  models.filter((name) => /(^|[:\-_])cloud$/i.test(name) || name.toLowerCase().includes(":cloud"));

const localOllamaModels = (models: string[]) =>
  models.filter((name) => !cloudOllamaModels([name]).length);

const pickChatModelFrom = (models: string[]) =>
  pickChatModel(models) ?? models[0] ?? "";

const pickOllamaCloudModel = (models: string[], preferred?: string | null) => {
  const cloudModels = cloudOllamaModels(models);
  const preferredModel = (preferred ?? "").trim();
  if (preferredModel && cloudModels.includes(preferredModel)) return preferredModel;
  return pickChatModelFrom(cloudModels) || cloudModels[0] || "";
};

const localDiscoveryProvider = (
  credential: CloudCredentialRow,
): "ollama" | "lmstudio" | null => {
  const service = credential.service.toLowerCase();
  const base = (credential.base_url ?? credential.endpoint ?? "").toLowerCase();
  if (service === "ollama-cloud-api" || base.includes("ollama.com")) return null;
  if (service.includes("ollama") || base.includes("localhost:11434")) return "ollama";
  if (service.includes("lmstudio") || service.includes("lm-studio") || base.includes("localhost:1234")) {
    return "lmstudio";
  }
  return null;
};

type CustomProviderShape =
  | "openai-compatible-env"
  | "openai-compatible-key"
  | "anthropic-env"
  | "anthropic-key"
  | "gemini-key"
  | "ollama-local"
  | "lmstudio-local";

const CUSTOM_PROVIDER_SHAPES: Array<{ value: CustomProviderShape; label: string }> = [
  { value: "openai-compatible-env", label: "OpenAI-compatible, environment key" },
  { value: "openai-compatible-key", label: "OpenAI-compatible, stored key" },
  { value: "anthropic-env", label: "Anthropic, environment key" },
  { value: "anthropic-key", label: "Anthropic, stored key" },
  { value: "gemini-key", label: "Gemini, stored key" },
  { value: "ollama-local", label: "Ollama local" },
  { value: "lmstudio-local", label: "LM Studio local" },
];

type ApiKeyWizardPresetId =
  | "openai"
  | "ollama-cloud-api"
  | "gemini"
  | "anthropic"
  | "openrouter"
  | "groq"
  | "custom";

interface ApiKeyWizardPreset {
  id: ApiKeyWizardPresetId;
  label: string;
  service: string;
  env_var: string;
  auth_style: ProviderTemplate["auth_style"];
  endpoint: string;
  model: string;
  context_window: number;
  supports_images: boolean;
}

interface ApiKeyWizardDraft extends ApiKeyWizardPreset {
  api_key: string;
}

const API_KEY_WIZARD_PRESETS: ApiKeyWizardPreset[] = [
  {
    id: "openai",
    label: "OpenAI / Codex",
    service: "openai",
    env_var: "OPENAI_API_KEY",
    auth_style: "bearer",
    endpoint: "https://api.openai.com/v1",
    model: "gpt-5.1",
    context_window: 128000,
    supports_images: true,
  },
  {
    id: "ollama-cloud-api",
    label: "Ollama Cloud API",
    service: "ollama-cloud-api",
    env_var: "OLLAMA_API_KEY",
    auth_style: "bearer",
    // OpenAI-compatible /v1 surface (see PROVIDER_TEMPLATES note above).
    endpoint: "https://ollama.com/v1",
    model: "gpt-oss:120b",
    context_window: 128000,
    supports_images: false,
  },
  {
    id: "gemini",
    label: "Gemini compatible",
    service: "gemini-compatible",
    env_var: "GEMINI_API_KEY",
    auth_style: "bearer",
    endpoint: "https://generativelanguage.googleapis.com/v1beta/openai",
    model: "gemini-2.0-flash",
    context_window: 1048576,
    supports_images: true,
  },
  {
    id: "anthropic",
    label: "Claude / Anthropic",
    service: "anthropic-env",
    env_var: "ANTHROPIC_API_KEY",
    auth_style: "anthropic",
    endpoint: "https://api.anthropic.com",
    model: "claude-sonnet-4-5",
    context_window: 200000,
    supports_images: true,
  },
  {
    id: "openrouter",
    label: "OpenRouter compatible",
    service: "openrouter",
    env_var: "OPENROUTER_API_KEY",
    auth_style: "bearer",
    endpoint: "https://openrouter.ai/api/v1",
    model: "anthropic/claude-sonnet-4-5",
    context_window: 200000,
    supports_images: true,
  },
  {
    id: "groq",
    label: "Groq compatible",
    service: "groq",
    env_var: "GROQ_API_KEY",
    auth_style: "bearer",
    endpoint: "https://api.groq.com/openai/v1",
    model: "llama-3.3-70b-versatile",
    context_window: 128000,
    supports_images: false,
  },
  {
    id: "custom",
    label: "Custom compatible",
    service: "custom-compatible",
    env_var: "OPENAI_API_KEY",
    auth_style: "bearer",
    endpoint: "https://your-server/v1",
    model: "your-model",
    context_window: 32768,
    supports_images: false,
  },
];

const apiKeyWizardDraft = (preset = API_KEY_WIZARD_PRESETS[0]): ApiKeyWizardDraft => ({
  ...preset,
  api_key: "",
});

const customShapeForDraft = (draft: CredentialDraft): CustomProviderShape => {
  if (draft.service === "ollama") return "ollama-local";
  if (draft.service === "lmstudio") return "lmstudio-local";
  if (draft.auth_style === "anthropic") {
    return draft.secret_source === "environment" ? "anthropic-env" : "anthropic-key";
  }
  if (draft.auth_style === "api_key_query") return "gemini-key";
  if (draft.secret_source === "environment") return "openai-compatible-env";
  return "openai-compatible-key";
};

const applyCustomProviderShape = (
  draft: CredentialDraft,
  shape: CustomProviderShape,
): CredentialDraft => {
  const serviceSeed = draft.service.trim();
  const generatedId =
    serviceSeed && serviceSeed !== "ollama" && serviceSeed !== "lmstudio"
      ? serviceSeed
      : `custom-compatible-${Date.now().toString(36)}`;
  switch (shape) {
    case "anthropic-env":
      return {
        ...draft,
        service: generatedId,
        name: draft.name || "Anthropic Endpoint",
        auth_style: "anthropic",
        endpoint: draft.endpoint || "https://api.anthropic.com",
        model: draft.model || "claude-sonnet-4-5",
        context_window: draft.context_window || 200000,
        supports_images: true,
        secret_source: "environment",
        api_key_required: false,
        env_var: draft.env_var || "ANTHROPIC_API_KEY",
      };
    case "anthropic-key":
      return {
        ...draft,
        service: generatedId,
        name: draft.name || "Anthropic Endpoint",
        auth_style: "anthropic",
        endpoint: draft.endpoint || "https://api.anthropic.com",
        model: draft.model || "claude-sonnet-4-5",
        context_window: draft.context_window || 200000,
        supports_images: true,
        secret_source: "vault",
        api_key_required: true,
        env_var: "",
      };
    case "gemini-key":
      return {
        ...draft,
        service: generatedId,
        name: draft.name || "Gemini Endpoint",
        auth_style: "api_key_query",
        endpoint: draft.endpoint || "https://generativelanguage.googleapis.com",
        model: draft.model || "gemini-2.0-flash",
        context_window: draft.context_window || 1048576,
        supports_images: true,
        secret_source: "vault",
        api_key_required: true,
        env_var: "",
      };
    case "ollama-local":
      return {
        ...draft,
        service: "ollama",
        name: "Ollama Local",
        auth_style: "bearer",
        endpoint: "http://localhost:11434/v1",
        model: draft.model,
        context_window: draft.context_window || 14336,
        supports_images: false,
        secret_source: "vault",
        api_key_required: false,
        env_var: "",
      };
    case "lmstudio-local":
      return {
        ...draft,
        service: "lmstudio",
        name: "LM Studio Local",
        auth_style: "bearer",
        endpoint: "http://localhost:1234/v1",
        model: draft.model,
        context_window: draft.context_window || 14336,
        supports_images: false,
        secret_source: "vault",
        api_key_required: false,
        env_var: "",
      };
    case "openai-compatible-key":
      return {
        ...draft,
        service: generatedId,
        name: draft.name || "Compatible Endpoint",
        auth_style: "bearer",
        endpoint: draft.endpoint || "https://your-server/v1",
        model: draft.model || "your-model",
        context_window: draft.context_window || 32768,
        supports_images: draft.supports_images,
        secret_source: "vault",
        api_key_required: true,
        env_var: "",
      };
    case "openai-compatible-env":
    default:
      return {
        ...draft,
        service: generatedId,
        name: draft.name || "Compatible Endpoint",
        auth_style: "bearer",
        endpoint: draft.endpoint || "https://your-server/v1",
        model: draft.model || "your-model",
        context_window: draft.context_window || 32768,
        supports_images: draft.supports_images,
        secret_source: "environment",
        api_key_required: false,
        env_var: draft.env_var || "OPENAI_API_KEY",
        enabled: draft.enabled,
      };
  }
};

const draftFromCredential = (
  c: CloudCredentialRow,
  template?: ProviderTemplate,
): CredentialDraft => {
  // Extras come back as plain strings on the wire (the runtime accepts
  // string-only values). Coerce numeric/bool fields back here.
  const extras = (c.extras ?? {}) as Record<string, string>;
  const secretSource =
    extras.auth_source === "environment" ? "environment" : template?.secret_source ?? "vault";
  const ctxRaw = liveOrFallback(extras.context_window, "");
  const tempRaw = liveOrFallback(extras.temperature, "");
  const ctx = ctxRaw ? Number(ctxRaw) : NaN;
  const temp = tempRaw ? Number(tempRaw) : NaN;
  return {
    service: c.service,
    name: liveOrFallback(
      liveOrFallback(c.label, "") || extras.name,
      template?.label ?? c.service,
    ),
    auth_style: (c.auth_style as CredentialDraft["auth_style"]) ?? "bearer",
    endpoint: liveOrFallback(c.base_url ?? c.endpoint, template?.endpoint ?? ""),
    secret: "",
    model: liveOrFallback(extras.model, template?.default_model ?? ""),
    context_window: Number.isFinite(ctx)
      ? ctx
      : template?.default_context_window ?? 128000,
    temperature: Number.isFinite(temp) ? temp : 0.2,
    supports_images:
      extras.supports_images === "true"
        ? true
        : extras.supports_images === "false"
        ? false
        : template?.supports_images ?? false,
    setup_url: template?.setup_url,
    api_key_required:
      secretSource === "environment" ? false : template?.api_key_required ?? true,
    secret_source: secretSource,
    env_var: liveOrFallback(extras.env_var, template ? defaultEnvVarForProvider(template) : ""),
    letter_color: template?.letter_color,
    enabled: credentialIsEnabled(c),
  };
};

const findTemplate = (service: string): ProviderTemplate | undefined =>
  PROVIDER_TEMPLATES.find((t) => t.id === service);

// Letter avatar: the first letter of the provider name in a colored
// rounded square. Inline replacement for brand SVGs we don't ship.
const BrandMark = ({ name, color, size = 30 }: { name: string; color?: string; size?: number }) => {
  const letter = (name.trim()[0] ?? "?").toUpperCase();
  const tint = color ?? "#888";
  return (
    <div
      style={{
        width: size,
        height: size,
        borderRadius: 7,
        background: `${tint}1f`,
        border: `1px solid ${tint}55`,
        display: "flex",
        alignItems: "center",
        justifyContent: "center",
        flexShrink: 0,
      }}
    >
      <span
        style={{
          fontFamily: FRAUNCES,
          fontSize: size * 0.45,
          fontWeight: 700,
          color: tint,
        }}
      >
        {letter}
      </span>
    </div>
  );
};

const CloudSurface = () => {
  const [creds, setCreds] = useState<CloudCredentialRow[] | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState<string | null>(null);
  const [toast, setToast] = useState<string | null>(null);
  const [editing, setEditing] = useState<CredentialDraft | null>(null);
  const [editingMode, setEditingMode] = useState<"new" | "rotate">("new");
  const [keyWizard, setKeyWizard] = useState<ApiKeyWizardDraft | null>(null);
  const [keyWizardError, setKeyWizardError] = useState<string | null>(null);
  const [keyWizardResult, setKeyWizardResult] = useState<LocalApiKeyInstallResult | null>(null);
  // Sticky inline error inside the configure modal — survives until
  // the operator fixes the form and tries again. Distinct from the
  // surface-level toast (which can fade).
  const [saveError, setSaveError] = useState<string | null>(null);
  const [defaultId, setDefaultIdRaw] = useState<string | null>(
    typeof window !== "undefined"
      ? window.localStorage.getItem("ordo:default_provider")
      : null,
  );
  const [testing, setTesting] = useState<string | null>(null);
  const [testResult, setTestResult] = useState<
    Record<string, { ok: boolean; ms: number; body: string }>
  >({});
  const [modelDiscovery, setModelDiscovery] = useState<
    Record<
      string,
      { status: "loading" | "ready" | "failed"; models?: string[]; error?: string; open?: boolean }
    >
  >({});
  const [ollamaConnectionMode, setOllamaConnectionMode] = useState<OllamaConnectionMode>("local");
  const [localProbe, setLocalProbe] = useState<Record<string, { reachable: boolean; models: string[]; base_url: string; error?: string }>>({});
  const [localSelectedModel, setLocalSelectedModel] = useState<Record<string, string>>({});

  const setDefaultId = (id: string | null) => {
    setDefaultIdRaw(id);
    if (typeof window !== "undefined") {
      if (id) window.localStorage.setItem("ordo:default_provider", id);
      else window.localStorage.removeItem("ordo:default_provider");
    }
  };

  const refresh = async () => {
    try {
      const res = await listCloudCredentials();
      setCreds(res.credentials);
      setError(null);
      const enabledCredentials = res.credentials.filter(credentialIsEnabled);
      if (defaultId && !enabledCredentials.some((c) => c.service === defaultId)) {
        setDefaultId(null);
      }
      // Auto-promote first enabled credential to default if nothing chosen yet.
      if (!defaultId && enabledCredentials.length > 0) {
        setDefaultId(enabledCredentials[0].service);
      }
    } catch (err: unknown) {
      setError(err instanceof Error ? err.message : String(err));
    }
  };

  useEffect(() => {
    void refresh();
  }, []);

  const openTemplate = async (t: ProviderTemplate) => {
    publishUxiDebugEvent("ordo.provider", "provider_template_selected", "Provider template selected.", {
      provider: t.id,
      auth_style: t.auth_style,
      secret_source: t.secret_source ?? "vault",
    });
    if (t.secret_source === "environment") {
      const envVar = defaultEnvVarForProvider(t);
      if (providerNeedsManualDetails(t)) {
        setEditing({ ...draftFromTemplate(t), env_var: envVar });
        setEditingMode("new");
        return;
      }
      setBusy(t.id);
      setToast(`connecting ${t.label} through ${envVar}...`);
      try {
        await upsertCloudCredential({
          service: t.id,
          auth_style: t.auth_style,
          base_url: t.endpoint,
          label: t.label,
          extras: {
            name: t.label,
            model: t.default_model,
            context_window: String(t.default_context_window),
            temperature: "0.2",
            supports_images: t.supports_images ? "true" : "false",
            enabled: "true",
            auth_source: "environment",
            env_var: envVar,
            timeout_secs: String(loadTimeoutPreset()),
            ...(t.id === "ollama-cloud-api" ? { provider_kind: "cloud_model" } : {}),
          },
        });
        publishUxiDebugEvent("ordo.provider", "provider_saved", "Environment-backed provider saved.", {
          provider: t.id,
          auth_style: t.auth_style,
          secret_source: "environment",
          env_var: envVar,
        });
        setToast(
          `${t.label} added. Ordo will read ${envVar} from the runtime environment when it calls this provider.`,
        );
        if (!defaultId) setDefaultId(t.id);
        await refresh();
      } catch (err: unknown) {
        setToast(
          `environment provider setup failed: ${err instanceof Error ? err.message : String(err)}`,
        );
        setEditing(draftFromTemplate(t));
        setEditingMode("new");
      } finally {
        setBusy(null);
      }
      return;
    }
    // Local providers (Ollama, LM Studio) auto-connect: probe the local
    // server, pick a non-embedding model, save the credential, mark as
    // default if nothing else is configured. No modal in the happy path.
    const localKind =
      t.id === "ollama" || t.id === "ollama-cloud"
        ? "ollama"
        : t.id === "lmstudio"
        ? "lmstudio"
        : null;
    if (localKind) {
      setBusy(t.id);
      setToast(`detecting ${t.label}…`);
      try {
        const found = await detectLocalLlm(localKind);
        const discoveredModels =
          t.id === "ollama-cloud"
            ? cloudOllamaModels(found.models)
            : t.id === "ollama"
            ? localOllamaModels(found.models)
            : found.models;
        const scopedFound = { ...found, models: discoveredModels };
        if (!found.reachable) {
          setToast(
            `${t.label} not reachable on ${found.base_url}${found.error ? `: ${found.error}` : ""}. Make sure it's running, then try again — or click USE again to configure manually.`,
          );
          // Fall back to the modal so the operator can fix it by hand.
          setEditing(draftFromTemplate(t));
          setEditingMode("new");
          setBusy(null);
          return;
        }
        if (discoveredModels.length === 0) {
          const message =
            t.id === "ollama-cloud"
              ? "Ollama is reachable, but no cloud models were listed. Sign in with ollama signin, then pull or select a cloud model."
              : `${t.label} is reachable, but no local-only models were listed. Cloud models stay under Ollama Cloud via Sign-In.`;
          setToast(message);
          setLocalProbe((prev) => ({ ...prev, [t.id]: scopedFound }));
          if (t.id !== "ollama-cloud") {
            setEditing(draftFromTemplate(t));
            setEditingMode("new");
          }
          setBusy(null);
          return;
        }
        const model =
          t.id === "ollama-cloud"
            ? pickOllamaCloudModel(discoveredModels, t.default_model)
            : pickChatModel(discoveredModels) ?? discoveredModels[0];
        if (t.id === "ollama-cloud" && !model) {
          setToast("Ollama is reachable, but no cloud models were listed. Sign in with ollama signin, then pull or select a cloud model.");
          setLocalProbe((prev) => ({ ...prev, [t.id]: scopedFound }));
          setBusy(null);
          return;
        }
        await upsertCloudCredential({
          service: t.id,
          auth_style: t.auth_style,
          base_url: t.endpoint,
          // Local providers don't validate the secret; send a placeholder
          // so the credential row reports has_secret=true and goes through
          // the same auth path as remote providers.
          secret: "local",
          label: `${t.label} (local)`,
          extras: {
            name: `${t.label} (local)`,
            model,
            context_window: String(t.default_context_window),
            temperature: "0.2",
            supports_images: t.supports_images ? "true" : "false",
            enabled: "true",
            // Inherit the operator's timeout preset (set in the
            // Runtime tab, default 5 min) so this provider gets the
            // same response budget as everything else.
            timeout_secs: String(loadTimeoutPreset()),
          },
        });
        publishUxiDebugEvent("ordo.provider", "provider_saved", "Local fallback provider saved.", {
          provider: t.id,
          auth_style: t.auth_style,
          model,
        });
        setToast(
          `${t.label} connected. Picked model "${model}" from ${discoveredModels.length} discovered.`,
        );
        if (!defaultId) setDefaultId(t.id);
        await refresh();
      } catch (err: unknown) {
        setToast(
          `auto-connect failed: ${err instanceof Error ? err.message : String(err)}`,
        );
        setEditing(draftFromTemplate(t));
        setEditingMode("new");
      } finally {
        setBusy(null);
      }
      return;
    }
    setEditing(draftFromTemplate(t));
    setEditingMode("new");
  };

  const openCustom = () => {
    setEditing({
      ...blankDraft(),
      service: `custom-compatible-${Date.now().toString(36)}`,
      name: "Compatible Endpoint",
      auth_style: "bearer",
      endpoint: "https://your-server/v1",
      secret_source: "environment",
      api_key_required: false,
      env_var: "OPENAI_API_KEY",
    });
    setEditingMode("new");
  };

  const openKeyWizard = (presetId: ApiKeyWizardPresetId = "openai") => {
    const preset = API_KEY_WIZARD_PRESETS.find((item) => item.id === presetId) ?? API_KEY_WIZARD_PRESETS[0];
    setKeyWizard(apiKeyWizardDraft(preset));
    setKeyWizardError(null);
    setKeyWizardResult(null);
  };

  const probeLocalProvider = async (provider: "ollama" | "lmstudio") => {
    setBusy(`probe-${provider}`);
    try {
      const found = await detectLocalLlm(provider);
      const scopedModels = provider === "ollama" ? localOllamaModels(found.models) : found.models;
      const scopedFound = { ...found, models: scopedModels };
      setLocalProbe((prev) => ({ ...prev, [provider]: scopedFound }));
      const picked = pickChatModelFrom(scopedModels);
      setLocalSelectedModel((prev) => ({ ...prev, [provider]: found.reachable ? picked : "" }));
      publishUxiDebugEvent("ordo.provider", "local_model_probe_completed", "Local model probe completed.", {
        provider,
        reachable: found.reachable,
        model_count: scopedModels.length,
        raw_model_count: found.models.length,
        base_url: found.base_url,
      }, found.reachable ? "INFO" : "WARN");
      setToast(
        found.reachable
          ? `${provider === "ollama" ? "Ollama Local" : "LM Studio"} detected ${scopedModels.length} model${scopedModels.length === 1 ? "" : "s"}.`
          : `${provider === "ollama" ? "Ollama" : "LM Studio"} not reachable${found.error ? `: ${found.error}` : ""}.`,
      );
    } catch (err: unknown) {
      const message = err instanceof Error ? err.message : String(err);
      setLocalProbe((prev) => ({
        ...prev,
        [provider]: {
          reachable: false,
          models: [],
          base_url: provider === "ollama" ? "http://localhost:11434/v1" : "http://localhost:1234/v1",
          error: message,
        },
      }));
      setToast(`local model probe failed: ${message}`);
    } finally {
      setBusy(null);
    }
  };

  const probeOllamaCloudModels = async () => {
    const provider = "ollama-cloud";
    setBusy(`probe-${provider}`);
    try {
      const found = await detectLocalLlm("ollama");
      const cloudModels = cloudOllamaModels(found.models);
      const cloudFound = { ...found, models: cloudModels };
      const picked = pickOllamaCloudModel(cloudModels, localSelectedModel[provider]);
      setLocalProbe((prev) => ({ ...prev, [provider]: cloudFound }));
      if (found.reachable && picked) {
        setLocalSelectedModel((prev) => ({ ...prev, [provider]: picked }));
      }
      const cloudCount = cloudModels.length;
      publishUxiDebugEvent("ordo.provider", "ollama_cloud_probe_completed", "Ollama Cloud model probe completed.", {
        provider,
        reachable: found.reachable,
        model_count: found.models.length,
        cloud_model_count: cloudCount,
        base_url: found.base_url,
      }, found.reachable && cloudCount > 0 ? "INFO" : "WARN");
      setToast(
        found.reachable
          ? cloudCount > 0
            ? `Ollama Cloud detected ${cloudCount} cloud model${cloudCount === 1 ? "" : "s"}.`
            : "Ollama is reachable, but no *-cloud models were listed. Sign in to Ollama and pull/select a cloud model, or enter the cloud model name manually."
          : `Ollama Cloud setup needs local Ollama signed in and running${found.error ? `: ${found.error}` : ""}.`,
      );
    } catch (err: unknown) {
      const message = err instanceof Error ? err.message : String(err);
      setLocalProbe((prev) => ({
        ...prev,
        "ollama-cloud": {
          reachable: false,
          models: [],
          base_url: "http://localhost:11434/v1",
          error: message,
        },
      }));
      setToast(`Ollama Cloud probe failed: ${message}`);
    } finally {
      setBusy(null);
    }
  };

  const connectLocalModel = async (provider: "ollama" | "lmstudio", model?: string) => {
    const template = findTemplate(provider === "ollama" ? "ollama" : "lmstudio");
    if (!template) return;
    if (!model) {
      await openTemplate(template);
      return;
    }
    if (provider === "ollama" && cloudOllamaModels([model]).length > 0) {
      setToast("That is an Ollama Cloud model. Use Ollama Cloud via Sign-In instead of Ollama Local.");
      return;
    }
    setBusy(`connect-${provider}`);
    try {
      await upsertCloudCredential({
        service: template.id,
        auth_style: template.auth_style,
        base_url: template.endpoint,
        secret: "local",
        label: `${template.label} (local)`,
        extras: {
          name: `${template.label} (local)`,
          model,
          context_window: String(template.default_context_window),
          temperature: "0.2",
          supports_images: template.supports_images ? "true" : "false",
          enabled: "true",
          provider_kind: "local_model",
          timeout_secs: String(loadTimeoutPreset()),
        },
      });
      if (!defaultId) setDefaultId(template.id);
      publishUxiDebugEvent("ordo.provider", "local_model_connected", "Local model connected.", {
        provider: template.id,
        model,
        base_url: template.endpoint,
      });
      setToast(`${template.label} connected with ${model}.`);
      await refresh();
    } catch (err: unknown) {
      setToast(`local model connect failed: ${err instanceof Error ? err.message : String(err)}`);
    } finally {
      setBusy(null);
    }
  };

  const connectOllamaCloudModel = async (model?: string) => {
    const template = findTemplate("ollama-cloud");
    if (!template) return;
    const selectedModel = model?.trim() || template.default_model;
    if (!selectedModel) {
      setEditing(draftFromTemplate(template));
      setEditingMode("new");
      return;
    }
    setBusy("connect-ollama-cloud");
    try {
      await upsertCloudCredential({
        service: template.id,
        auth_style: template.auth_style,
        base_url: template.endpoint,
        secret: "local",
        label: template.label,
        extras: {
          name: template.label,
          model: selectedModel,
          context_window: String(template.default_context_window),
          temperature: "0.2",
          supports_images: template.supports_images ? "true" : "false",
          enabled: "true",
          provider_kind: "ollama_cloud_via_local_ollama",
          requires_ollama_signin: "true",
          timeout_secs: String(loadTimeoutPreset()),
        },
      });
      if (!defaultId) setDefaultId(template.id);
      publishUxiDebugEvent("ordo.provider", "ollama_cloud_connected", "Ollama Cloud provider connected.", {
        provider: template.id,
        model: selectedModel,
        base_url: template.endpoint,
      });
      setToast(`Ollama Cloud connected with ${selectedModel}.`);
      await refresh();
    } catch (err: unknown) {
      setToast(`Ollama Cloud connect failed: ${err instanceof Error ? err.message : String(err)}`);
    } finally {
      setBusy(null);
    }
  };

  const openRotate = (c: CloudCredentialRow) => {
    const t = findTemplate(c.service);
    setEditing(draftFromCredential(c, t));
    setEditingMode("rotate");
  };

  const installKeyWizard = async () => {
    if (!keyWizard) return;
    setKeyWizardError(null);
    setBusy("api-key-wizard");
    try {
      const installed = await installLocalApiKeyEnv(keyWizard.env_var, keyWizard.api_key);
      const service =
        keyWizard.id === "custom"
          ? `${keyWizard.service}-${Date.now().toString(36)}`
          : keyWizard.service;
      await upsertCloudCredential({
        service,
        auth_style: keyWizard.auth_style,
        base_url: keyWizard.endpoint,
        label: keyWizard.label,
        extras: {
          name: keyWizard.label,
          model: keyWizard.model,
          context_window: String(keyWizard.context_window),
          temperature: "0.2",
          supports_images: keyWizard.supports_images ? "true" : "false",
          enabled: "true",
          auth_source: "environment",
          env_var: keyWizard.env_var,
          timeout_secs: String(loadTimeoutPreset()),
          ...(service === "ollama-cloud-api" ? { provider_kind: "cloud_model" } : {}),
        },
      });
      setKeyWizardResult(installed);
      setToast(`${keyWizard.label} installed locally via ${keyWizard.env_var}.`);
      if (!defaultId) setDefaultId(service);
      await refresh();
      setKeyWizard({ ...keyWizard, api_key: "" });
    } catch (err: unknown) {
      const message = err instanceof Error ? err.message : String(err);
      setKeyWizardError(message);
      setToast(`key install failed: ${message}`);
    } finally {
      setBusy(null);
    }
  };

  const saveDraft = async () => {
    if (!editing) return;
    // Surface validation errors INLINE in the modal (via setSaveError)
    // rather than the surface-level toast, so the operator sees them
    // attached to the form they're trying to submit.
    setSaveError(null);
    const trimmedService = editing.service.trim();
    const trimmedModel = (editing.model ?? "").trim();
    if (!trimmedService) {
      setSaveError("Provider id is required.");
      return;
    }
    if (!trimmedModel) {
      setSaveError("Model is required — pick whichever model the provider serves.");
      return;
    }
    const isEnvironmentBacked = editing.secret_source === "environment";
    const envVar = isEnvironmentBacked ? defaultEnvVarForProvider(editing) : "";
    if (isEnvironmentBacked && !envVar.trim()) {
      setSaveError("Environment variable is required for env-backed providers.");
      return;
    }
    if (editing.api_key_required !== false && !editing.secret.trim() && !isEnvironmentBacked && editingMode === "new") {
      setSaveError("API key is required for this provider.");
      return;
    }
    setBusy(editing.service);
    setToast(null);
    try {
      // Runtime keeps extras as plain strings — stringify everything
      // we want to round-trip. base_url (not endpoint) is the canonical
      // URL field on CloudCredentialUpdate. For local providers that
      // don't validate the secret (Ollama, LM Studio), we still send
      // a placeholder so has_secret reads true and the bearer header
      // dispatch path stays consistent.
      const isLocal = editing.api_key_required === false && editing.secret_source !== "environment";
      const secretToSend =
        editing.secret.trim() || (isLocal ? "local" : undefined);
      await upsertCloudCredential({
        service: trimmedService,
        auth_style: editing.auth_style,
        base_url: editing.endpoint || undefined,
        secret: secretToSend,
        label: editing.name || undefined,
        extras: {
          name: editing.name,
          model: trimmedModel,
          context_window: String(editing.context_window),
          temperature: String(editing.temperature),
          supports_images: editing.supports_images ? "true" : "false",
          enabled: editing.enabled ? "true" : "false",
          ...(isEnvironmentBacked
            ? {
                auth_source: "environment",
                env_var: envVar,
              }
            : {}),
          // Inherit the operator's timeout preset (set in the
          // Runtime tab) so this provider gets the same response
          // budget as everything else.
          timeout_secs: String(loadTimeoutPreset()),
        },
      });
      publishUxiDebugEvent("ordo.provider", "provider_saved", "Provider credential saved.", {
        provider: trimmedService,
        auth_style: editing.auth_style,
        secret_source: isEnvironmentBacked ? "environment" : "vault",
      });
      setToast(`${editing.name || editing.service}: credential saved.`);
      // Promote to default if no default yet.
      if (!defaultId) setDefaultId(editing.service);
      setEditing(null);
      setSaveError(null);
      await refresh();
    } catch (err: unknown) {
      const msg = err instanceof Error ? err.message : String(err);
      // Two surfaces for the failure: a sticky inline error in the
      // modal so the operator can fix the form, and a toast for the
      // surface-level history. Also log to console so DevTools shows
      // the full error chain (network, status, body).
      console.error("[Cloud] save failed:", err);
      setSaveError(msg);
      setToast(`save failed: ${msg}`);
    } finally {
      setBusy(null);
    }
  };

  const removeCred = async (service: string) => {
    if (!confirm(`Remove credential for "${service}"?`)) return;
    setBusy(service);
    setToast(null);
    try {
      await deleteCloudCredential(service);
      publishUxiDebugEvent("ordo.provider", "provider_deleted", "Provider credential deleted.", {
        provider: service,
      });
      setToast(`${service}: credential removed.`);
      if (defaultId === service) setDefaultId(null);
      await refresh();
    } catch (err: unknown) {
      setToast(`delete failed: ${err instanceof Error ? err.message : String(err)}`);
    } finally {
      setBusy(null);
    }
  };

  // Live "Test" — pings the appropriate cloud capability with a tiny
  // request and reports latency + first chars of response.
  //
  // Capability dispatch is provider-shape based, not service-name based:
  // anthropic-style credentials route to `cloud.anthropic.messages`;
  // every other auth style (bearer / api_key_header / api_key_query)
  // is OpenAI-shaped and routes to `cloud.openai.chat`. This works for
  // OpenAI, Ollama, LM Studio, OpenRouter, Groq, Moonshot, Qwen, Azure,
  // Bedrock, and "OpenAI Compatible" without baking provider names in.
  //
  // Runtime auto-injects `extras.model` from the credential when the
  // request omits it (see ordo-mcp-host::cloud_service_call), so the
  // local Ollama call routes to whichever model the operator has loaded
  // instead of a phantom default like `gpt-4o-mini`.
  const toggleCred = async (credential: CloudCredentialRow) => {
    const nextEnabled = !credentialIsEnabled(credential);
    setBusy(`toggle-${credential.service}`);
    setToast(null);
    try {
      await upsertCloudCredential({
        service: credential.service,
        extras: {
          ...(credential.extras ?? {}),
          enabled: nextEnabled ? "true" : "false",
        },
      });
      publishUxiDebugEvent(
        "ordo.provider",
        nextEnabled ? "provider_enabled" : "provider_paused",
        nextEnabled ? "Provider enabled." : "Provider paused.",
        { provider: credential.service },
      );
      if (!nextEnabled && defaultId === credential.service) setDefaultId(null);
      setToast(`${credential.label ?? credential.service}: ${nextEnabled ? "enabled" : "paused"}.`);
      await refresh();
    } catch (err: unknown) {
      setToast(`toggle failed: ${err instanceof Error ? err.message : String(err)}`);
    } finally {
      setBusy(null);
    }
  };

  const discoverProviderModels = async (credential: CloudCredentialRow) => {
    const current = modelDiscovery[credential.service];
    if (current && current.status !== "loading") {
      setModelDiscovery((prev) => ({
        ...prev,
        [credential.service]: { ...current, open: !current.open },
      }));
      return;
    }
    setModelDiscovery((prev) => ({
      ...prev,
      [credential.service]: { status: "loading", open: true },
    }));
    const t0 = performance.now();
    try {
      const out = await invokeTool("cloud.credentials.models", {
        service: credential.service,
      });
      const body = out as { ok?: boolean; models?: string[]; error?: string; count?: number };
      const ms = Math.round(performance.now() - t0);
      if (body.ok && Array.isArray(body.models)) {
        setModelDiscovery((prev) => ({
          ...prev,
          [credential.service]: {
            status: "ready",
            models: body.models ?? [],
            open: true,
          },
        }));
        publishUxiDebugEvent("ordo.provider", "provider_models_discovered", "Provider model discovery succeeded.", {
          provider: credential.service,
          model_count: body.models.length,
          elapsed_ms: ms,
        });
      } else {
        const errorMessage = body.error || "model discovery failed";
        setModelDiscovery((prev) => ({
          ...prev,
          [credential.service]: {
            status: "failed",
            error: errorMessage,
            models: [],
            open: true,
          },
        }));
        publishUxiDebugEvent("ordo.provider", "provider_model_discovery_failed", "Provider model discovery failed.", {
          provider: credential.service,
          elapsed_ms: ms,
          error: errorMessage,
        }, "WARN");
      }
    } catch (err: unknown) {
      const message = err instanceof Error ? err.message : String(err);
      setModelDiscovery((prev) => ({
        ...prev,
        [credential.service]: {
          status: "failed",
          error: message,
          models: [],
          open: true,
        },
      }));
      publishUxiDebugEvent("ordo.provider", "provider_model_discovery_failed", "Provider model discovery failed.", {
        provider: credential.service,
        error: message,
      }, "WARN");
    }
  };

  const setModelForCredential = async (credential: CloudCredentialRow, model: string) => {
    const nextModel = model.trim();
    if (!nextModel) return;
    setBusy(`model-${credential.service}`);
    try {
      await upsertCloudCredential({
        service: credential.service,
        extras: {
          ...(credential.extras ?? {}),
          model: nextModel,
        },
      });
      publishUxiDebugEvent("ordo.provider", "provider_model_selected", "Provider model selected.", {
        provider: credential.service,
        model: nextModel,
      });
      setToast(`${credential.label ?? credential.service}: model set to ${nextModel}.`);
      await refresh();
    } catch (err: unknown) {
      const message = err instanceof Error ? err.message : String(err);
      setToast(`model update failed: ${message}`);
      publishUxiDebugEvent("ordo.provider", "provider_model_select_failed", "Provider model select failed.", {
        provider: credential.service,
        model: nextModel,
        error: message,
      }, "ERROR");
    } finally {
      setBusy(null);
    }
  };

  const runTest = async (c: CloudCredentialRow) => {
    const isOllamaCloudApi = c.service === "ollama-cloud-api";
    const cap =
      isOllamaCloudApi
        ? "cloud.credentials.models"
        : c.auth_style === "anthropic"
        ? "cloud.anthropic.messages"
        : "cloud.openai.chat";
    // Anthropic requires `max_tokens`; OpenAI-shape accepts it as a
    // budget hint. 256 leaves room for a thinking-model trace before
    // the first content token (qwen, deepseek-r1, …).
    const args = isOllamaCloudApi
      ? { service: c.service }
      : {
          messages: [{ role: "user", content: "ping" }],
          max_tokens: 256,
        };
    setTesting(c.service);
    const t0 = performance.now();
    try {
      const out = await invokeTool(cap, args);
      const ms = Math.round(performance.now() - t0);
      publishUxiDebugEvent("ordo.provider", "provider_test_succeeded", "Provider test succeeded.", {
        provider: c.service,
        capability: cap,
        elapsed_ms: ms,
      });
      setTestResult((r) => ({
        ...r,
        [c.service]: {
          ok: true,
          ms,
          body: JSON.stringify(out, null, 2).slice(0, 2000),
        },
      }));
    } catch (err: unknown) {
      const ms = Math.round(performance.now() - t0);
      // Surface the runtime's actual error body when available — the
      // ApiError carries the parsed JSON response, which usually has
      // the real reason ("missing field X", "model Y not loaded", …).
      // Falling back to err.message just shows "POST … → 500" which is
      // useless for diagnosing the root cause.
      let detail: string;
      if (err instanceof ApiError) {
        const body =
          typeof err.body === "string"
            ? err.body
            : err.body !== undefined
            ? JSON.stringify(err.body, null, 2)
            : "";
        detail = body
          ? `${err.message}\n\n${body.slice(0, 2000)}`
          : err.message;
      } else if (err instanceof Error) {
        detail = err.message;
      } else {
        detail = String(err);
      }
      console.error("[Cloud] test failed:", err);
      publishUxiDebugEvent("ordo.provider", "provider_test_failed", "Provider test failed.", {
        provider: c.service,
        capability: cap,
        elapsed_ms: ms,
        error: detail.slice(0, 500),
      }, "ERROR");
      setTestResult((r) => ({
        ...r,
        [c.service]: { ok: false, ms, body: detail },
      }));
    } finally {
      setTesting(null);
    }
  };

  const orderedCreds = (creds ?? []).slice().sort((a, b) => {
    if (a.service === defaultId) return -1;
    if (b.service === defaultId) return 1;
    return a.service.localeCompare(b.service);
  });
  const enabledCreds = orderedCreds.filter(credentialIsEnabled);
  const openaiTemplate = findTemplate("openai");
  const openaiCredential = orderedCreds.find((c) => c.service === "openai");

  return (
    <div className="h-full flex flex-col gap-4 overflow-auto pb-4">
      <SectionHeader
        icon={<Cloud size={22} />}
        title="Provider"
        sub="OpenAI API is the default. Custom endpoints are added only when you choose to configure one."
        trailing={
          <div className="flex items-center gap-3">
            <Field label="" hint={undefined}>
              <div className="flex items-center gap-2">
                <Mono size={11} upper track="0.18em" color={UI.textMuted}>
                  default
                </Mono>
                <div style={{ minWidth: 200 }}>
                  <Select
                    value={defaultId ?? ""}
                    onChange={(v) => setDefaultId(v || null)}
                    options={[
                      { value: "", label: "(none)" },
                      ...enabledCreds.map((c) => {
                        const t = findTemplate(c.service);
                        return {
                          value: c.service,
                          label: ((c.extras ?? {}) as { name?: string }).name ?? t?.label ?? c.service,
                        };
                      }),
                    ]}
                  />
                </div>
              </div>
            </Field>
            <Button onClick={openCustom} variant="primary" size="md">
              <span style={{ display: "inline-flex", alignItems: "center", gap: 4 }}>
                <Plus size={13} strokeWidth={2.5} /> Customize API
              </span>
            </Button>
          </div>
        }
      />

      {error && <Alert variant="danger">{error}</Alert>}
      {toast && <Alert variant="success">{toast}</Alert>}

      <Card padded={false}>
        <div style={{ padding: "18px 20px" }}>
          <div className="flex items-start justify-between gap-4 flex-wrap">
            <div>
              <Mono size={11} upper track="0.18em" color={UI.textMuted}>
                ollama and local models
              </Mono>
              <div style={{ marginTop: 5, maxWidth: 760 }}>
                <Mono size={11} color={UI.textMuted}>
                  Choose how Ollama connects, then detect models and save the selected one as a provider profile. LM Studio remains local-only.
                </Mono>
              </div>
            </div>
            <Badge variant="info">single Ollama setup</Badge>
          </div>
          <div className="grid gap-3" style={{ gridTemplateColumns: "repeat(auto-fit, minmax(320px, 1fr))", marginTop: 16 }}>
            {(() => {
              const mode = ollamaConnectionMode;
              const probeKey = mode === "signin" ? "ollama-cloud" : "ollama";
              const templateId =
                mode === "signin" ? "ollama-cloud" : mode === "api-key" ? "ollama-cloud-api" : "ollama";
              const template = findTemplate(templateId);
              const probe = localProbe[probeKey];
              const rawModelOptions = probe?.models ?? [];
              const modelOptions =
                mode === "signin"
                  ? cloudOllamaModels(rawModelOptions)
                  : mode === "local"
                  ? localOllamaModels(rawModelOptions)
                  : [];
              const preferredModel = localSelectedModel[probeKey] ?? "";
              const selected = modelOptions.includes(preferredModel)
                ? preferredModel
                : mode === "signin"
                ? pickOllamaCloudModel(modelOptions)
                : modelOptions.length
                ? pickChatModel(modelOptions) ?? modelOptions[0]
                : preferredModel;
              const probeBusy = busy === `probe-${probeKey}`;
              const connectBusy = mode === "signin" ? "connect-ollama-cloud" : "connect-ollama";
              const statusLabel =
                mode === "api-key"
                  ? "env key"
                  : probe?.reachable
                  ? mode === "signin"
                    ? `${modelOptions.length} cloud`
                    : `${modelOptions.length} local`
                  : "not probed";
              const statusVariant =
                mode === "api-key" ? "info" : probe?.reachable ? "success" : "neutral";
              return (
                <div
                  key="ollama-family"
                  style={{
                    border: `1px solid ${UI.cardBorder}`,
                    background: UI.cardBgRaised,
                    borderRadius: 10,
                    padding: 14,
                  }}
                >
                  <div className="flex items-start justify-between gap-3">
                    <div className="flex items-start gap-3" style={{ minWidth: 0 }}>
                      <BrandMark name="Ollama" color={UI.primary} size={32} />
                      <div style={{ minWidth: 0 }}>
                        <Mono size={13} color={UI.parchment} weight={700}>Ollama</Mono>
                        <div style={{ marginTop: 4 }}>
                          <Mono size={10} color={UI.textMuted}>
                            {mode === "local"
                              ? "Local daemon at http://localhost:11434/v1"
                              : mode === "signin"
                              ? "Local Ollama after `ollama signin`; no API key stored in Ordo"
                              : "Direct cloud API using OLLAMA_API_KEY from the local environment"}
                          </Mono>
                        </div>
                      </div>
                    </div>
                    <Badge variant={statusVariant}>{statusLabel}</Badge>
                  </div>
                  <div style={{ marginTop: 12 }}>
                    <Field label="Connection">
                      <Select
                        value={ollamaConnectionMode}
                        onChange={(next) => setOllamaConnectionMode(next)}
                        options={[
                          { value: "local", label: "Local" },
                          { value: "signin", label: "Local Sign-In" },
                          { value: "api-key", label: "API Key" },
                        ]}
                      />
                    </Field>
                  </div>
                  {mode !== "api-key" && probe && !probe.reachable && (
                    <div style={{ marginTop: 10 }}>
                      <Alert variant="warn">{probe.error || "Local Ollama is not reachable."}</Alert>
                    </div>
                  )}
                  {mode !== "api-key" && probe?.reachable && modelOptions.length > 0 && (
                    <div style={{ marginTop: 12 }}>
                      <Select
                        value={selected}
                        onChange={(model) =>
                          setLocalSelectedModel((prev) => ({ ...prev, [probeKey]: model }))
                        }
                        options={modelOptions.map((model) => ({ value: model, label: model }))}
                      />
                    </div>
                  )}
                  {mode === "signin" && (!probe?.reachable || modelOptions.length === 0) && (
                    <div style={{ marginTop: 12 }}>
                      <TextInput
                        value={selected}
                        onChange={(model) =>
                          setLocalSelectedModel((prev) => ({ ...prev, [probeKey]: model }))
                        }
                        placeholder="gpt-oss:120b-cloud"
                      />
                    </div>
                  )}
                  {mode === "local" && probe?.reachable && modelOptions.length === 0 && (
                    <div style={{ marginTop: 10 }}>
                      <Alert variant="warn">No local-only Ollama models were listed. Cloud models belong under Local Sign-In.</Alert>
                    </div>
                  )}
                  {mode === "api-key" ? (
                    <div className="flex gap-2 flex-wrap" style={{ marginTop: 12 }}>
                      <Button
                        size="sm"
                        variant="primary"
                        onClick={() => template && void openTemplate(template)}
                        disabled={!template || busy === "ollama-cloud-api"}
                      >
                        Use Ollama API Key
                      </Button>
                      <Button
                        size="sm"
                        onClick={() => {
                          if (template) {
                            setEditing(draftFromTemplate(template));
                            setEditingMode("new");
                          }
                        }}
                      >
                        Manual setup
                      </Button>
                      <Button size="sm" onClick={() => openKeyWizard("ollama-cloud-api")}>
                        Install OLLAMA_API_KEY
                      </Button>
                    </div>
                  ) : (
                    <div className="flex gap-2 flex-wrap" style={{ marginTop: 12 }}>
                      <Button
                        size="sm"
                        onClick={() =>
                          mode === "signin" ? void probeOllamaCloudModels() : void probeLocalProvider("ollama")
                        }
                        disabled={probeBusy}
                      >
                        {probeBusy ? "Detecting..." : "Detect models"}
                      </Button>
                      <Button
                        size="sm"
                        variant="primary"
                        onClick={() =>
                          mode === "signin"
                            ? void connectOllamaCloudModel(selected)
                            : void connectLocalModel("ollama", selected)
                        }
                        disabled={busy === connectBusy || !selected.trim()}
                      >
                        {selected ? `Connect ${selected}` : "Connect"}
                      </Button>
                      <Button
                        size="sm"
                        onClick={() => {
                          if (template) {
                            setEditing(draftFromTemplate(template));
                            setEditingMode("new");
                          }
                        }}
                      >
                        Manual setup
                      </Button>
                    </div>
                  )}
                </div>
              );
            })()}
            {(() => {
              const probe = localProbe.lmstudio;
              const modelOptions = probe?.models ?? [];
              const preferredModel = localSelectedModel.lmstudio ?? "";
              const selected = modelOptions.includes(preferredModel)
                ? preferredModel
                : modelOptions.length
                ? pickChatModel(modelOptions) ?? modelOptions[0]
                : "";
              return (
                <div
                  key="lmstudio"
                  style={{
                    border: `1px solid ${UI.cardBorder}`,
                    background: UI.cardBgRaised,
                    borderRadius: 10,
                    padding: 14,
                  }}
                >
                  <div className="flex items-start justify-between gap-3">
                    <div className="flex items-start gap-3" style={{ minWidth: 0 }}>
                      <BrandMark name="LM Studio Local" color={UI.primary} size={32} />
                      <div style={{ minWidth: 0 }}>
                        <Mono size={13} color={UI.parchment} weight={700}>LM Studio Local</Mono>
                        <div style={{ marginTop: 4 }}>
                          <Mono size={10} color={UI.textMuted}>http://localhost:1234/v1</Mono>
                        </div>
                      </div>
                    </div>
                    <Badge variant={probe?.reachable ? "success" : "neutral"}>
                      {probe?.reachable ? `${modelOptions.length} models` : "not probed"}
                    </Badge>
                  </div>
                  {probe && !probe.reachable && (
                    <div style={{ marginTop: 10 }}>
                      <Alert variant="warn">{probe.error || "LM Studio is not reachable."}</Alert>
                    </div>
                  )}
                  {probe?.reachable && modelOptions.length > 0 && (
                    <div style={{ marginTop: 12 }}>
                      <Select
                        value={selected}
                        onChange={(model) =>
                          setLocalSelectedModel((prev) => ({ ...prev, lmstudio: model }))
                        }
                        options={modelOptions.map((model) => ({ value: model, label: model }))}
                      />
                    </div>
                  )}
                  <div className="flex gap-2 flex-wrap" style={{ marginTop: 12 }}>
                    <Button
                      size="sm"
                      onClick={() => void probeLocalProvider("lmstudio")}
                      disabled={busy === "probe-lmstudio"}
                    >
                      {busy === "probe-lmstudio" ? "Detecting..." : "Detect models"}
                    </Button>
                    <Button
                      size="sm"
                      variant="primary"
                      onClick={() => void connectLocalModel("lmstudio", selected)}
                      disabled={busy === "connect-lmstudio" || !selected.trim()}
                    >
                      {selected ? `Connect ${selected}` : "Connect"}
                    </Button>
                    <Button
                      size="sm"
                      onClick={() => {
                        const template = findTemplate("lmstudio");
                        if (template) {
                          setEditing(draftFromTemplate(template));
                          setEditingMode("new");
                        }
                      }}
                    >
                      Manual setup
                    </Button>
                  </div>
                </div>
              );
            })()}
          </div>
        </div>
      </Card>

      <Card padded={false}>
        <div style={{ padding: "18px 20px" }}>
          <div className="flex items-start justify-between gap-4">
            <div className="flex items-start gap-3">
              <BrandMark name="OpenAI API" color={UI.primary} size={34} />
              <div>
                <div style={{ fontFamily: FRAUNCES, fontSize: 18, fontWeight: 650, color: UI.parchment }}>
                  OpenAI API
                </div>
                <div style={{ marginTop: 4 }}>
                  <Mono size={11} color={UI.textMuted}>
                    Uses `OPENAI_API_KEY` from the environment that launches Ordo.
                  </Mono>
                </div>
              </div>
            </div>
            <div className="flex items-center gap-2">
              {openaiCredential ? (
                <Button onClick={() => openRotate(openaiCredential)} size="sm">
                  Edit
                </Button>
              ) : (
                <Button
                  onClick={() => openaiTemplate && void openTemplate(openaiTemplate)}
                  disabled={!openaiTemplate || busy === "openai"}
                  variant="primary"
                  size="sm"
                >
                  Use OpenAI API
                </Button>
              )}
              <Button onClick={openCustom} size="sm">
                <Plus size={13} /> Customize API
              </Button>
              <Button onClick={() => openKeyWizard("openai")} size="sm">
                Install API Key
              </Button>
            </div>
          </div>
        </div>
      </Card>

      <div className="space-y-2">
        <Card padded={false}>
          <div className="flex items-center justify-between gap-3" style={{ padding: "14px 16px" }}>
            <div>
              <Mono size={11} upper track="0.18em" color={UI.textMuted}>
                configured providers
              </Mono>
              <div style={{ marginTop: 3 }}>
                <Mono size={10} color={UI.textDim}>
                  Customized APIs appear here after setup and can be selected as the default provider.
                </Mono>
              </div>
            </div>
            <div style={{ minWidth: 240 }}>
              <Select
                value={defaultId ?? ""}
                onChange={(v) => setDefaultId(v || null)}
                options={[
                  { value: "", label: "(none)" },
                  ...enabledCreds.map((c) => {
                    const t = findTemplate(c.service);
                    return {
                      value: c.service,
                      label: ((c.extras ?? {}) as { name?: string }).name ?? t?.label ?? c.service,
                    };
                  }),
                ]}
              />
            </div>
          </div>
        </Card>
        {orderedCreds.map((c) => {
          const t = findTemplate(c.service);
          const extras = (c.extras ?? {}) as { name?: string; model?: string; auth_source?: string; env_var?: string };
          const tr = testResult[c.service];
          const discovery = modelDiscovery[c.service];
          const isEnabled = credentialIsEnabled(c);
          const providerMeta = [
            isEnabled ? "active" : "paused",
            extras.model ?? "provider model pending",
            c.base_url ?? c.endpoint ?? "",
            extras.auth_source === "environment"
              ? `env: ${extras.env_var ?? (t ? defaultEnvVarForProvider(t) : "OPENAI_API_KEY")}`
              : "",
          ].filter(Boolean).join(" - ");
          return (
            <div key={c.service}>
              <ConfiguredRow
                selected={defaultId === c.service}
                onSelect={() => isEnabled && setDefaultId(c.service)}
                icon={<BrandMark name={extras.name ?? t?.label ?? c.service} color={UI.primary} size={28} />}
                name={extras.name ?? t?.label ?? c.service}
                defaultBadge={defaultId === c.service}
                subtitle={<span>{providerMeta}</span>}
                actions={
                  <>
                    <Button
                      onClick={() => void toggleCred(c)}
                      disabled={busy === `toggle-${c.service}`}
                      size="sm"
                    >
                      {isEnabled ? "Pause" : "Enable"}
                    </Button>
                    <Button
                      onClick={() => void runTest(c)}
                      disabled={!isEnabled || testing === c.service || busy === c.service}
                      size="sm"
                    >
                      {testing === c.service ? "testing…" : "Test"}
                    </Button>
                    <Button
                      onClick={() => void discoverProviderModels(c)}
                      disabled={!isEnabled || discovery?.status === "loading" || busy === c.service}
                      size="sm"
                    >
                      {discovery?.status === "loading" ? "Discovering..." : "Discover Models"}
                    </Button>
                    <Button onClick={() => openRotate(c)} disabled={!!busy} size="sm">
                      Edit
                    </Button>
                    <Button onClick={() => void removeCred(c.service)} disabled={!!busy} variant="danger" size="sm">
                      <Trash2 size={12} />
                    </Button>
                  </>
                }
              />
              {discovery?.open && (
                <div style={{ marginTop: 6 }}>
                  <Card>
                    <div className="flex items-start justify-between gap-3">
                      <div>
                        <Mono size={11} upper track="0.16em" color={UI.textMuted}>
                          model discovery
                        </Mono>
                        <div style={{ marginTop: 4 }}>
                          <Mono size={11} color={UI.textDim}>
                            Choose a discovered model to save it to this provider profile.
                          </Mono>
                        </div>
                      </div>
                      <Badge
                        variant={
                          discovery.status === "ready"
                            ? "success"
                            : discovery.status === "failed"
                              ? "danger"
                              : "neutral"
                        }
                      >
                        {discovery.status === "ready"
                          ? `${discovery.models?.length ?? 0} models`
                          : discovery.status}
                      </Badge>
                    </div>
                    {discovery.status === "failed" && (
                      <div style={{ marginTop: 10 }}>
                        <Alert variant="warn">{discovery.error || "Model discovery failed."}</Alert>
                      </div>
                    )}
                    {discovery.status === "ready" && (discovery.models?.length ?? 0) > 0 && (
                      <div className="grid gap-2" style={{ marginTop: 12, gridTemplateColumns: "minmax(0, 1fr) auto" }}>
                        <Select
                          value={extras.model ?? ""}
                          onChange={(model) => void setModelForCredential(c, model)}
                          options={(discovery.models ?? []).map((model) => ({
                            value: model,
                            label: model,
                          }))}
                        />
                        <Button
                          size="sm"
                          onClick={() => void discoverProviderModels(c)}
                        >
                          Refresh
                        </Button>
                      </div>
                    )}
                    {discovery.status === "ready" && (discovery.models?.length ?? 0) === 0 && (
                      <div style={{ marginTop: 10 }}>
                        <Alert variant="warn">Provider responded, but returned no models.</Alert>
                      </div>
                    )}
                  </Card>
                </div>
              )}
              {tr && (
                <div style={{ marginTop: 6 }}>
                  <Alert variant={tr.ok ? "success" : "danger"}>
                    <span style={{ fontWeight: 600 }}>
                      {tr.ok ? "test ok" : "test failed"}
                    </span>{" "}
                    · {tr.ms} ms
                    {tr.body && (
                      <pre
                        style={{
                          marginTop: 6,
                          fontFamily: MONO,
                          fontSize: 10,
                          color: tr.ok ? UI.jade : UI.red,
                          whiteSpace: "pre-wrap",
                          wordBreak: "break-word",
                          maxHeight: 160,
                          overflow: "auto",
                        }}
                      >
                        {tr.body}
                      </pre>
                    )}
                  </Alert>
                </div>
              )}
            </div>
          );
        })}
        {orderedCreds.length === 0 && (
          <Card>
            <div style={{ textAlign: "center", padding: "16px 0" }}>
              <Serif size={14} italic color={UI.textMuted}>
                No providers configured. Use OpenAI API or customize an endpoint.
              </Serif>
            </div>
          </Card>
        )}
      </div>

      <ProviderConfigureModal
        editing={editing}
        mode={editingMode}
        onChange={setEditing}
        onClose={() => {
          setEditing(null);
          setSaveError(null);
        }}
        onSave={() => void saveDraft()}
        busy={!!busy}
        error={saveError}
      />
      <ApiKeyInstallWizard
        draft={keyWizard}
        busy={busy === "api-key-wizard"}
        error={keyWizardError}
        result={keyWizardResult}
        onChange={setKeyWizard}
        onInstall={() => void installKeyWizard()}
        onClose={() => {
          setKeyWizard(null);
          setKeyWizardError(null);
          setKeyWizardResult(null);
        }}
      />
    </div>
  );
};

const ApiKeyInstallWizard = ({
  draft,
  busy,
  error,
  result,
  onChange,
  onInstall,
  onClose,
}: {
  draft: ApiKeyWizardDraft | null;
  busy: boolean;
  error: string | null;
  result: LocalApiKeyInstallResult | null;
  onChange: (draft: ApiKeyWizardDraft | null) => void;
  onInstall: () => void;
  onClose: () => void;
}) => {
  if (!draft) return null;
  const detectedPlatform = detectKeyInstallPlatform();
  const installHint =
    detectedPlatform === "Windows"
      ? "Windows user environment plus Ordo local env file"
      : detectedPlatform === "Apple"
        ? "Apple local Ordo env file under the user config directory"
        : detectedPlatform === "Linux"
          ? "Linux local Ordo env file under the user config directory"
          : "Ordo local env file under the detected user config directory";
  const canInstall =
    !busy &&
    draft.env_var.trim().length > 0 &&
    draft.api_key.trim().length > 0 &&
    draft.endpoint.trim().length > 0 &&
    draft.model.trim().length > 0;
  const set = (patch: Partial<ApiKeyWizardDraft>) => onChange({ ...draft, ...patch });
  const choosePreset = (id: string) => {
    const next = API_KEY_WIZARD_PRESETS.find((preset) => preset.id === id) ?? API_KEY_WIZARD_PRESETS[0];
    onChange({ ...apiKeyWizardDraft(next), api_key: draft.api_key });
  };
  return (
    <Modal
      open={draft !== null}
      onClose={onClose}
      title="Install API Key"
      sub="Install a local key and create the matching provider profile."
      width={620}
      footer={
        <>
          <Button onClick={onClose} disabled={busy}>
            Close
          </Button>
          <Button onClick={onInstall} disabled={!canInstall} variant="primary">
            {busy ? "Installing..." : "Install Locally"}
          </Button>
        </>
      }
    >
      <div className="space-y-4">
        {error && <Alert variant="danger">{error}</Alert>}
        {result && (
          <Alert variant="success">
            Installed for {result.platform} via {result.env_var}. Restart Ordo after switching external runtimes.
          </Alert>
        )}
        {!result && (
          <Alert>
            Detected {detectedPlatform}. Installer will use {installHint}.
          </Alert>
        )}
        <div className="grid grid-cols-2 gap-3">
          <Field label="Provider" required>
            <Select
              value={draft.id}
              onChange={choosePreset}
              options={API_KEY_WIZARD_PRESETS.map((preset) => ({
                value: preset.id,
                label: preset.label,
              }))}
            />
          </Field>
          <Field label="Environment variable" required>
            <TextInput
              value={draft.env_var}
              onChange={(value) => set({ env_var: value.toUpperCase() })}
              placeholder="OPENAI_API_KEY"
            />
          </Field>
        </div>

        <Field label="API Key" required>
          <TextInput
            type="password"
            value={draft.api_key}
            onChange={(value) => set({ api_key: value })}
            placeholder="Paste key"
          />
        </Field>

        <div className="grid grid-cols-2 gap-3">
          <Field label="Profile name" required>
            <TextInput value={draft.label} onChange={(value) => set({ label: value })} />
          </Field>
          <Field label="Model" required>
            <TextInput value={draft.model} onChange={(value) => set({ model: value })} />
          </Field>
        </div>

        <Field label="Base URL" required>
          <TextInput value={draft.endpoint} onChange={(value) => set({ endpoint: value })} />
        </Field>

        {result && (
          <Field label="Local install path">
            <TextInput value={result.local_env_path} onChange={() => undefined} />
          </Field>
        )}
      </div>
    </Modal>
  );
};

const detectKeyInstallPlatform = (): "Windows" | "Linux" | "Apple" | "Unknown" => {
  const nav =
    typeof navigator !== "undefined"
      ? (navigator as Navigator & { userAgentData?: { platform?: string } })
      : null;
  const platform =
    nav
      ? (nav.userAgentData?.platform || nav.platform || nav.userAgent || "")
      : "";
  const normalized = platform.toLowerCase();
  if (normalized.includes("win")) return "Windows";
  if (normalized.includes("mac") || normalized.includes("iphone") || normalized.includes("ipad")) return "Apple";
  if (normalized.includes("linux") || normalized.includes("x11")) return "Linux";
  return "Unknown";
};

const ProviderConfigureModal = ({
  editing,
  mode,
  onChange,
  onClose,
  onSave,
  busy,
  error,
}: {
  editing: CredentialDraft | null;
  mode: "new" | "rotate";
  onChange: (d: CredentialDraft | null) => void;
  onClose: () => void;
  onSave: () => void;
  busy: boolean;
  error: string | null;
}) => {
  if (!editing) return null;
  const set = (patch: Partial<CredentialDraft>) =>
    onChange({ ...editing, ...patch });
  // Save is disabled only when busy or when a hard requirement is
  // missing. For local providers (api_key_required: false), the API
  // key field is optional — don't gate Save on it.
  const usesEnvironmentSecret = editing.secret_source === "environment";
  const envVar = usesEnvironmentSecret ? defaultEnvVarForProvider(editing) : "";
  const needsSecret = editing.api_key_required !== false && !usesEnvironmentSecret && mode === "new";
  const canSave =
    !busy &&
    editing.service.trim().length > 0 &&
    (editing.model ?? "").trim().length > 0 &&
    (!needsSecret || editing.secret.trim().length > 0);
  return (
    <Modal
      open={editing !== null}
      onClose={onClose}
      title={mode === "rotate" ? `Edit ${editing.name}` : "Configure New Provider"}
      sub={mode === "rotate"
        ? "Update endpoint or rotate the API key. Existing extras carry over unless changed."
        : "Add a new LLM provider configuration with API key and model settings."}
      width={620}
      footer={
        <>
          <Button onClick={onClose} disabled={busy}>
            Cancel
          </Button>
          <Button onClick={onSave} disabled={!canSave} variant="primary">
            {busy ? "Saving…" : "Save"}
          </Button>
        </>
      }
    >
      <div className="space-y-4">
        {error && <Alert variant="danger">{error}</Alert>}
        <div className="grid grid-cols-2 gap-3">
          <Field label="API Shape" required>
            <Select
              value={customShapeForDraft(editing)}
              onChange={(v) => set(applyCustomProviderShape(editing, v as CustomProviderShape))}
              options={CUSTOM_PROVIDER_SHAPES}
            />
          </Field>
          <Field label="Provider Name" required>
            <TextInput value={editing.name} onChange={(v) => set({ name: v })} />
          </Field>
        </div>

        <Field label="Base URL" required>
          <TextInput
            value={editing.endpoint}
            onChange={(v) => set({ endpoint: v })}
            placeholder="https://api.example.com/v1"
          />
        </Field>

        {usesEnvironmentSecret ? (
          <>
            <Alert>
              This provider does not store a key in Ordo. The runtime reads the environment variable below when it calls the provider.
            </Alert>
            <Field
              label="Environment variable"
              required
              hint="Set this in the environment that launches Ordo, then restart Ordo so the runtime can read it."
            >
              <TextInput
                value={editing.env_var || envVar}
                onChange={(v) => set({ env_var: v })}
                placeholder={envVar || "OPENAI_API_KEY"}
              />
            </Field>
          </>
        ) : (
          <Field
            label="API Key"
            hint={
              editing.setup_url ? (
                <>
                  Your API key is encrypted and stored locally.{" "}
                  <a
                    href={editing.setup_url}
                    target="_blank"
                    rel="noreferrer"
                    style={{ color: UI.primary, textDecoration: "underline" }}
                  >
                    {editing.name} setup guide
                  </a>
                </>
              ) : (
                "Your API key is encrypted and stored locally."
              )
            }
          >
            <TextInput
              type="password"
              value={editing.secret}
              onChange={(v) => set({ secret: v })}
              placeholder={
                mode === "rotate"
                  ? "leave blank to keep existing key"
                  : "Enter your API key" + (editing.api_key_required === false ? " (optional)" : "")
              }
            />
          </Field>
        )}

        <Field label="Model" required>
          <TextInput value={editing.model} onChange={(v) => set({ model: v })} />
        </Field>

        <div>
          <div
            style={{
              fontFamily: FRAUNCES,
              fontSize: 14,
              fontWeight: 600,
              color: UI.parchment,
              marginBottom: 10,
              paddingBottom: 8,
              borderBottom: `1px solid ${UI.cardBorder}`,
            }}
          >
            Model Configuration
          </div>
          <div className="space-y-3">
            <Checkbox
              checked={editing.enabled}
              onChange={(v) => set({ enabled: v })}
              label="Provider Enabled"
            />
            <Checkbox
              checked={editing.supports_images}
              onChange={(v) => set({ supports_images: v })}
              label="Supports Images"
            />
            <div className="grid grid-cols-2 gap-3">
              <Field label="Context Window Size" hint="Auto-filled based on model">
                <NumberInput
                  value={editing.context_window}
                  onChange={(v) => set({ context_window: v })}
                  min={1024}
                  step={1024}
                />
              </Field>
              <Field label="Temperature (0-2)" hint="Controls response randomness">
                <NumberInput
                  value={editing.temperature}
                  onChange={(v) => set({ temperature: v })}
                  min={0}
                  max={2}
                  step={0.1}
                />
              </Field>
            </div>
          </div>
        </div>
      </div>
    </Modal>
  );
};

const ConnectionsSurface = () => {
  const [types, setTypes] = useState<ConnectionType[] | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState<string | null>(null);
  const [results, setResults] = useState<Record<string, { ok: boolean; body: string }>>({});

  const refresh = async () => {
    try {
      const res = await listConnectionTypes();
      setTypes(res.types);
      setError(null);
    } catch (err: unknown) {
      setError(err instanceof Error ? err.message : String(err));
    }
  };

  useEffect(() => {
    void refresh();
  }, []);

  const runTest = async (id: string) => {
    setBusy(id);
    try {
      const out = await testConnection(id, {});
      setResults((r) => ({ ...r, [id]: { ok: true, body: JSON.stringify(out, null, 2) } }));
    } catch (err: unknown) {
      const msg = err instanceof Error ? err.message : String(err);
      setResults((r) => ({ ...r, [id]: { ok: false, body: msg } }));
    } finally {
      setBusy(null);
    }
  };

  return (
    <div className="h-full flex flex-col gap-4 overflow-auto pb-4">
      <SectionHeader
        icon={<Network size={22} />}
        title="Connections"
        sub="Outbound integrations Ordo can reach. Each type can be tested live to verify wiring."
        trailing={
          <Button onClick={() => void refresh()} size="sm">
            <span style={{ display: "inline-flex", alignItems: "center", gap: 4 }}>
              <RefreshCcw size={11} /> Refresh
            </span>
          </Button>
        }
      />
      {error && <Alert variant="danger">{error}</Alert>}
      <div className="space-y-2">
        {(types ?? []).map((t) => {
          const result = results[t.id];
          const dotColor =
            result === undefined ? UI.slate : result.ok ? UI.jade : UI.red;
          return (
            <div key={t.id}>
              <ConfiguredRow
                icon={<Dot color={dotColor} size={9} />}
                name={t.label}
                subtitle={
                  <span>
                    {t.id}
                    {t.service ? ` · ${t.service}` : ""}
                    {t.description ? ` — ${t.description}` : ""}
                  </span>
                }
                actions={
                  <Button
                    onClick={() => void runTest(t.id)}
                    disabled={!!busy}
                    size="sm"
                  >
                    {busy === t.id ? "Testing…" : "Test"}
                  </Button>
                }
              />
              {result && (
                <div style={{ marginTop: 6 }}>
                  <Alert variant={result.ok ? "success" : "danger"}>
                    <pre
                      style={{
                        fontFamily: MONO,
                        fontSize: 10,
                        color: result.ok ? UI.jade : UI.red,
                        whiteSpace: "pre-wrap",
                        wordBreak: "break-word",
                        maxHeight: 220,
                        overflow: "auto",
                        margin: 0,
                      }}
                    >
                      {result.body}
                    </pre>
                  </Alert>
                </div>
              )}
            </div>
          );
        })}
        {(types ?? []).length === 0 && (
          <Card>
            <div style={{ textAlign: "center", padding: "16px 0" }}>
              <Serif size={14} italic color={UI.textMuted}>
                No connection types registered.
              </Serif>
            </div>
          </Card>
        )}
      </div>
    </div>
  );
};

const DirectoryConnectionsSurface = ({ onOpenDirectoryTab }: { onOpenDirectoryTab: (tab: DirectoryTabId) => void }) => {
  const [types, setTypes] = useState<ConnectionType[] | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState<string | null>(null);
  const [results, setResults] = useState<Record<string, { ok: boolean; body: string }>>({});
  const [filter, setFilter] = useState("");
  const [sort, setSort] = useState<"name" | "service" | "status">("name");

  const refresh = async () => {
    try {
      const res = await listConnectionTypes();
      setTypes(res.types);
      setError(null);
    } catch (err: unknown) {
      setError(err instanceof Error ? err.message : String(err));
    }
  };

  useEffect(() => {
    void refresh();
  }, []);

  const runTest = async (id: string) => {
    setBusy(id);
    try {
      const out = await testConnection(id, {});
      setResults((r) => ({ ...r, [id]: { ok: true, body: JSON.stringify(out, null, 2) } }));
    } catch (err: unknown) {
      const msg = err instanceof Error ? err.message : String(err);
      setResults((r) => ({ ...r, [id]: { ok: false, body: msg } }));
    } finally {
      setBusy(null);
    }
  };

  const visibleTypes = useMemo(() => {
    const q = filter.trim().toLowerCase();
    const list = (types ?? []).filter((type) => {
      if (!q) return true;
      return (
        type.id.toLowerCase().includes(q) ||
        type.label.toLowerCase().includes(q) ||
        (type.service ?? "").toLowerCase().includes(q) ||
        (type.description ?? "").toLowerCase().includes(q)
      );
    });
    return [...list].sort((a, b) => {
      if (sort === "service") return `${a.service ?? ""}.${a.label}`.localeCompare(`${b.service ?? ""}.${b.label}`);
      if (sort === "status") {
        const ar = results[a.id];
        const br = results[b.id];
        const av = ar === undefined ? 1 : ar.ok ? 0 : 2;
        const bv = br === undefined ? 1 : br.ok ? 0 : 2;
        return av - bv || a.label.localeCompare(b.label);
      }
      return a.label.localeCompare(b.label);
    });
  }, [filter, results, sort, types]);

  return (
    <DirectoryFrame
      active="connectors"
      search={filter}
      onSearch={setFilter}
      placeholder="Search connectors..."
      onOpen={onOpenDirectoryTab}
      controls={
        <div className="flex items-center justify-between gap-3 w-full">
          <div className="flex items-center gap-2">
            <DirectoryPill active onClick={() => undefined}>
              Ordo Connectors
            </DirectoryPill>
          </div>
          <div className="flex items-center gap-2">
            <DirectorySelect
              value={sort}
              onChange={setSort}
              options={[
                { value: "name", label: "Sort by name" },
                { value: "service", label: "Sort by service" },
                { value: "status", label: "Sort by status" },
              ]}
            />
            <Button onClick={() => void refresh()} size="sm">
              <span style={{ display: "inline-flex", alignItems: "center", gap: 4 }}>
                <RefreshCcw size={11} /> Refresh
              </span>
            </Button>
          </div>
        </div>
      }
    >
      <div className="space-y-4">
        {error && <Alert variant="danger">{error}</Alert>}
        <Mono size={11} upper track="0.18em" color={UI.textMuted}>
          {types ? `${types.length} connector${types.length === 1 ? "" : "s"} registered` : "loading connectors"}
        </Mono>
        {visibleTypes.length > 0 && (
          <DirectoryGrid>
            {visibleTypes.map((type) => {
              const result = results[type.id];
              const dotColor = result === undefined ? UI.slate : result.ok ? UI.jade : UI.red;
              return (
                <DirectoryCard
                  key={type.id}
                  icon={<Dot color={dotColor} size={10} />}
                  title={type.label}
                  source={`${type.id}${type.service ? ` / ${type.service}` : ""}`}
                  description={
                    <>
                      {type.description || "Connector type registered with Ordo."}
                      {result && (
                        <pre
                          style={{
                            fontFamily: MONO,
                            fontSize: 10,
                            color: result.ok ? UI.jade : UI.red,
                            whiteSpace: "pre-wrap",
                            wordBreak: "break-word",
                            maxHeight: 96,
                            overflow: "auto",
                            margin: "8px 0 0",
                          }}
                        >
                          {result.body}
                        </pre>
                      )}
                    </>
                  }
                  actions={
                    <Button onClick={() => void runTest(type.id)} disabled={!!busy} size="sm">
                      {busy === type.id ? "Testing..." : "Test"}
                    </Button>
                  }
                  badges={
                    <>
                      <Badge variant={result === undefined ? "neutral" : result.ok ? "success" : "danger"}>
                        {result === undefined ? "untested" : result.ok ? "online" : "failed"}
                      </Badge>
                      {type.fields && <Badge variant="info">{type.fields.length} fields</Badge>}
                    </>
                  }
                />
              );
            })}
          </DirectoryGrid>
        )}
        {types !== null && visibleTypes.length === 0 && (
          <DirectoryEmpty
            title="No matching connectors"
            sub={filter ? "Try a different search." : "No connector types are registered."}
          />
        )}
        {types === null && <DirectoryEmpty title="Loading connectors" sub="Reading connection types from the runtime." />}
      </div>
    </DirectoryFrame>
  );
};

const PluginsSurface = () => {
  const [plugs, setPlugs] = useState<PluginStatus[] | null>(null);
  const [error, setError] = useState<string | null>(null);

  const refresh = async () => {
    try {
      const res = await listPlugins();
      setPlugs(res.plugins);
      setError(null);
    } catch (err: unknown) {
      setError(err instanceof Error ? err.message : String(err));
    }
  };

  useEffect(() => {
    void refresh();
  }, []);

  return (
    <div className="h-full flex flex-col gap-4 overflow-auto pb-4">
      <SectionHeader
        icon={<Plug size={22} />}
        title="Plugins"
        sub="External providers, stdio-bridged. Each plugin advertises capabilities."
        trailing={
          <Button onClick={() => void refresh()} size="sm">
            <span style={{ display: "inline-flex", alignItems: "center", gap: 4 }}>
              <RefreshCcw size={11} /> Refresh
            </span>
          </Button>
        }
      />
      {error && <Alert variant="danger">{error}</Alert>}
      <div className="space-y-2">
        {(plugs ?? []).map((p) => {
          const variant: "success" | "warn" | "danger" | "neutral" =
            p.state === "Active" || p.state === "active"
              ? "success"
              : p.state === "Disabled" || p.state === "disabled"
              ? "neutral"
              : "danger";
          return (
            <div key={p.name}>
              <ConfiguredRow
                icon={<Plug size={14} />}
                name={p.name}
                subtitle={
                  <span>
                    v{p.version} · {p.tool_count} tool{p.tool_count === 1 ? "" : "s"}
                    {p.description ? ` — ${p.description}` : ""}
                  </span>
                }
                rightBadge={<Badge variant={variant}>{p.state}</Badge>}
                actions={
                  <div className="flex items-center gap-1.5 flex-wrap">
                    {p.expected_lanes.map((lane) => (
                      <Badge key={lane} variant="warn">
                        {lane}
                      </Badge>
                    ))}
                  </div>
                }
              />
              {p.failure && (
                <div style={{ marginTop: 6 }}>
                  <Alert variant="danger">failure: {p.failure}</Alert>
                </div>
              )}
            </div>
          );
        })}
        {(plugs ?? []).length === 0 && (
          <Card>
            <div style={{ textAlign: "center", padding: "16px 0" }}>
              <Serif size={14} italic color={UI.textMuted}>
                No plugins installed. Drop a plugin.json under{" "}
                <code style={{ fontFamily: MONO, color: UI.parchment }}>
                  user-files/plugins/&lt;name&gt;/
                </code>{" "}
                and restart Ordo.
              </Serif>
            </div>
          </Card>
        )}
      </div>
    </div>
  );
};

// ─── MCP — Ordo as MCP server + external MCP server registry ────

interface PluginDraft {
  name: string;
  version: string;
  description: string;
  command: string;
  argsText: string;
  lanesText: string;
  requiredEnvText: string;
  envText: string;
  enabled: boolean;
  coreOverride: boolean;
}

const emptyPluginDraft = (): PluginDraft => ({
  name: "",
  version: "0.1.0",
  description: "",
  command: "",
  argsText: "",
  lanesText: "research.",
  requiredEnvText: "",
  envText: "{}",
  enabled: true,
  coreOverride: false,
});

const pluginDraftFromStatus = (p: PluginStatus): PluginDraft => ({
  name: p.name,
  version: p.version || "0.1.0",
  description: p.description ?? "",
  command: p.command ?? "",
  argsText: (p.args ?? []).join("\n"),
  lanesText: (p.expected_lanes ?? []).join("\n"),
  requiredEnvText: (p.required_env ?? []).join("\n"),
  envText: JSON.stringify(p.env ?? {}, null, 2),
  enabled: p.enabled,
  coreOverride: p.core_override,
});

const splitPluginList = (value: string): string[] =>
  value
    .split(/[\n,]/)
    .map((item) => item.trim())
    .filter(Boolean);

const pluginDraftToManifest = (draft: PluginDraft): PluginManifestDraft => {
  const env = JSON.parse(draft.envText || "{}") as unknown;
  if (typeof env !== "object" || env === null || Array.isArray(env)) {
    throw new Error("env must be a JSON object");
  }
  for (const [key, value] of Object.entries(env)) {
    if (typeof value !== "string") throw new Error(`env.${key} must be a string`);
  }
  return {
    name: draft.name.trim(),
    version: draft.version.trim() || "0.1.0",
    description: draft.description.trim(),
    command: draft.command.trim(),
    args: splitPluginList(draft.argsText),
    expected_lanes: splitPluginList(draft.lanesText),
    required_env: splitPluginList(draft.requiredEnvText),
    env: env as Record<string, string>,
    enabled: draft.enabled,
    core_override: draft.coreOverride,
  };
};

const EnhancedPluginsSurface = ({ onOpenDirectoryTab }: { onOpenDirectoryTab: (tab: DirectoryTabId) => void }) => {
  const [plugs, setPlugs] = useState<PluginStatus[] | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [toast, setToast] = useState<string | null>(null);
  const [busy, setBusy] = useState<string | null>(null);
  const [installOpen, setInstallOpen] = useState(false);
  const [editing, setEditing] = useState<PluginStatus | null>(null);
  const [draft, setDraft] = useState<PluginDraft>(() => emptyPluginDraft());
  const [filter, setFilter] = useState("");
  const [scope, setScope] = useState<"installed" | "user">("installed");
  const [sort, setSort] = useState<"name" | "lane" | "status">("name");

  const refresh = async () => {
    try {
      const res = await listPlugins();
      setPlugs(res.plugins);
      setError(null);
    } catch (err: unknown) {
      setError(err instanceof Error ? err.message : String(err));
    }
  };

  useEffect(() => {
    void refresh();
  }, []);

  const saveInstall = async () => {
    setBusy("install-plugin");
    setToast(null);
    try {
      await installPlugin(pluginDraftToManifest(draft));
      setToast(`plugin installed: ${draft.name.trim()}`);
      setInstallOpen(false);
      await refresh();
    } catch (err: unknown) {
      setToast(`install failed: ${err instanceof Error ? err.message : String(err)}`);
    } finally {
      setBusy(null);
    }
  };

  const saveEdit = async () => {
    if (!editing) return;
    setBusy(`edit:${editing.name}`);
    setToast(null);
    try {
      await updatePlugin(editing.name, pluginDraftToManifest(draft));
      setToast(`plugin updated: ${editing.name}`);
      setEditing(null);
      await refresh();
    } catch (err: unknown) {
      setToast(`update failed: ${err instanceof Error ? err.message : String(err)}`);
    } finally {
      setBusy(null);
    }
  };

  const toggleEnabled = async (plugin: PluginStatus) => {
    const next = !plugin.enabled;
    setBusy(`toggle:${plugin.name}`);
    setToast(null);
    try {
      await setPluginEnabled(plugin.name, next);
      setToast(`${plugin.name} ${next ? "resumed" : "paused"}.`);
      await refresh();
    } catch (err: unknown) {
      setToast(`toggle failed: ${err instanceof Error ? err.message : String(err)}`);
    } finally {
      setBusy(null);
    }
  };

  const removePlugin = async (plugin: PluginStatus) => {
    if (!window.confirm(`Delete plugin "${plugin.name}" from disk?`)) return;
    setBusy(`delete:${plugin.name}`);
    setToast(null);
    try {
      await deletePlugin(plugin.name);
      setToast(`plugin deleted: ${plugin.name}`);
      await refresh();
    } catch (err: unknown) {
      setToast(`delete failed: ${err instanceof Error ? err.message : String(err)}`);
    } finally {
      setBusy(null);
    }
  };

  const openInstall = () => {
    setDraft(emptyPluginDraft());
    setInstallOpen(true);
  };

  const openEdit = (plugin: PluginStatus) => {
    setDraft(pluginDraftFromStatus(plugin));
    setEditing(plugin);
  };

  const visiblePlugins = useMemo(() => {
    const q = filter.trim().toLowerCase();
    const list = (plugs ?? []).filter((plugin) => {
      const userManaged = !plugin.core_override;
      if (scope === "user" && !userManaged) return false;
      if (!q) return true;
      return (
        plugin.name.toLowerCase().includes(q) ||
        plugin.description.toLowerCase().includes(q) ||
        plugin.command.toLowerCase().includes(q) ||
        plugin.expected_lanes.some((lane) => lane.toLowerCase().includes(q))
      );
    });
    return [...list].sort((a, b) => {
      if (sort === "lane") {
        return `${a.expected_lanes[0] ?? ""}.${a.name}`.localeCompare(`${b.expected_lanes[0] ?? ""}.${b.name}`);
      }
      if (sort === "status") {
        const av = a.enabled ? 0 : 1;
        const bv = b.enabled ? 0 : 1;
        return av - bv || a.name.localeCompare(b.name);
      }
      return a.name.localeCompare(b.name);
    });
  }, [filter, plugs, scope, sort]);

  return (
    <div className="h-full flex flex-col gap-4 overflow-auto pb-4">
      <DirectoryFrame
        active="plugins"
        search={filter}
        onSearch={setFilter}
        placeholder="Search plugins..."
        onOpen={onOpenDirectoryTab}
        controls={
          <div className="flex items-center justify-between gap-3 w-full">
            <div className="flex items-center gap-2">
              <DirectoryPill active={scope === "installed"} onClick={() => setScope("installed")}>
                Installed
              </DirectoryPill>
              <DirectoryPill active={scope === "user"} onClick={() => setScope("user")}>
                User Added
              </DirectoryPill>
            </div>
            <div className="flex items-center gap-2">
              <DirectorySelect
                value={sort}
                onChange={setSort}
                options={[
                  { value: "name", label: "Sort by name" },
                  { value: "lane", label: "Sort by lane" },
                  { value: "status", label: "Sort by status" },
                ]}
              />
              <Button onClick={openInstall} size="sm" variant="primary">
                <span style={{ display: "inline-flex", alignItems: "center", gap: 4 }}>
                  <Plus size={11} /> Install plugin
                </span>
              </Button>
              <Button onClick={() => void refresh()} size="sm">
                <span style={{ display: "inline-flex", alignItems: "center", gap: 4 }}>
                  <RefreshCcw size={11} /> Refresh
                </span>
              </Button>
            </div>
          </div>
        }
      >
        <div className="space-y-4">
          {error && <Alert variant="danger">{error}</Alert>}
          {toast && <Alert>{toast}</Alert>}
          <Mono size={11} upper track="0.18em" color={UI.textMuted}>
            {plugs ? `${plugs.length} plugin${plugs.length === 1 ? "" : "s"} installed` : "loading plugins"}
          </Mono>
          {visiblePlugins.length > 0 && (
            <DirectoryGrid>
              {visiblePlugins.map((plugin) => (
                <DirectoryCard
                  key={plugin.name}
                  icon={<Plug size={20} />}
                  title={plugin.name}
                  source={`v${plugin.version} / ${plugin.tool_count} tool${plugin.tool_count === 1 ? "" : "s"}`}
                  description={
                    <>
                      {plugin.description || plugin.command || "Plugin manifest registered with Ordo."}
                      {plugin.failure && (
                        <div style={{ marginTop: 8, color: UI.red }}>
                          failure: {plugin.failure}
                        </div>
                      )}
                    </>
                  }
                  muted={!plugin.enabled}
                  actions={
                    <>
                      <Button
                        size="sm"
                        onClick={() => void toggleEnabled(plugin)}
                        disabled={busy === `toggle:${plugin.name}`}
                        title={plugin.enabled ? "Pause plugin" : "Resume plugin"}
                      >
                        {plugin.enabled ? <Pause size={14} /> : <Play size={14} />}
                      </Button>
                      <Button size="sm" onClick={() => openEdit(plugin)} disabled={busy !== null} title="Edit plugin">
                        <Wrench size={14} />
                      </Button>
                      <Button
                        size="sm"
                        variant="danger"
                        onClick={() => void removePlugin(plugin)}
                        disabled={busy === `delete:${plugin.name}`}
                        title="Delete plugin"
                      >
                        <Trash2 size={14} />
                      </Button>
                    </>
                  }
                  badges={
                    <>
                      <Badge variant={plugin.enabled ? "success" : "neutral"}>
                        {plugin.enabled ? "active" : "paused"}
                      </Badge>
                      {plugin.core_override && <Badge variant="warn">core</Badge>}
                      {plugin.expected_lanes.slice(0, 3).map((lane) => (
                        <Badge key={lane} variant="info">
                          {lane}
                        </Badge>
                      ))}
                    </>
                  }
                />
              ))}
            </DirectoryGrid>
          )}
          {plugs !== null && visiblePlugins.length === 0 && (
            <DirectoryEmpty
              title="No matching plugins"
              sub={filter ? "Try a different search." : "Click Install plugin to add one."}
            />
          )}
          {plugs === null && <DirectoryEmpty title="Loading plugins" sub="Reading plugin manifests from the runtime." />}
        </div>
      </DirectoryFrame>

      <PluginManifestModal
        open={installOpen}
        mode="install"
        draft={draft}
        busy={busy === "install-plugin"}
        onChange={setDraft}
        onClose={() => setInstallOpen(false)}
        onSave={() => void saveInstall()}
      />
      <PluginManifestModal
        open={editing !== null}
        mode="edit"
        draft={draft}
        busy={editing ? busy === `edit:${editing.name}` : false}
        onChange={setDraft}
        onClose={() => setEditing(null)}
        onSave={() => void saveEdit()}
      />
    </div>
  );
};

const PluginManifestModal = ({
  open,
  mode,
  draft,
  busy,
  onChange,
  onClose,
  onSave,
}: {
  open: boolean;
  mode: "install" | "edit";
  draft: PluginDraft;
  busy: boolean;
  onChange: (draft: PluginDraft) => void;
  onClose: () => void;
  onSave: () => void;
}) => {
  const set = (patch: Partial<PluginDraft>) => onChange({ ...draft, ...patch });
  const canSave =
    draft.name.trim().length > 0 &&
    draft.command.trim().length > 0 &&
    splitPluginList(draft.lanesText).length > 0;
  return (
    <Modal
      open={open}
      onClose={onClose}
      title={mode === "install" ? "Install Plugin" : `Edit ${draft.name}`}
      sub="Plugin manifests live under user-files/plugins and stay separate from MCP server manifests."
      width={700}
      footer={
        <>
          <Button onClick={onClose} disabled={busy}>
            Cancel
          </Button>
          <Button onClick={onSave} disabled={busy || !canSave} variant="primary">
            {busy ? "Saving..." : mode === "install" ? "Install" : "Save"}
          </Button>
        </>
      }
    >
      <div className="space-y-4">
        <div className="grid grid-cols-2 gap-3">
          <Field label="Name" required hint="Lowercase id; also becomes the plugin directory name.">
            <TextInput
              value={draft.name}
              onChange={(v) => set({ name: v })}
              placeholder="research-example"
              disabled={mode === "edit"}
              autoFocus={mode === "install"}
            />
          </Field>
          <Field label="Version">
            <TextInput value={draft.version} onChange={(v) => set({ version: v })} placeholder="0.1.0" />
          </Field>
        </div>
        <Field label="Description">
          <Textarea
            value={draft.description}
            onChange={(v) => set({ description: v })}
            rows={2}
            placeholder="Searches a research provider and returns structured results."
          />
        </Field>
        <Field label="Command" required hint="Executable path. Relative commands resolve inside this plugin's folder.">
          <TextInput value={draft.command} onChange={(v) => set({ command: v })} placeholder="plugin.exe" />
        </Field>
        <div className="grid grid-cols-2 gap-3">
          <Field label="Args" hint="One arg per line, or comma-separated.">
            <Textarea value={draft.argsText} onChange={(v) => set({ argsText: v })} rows={3} />
          </Field>
          <Field label="Expected lanes" required hint="One lane prefix per line. Plugin lanes must not start with mcp.">
            <Textarea
              value={draft.lanesText}
              onChange={(v) => set({ lanesText: v })}
              rows={3}
              placeholder={"research.example.\napi.example."}
            />
          </Field>
        </div>
        <div className="grid grid-cols-2 gap-3">
          <Field label="Required env" hint="Environment variable names to forward.">
            <Textarea value={draft.requiredEnvText} onChange={(v) => set({ requiredEnvText: v })} rows={3} />
          </Field>
          <Field label="Env JSON" hint='Plain string values only, for example { "BASE_URL": "https://api.example.com" }.'>
            <Textarea value={draft.envText} onChange={(v) => set({ envText: v })} rows={3} spellCheck={false} />
          </Field>
        </div>
        <div className="flex items-center gap-5">
          <Checkbox checked={draft.enabled} onChange={(v) => set({ enabled: v })} label="Enabled" />
          <Checkbox checked={draft.coreOverride} onChange={(v) => set({ coreOverride: v })} label="Core override" />
        </div>
      </div>
    </Modal>
  );
};

type McpClientId = "claude-code" | "claude-desktop" | "codex" | "gemini-cli" | "cursor";

const MCP_CLIENTS: { id: McpClientId; label: string; command: (binPath: string) => string }[] = [
  {
    id: "claude-code",
    label: "Claude Code",
    command: (bin) => `claude mcp add --transport stdio ordo "${bin}" --scope user`,
  },
  {
    id: "claude-desktop",
    label: "Claude Desktop",
    command: (bin) =>
      `# add to claude_desktop_config.json: { "mcpServers": { "ordo": { "command": "${bin}" } } }`,
  },
  {
    id: "codex",
    label: "Codex",
    command: (bin) => `codex mcp add ordo "${bin}"`,
  },
  {
    id: "gemini-cli",
    label: "Gemini CLI",
    command: (bin) => `gemini config set mcp.ordo.command "${bin}"`,
  },
  {
    id: "cursor",
    label: "Cursor",
    command: (bin) =>
      `# add to ~/.cursor/mcp.json: { "mcpServers": { "ordo": { "command": "${bin}" } } }`,
  },
];

const defaultMcpBinaryPath = () => {
  if (typeof navigator !== "undefined" && navigator.platform.startsWith("Win")) {
    return "target\\release\\ordo-mcp.exe";
  }
  return "target/release/ordo-mcp";
};

const ORDO_MCP_BIN_DEFAULT = defaultMcpBinaryPath();

const McpSurface = () => {
  const [servers, setServers] = useState<McpServer[] | null>(null);
  const [tools, setTools] = useState<CapabilityDescriptor[] | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState<string | null>(null);
  const [toast, setToast] = useState<string | null>(null);
  const [installOpen, setInstallOpen] = useState(false);
  const [installPayload, setInstallPayload] = useState(
    `{\n  "server_id": "example",\n  "module_path": "user-files/mcp/example.wasm"\n}`,
  );
  const [inspectId, setInspectId] = useState<string | null>(null);
  const [inspectBody, setInspectBody] = useState<string>("");
  const [client, setClient] = useState<McpClientId>("claude-code");
  const [toolFilter, setToolFilter] = useState("");
  const [toolsOpen, setToolsOpen] = useState(true);
  // Bin path is read from localStorage so the operator can adjust if
  // their build is somewhere unusual. On mount we ask the runtime to
  // auto-detect (see effect below) and prefill if found — operators
  // shouldn't have to type or paste a known-good path on first run.
  const [binPath, setBinPathRaw] = useState<string>(() => {
    if (typeof window === "undefined") return "";
    return window.localStorage.getItem("ordo:mcp_bin_path") ?? "";
  });
  const [binAutoDetected, setBinAutoDetected] = useState(false);
  const [binDetectChecked, setBinDetectChecked] = useState<string[]>([]);
  const setBinPath = (v: string) => {
    setBinPathRaw(v);
    if (typeof window !== "undefined") window.localStorage.setItem("ordo:mcp_bin_path", v);
  };
  // Browse… — opens the native OS file picker via tauri-plugin-dialog.
  // No-op fallback when running outside a Tauri shell (vite-only dev
  // browser preview): the operator types the path manually.
  const browseForBin = async () => {
    try {
      const mod = await import("@tauri-apps/plugin-dialog");
      const picked = await mod.open({
        multiple: false,
        directory: false,
        title: "Locate ordo-mcp executable",
        defaultPath: binPath || undefined,
        filters:
          typeof navigator !== "undefined" && navigator.platform.startsWith("Win")
            ? [{ name: "Executable", extensions: ["exe"] }]
            : undefined,
      });
      if (typeof picked === "string" && picked.length > 0) {
        setBinPath(picked);
        setBinAutoDetected(false);
      }
    } catch (err) {
      // Browser-only dev preview hits this — Tauri plugin not available.
      setToast(
        `file picker unavailable here: ${err instanceof Error ? err.message : String(err)}. Type the path manually.`,
      );
    }
  };

  const refresh = async () => {
    try {
      const [s, c] = await Promise.all([listMcpServers(), fetchMcpCapabilities()]);
      setServers(s.servers);
      setTools(c.descriptors);
      setError(null);
    } catch (err: unknown) {
      setError(err instanceof Error ? err.message : String(err));
    }
  };

  useEffect(() => {
    void refresh();
  }, []);

  // Auto-detect the ordo-mcp executable on first mount via the runtime's
  // /api/system/find_binary endpoint. The runtime walks paths
  // anchored on its own location (sibling target/release dir, parent
  // directories, $PATH) so it finds the binary in the common case
  // where both the runtime and MCP binaries are built side by side.
  useEffect(() => {
    // Don't override an operator-set value; only fill if blank.
    if (binPath.trim().length > 0) return;
    let cancelled = false;
    (async () => {
      try {
        const res = await findBinary("ordo-mcp");
        if (cancelled) return;
        setBinDetectChecked(res.candidates);
        if (res.found) {
          setBinPath(res.found);
          setBinAutoDetected(true);
        }
      } catch {
        // Silent — operator can still use the path field manually.
      }
    })();
    return () => {
      cancelled = true;
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  const install = async () => {
    setBusy("install");
    setToast(null);
    try {
      const parsed: unknown = JSON.parse(installPayload);
      await installMcpServer(parsed as Record<string, unknown>);
      setToast("server installed.");
      setInstallOpen(false);
      await refresh();
    } catch (err: unknown) {
      setToast(`install failed: ${err instanceof Error ? err.message : String(err)}`);
    } finally {
      setBusy(null);
    }
  };

  const doInspect = async (id: string) => {
    setBusy(id);
    setInspectId(id);
    setInspectBody("loading…");
    try {
      const out = await inspectMcpServer(id);
      setInspectBody(JSON.stringify(out, null, 2));
    } catch (err: unknown) {
      setInspectBody(`error: ${err instanceof Error ? err.message : String(err)}`);
    } finally {
      setBusy(null);
    }
  };

  const doUninstall = async (id: string) => {
    if (!confirm(`Uninstall MCP server "${id}"?`)) return;
    setBusy(id);
    setToast(null);
    try {
      await uninstallMcpServer(id);
      setToast(`${id} uninstalled.`);
      if (inspectId === id) {
        setInspectId(null);
        setInspectBody("");
      }
      await refresh();
    } catch (err: unknown) {
      setToast(`uninstall failed: ${err instanceof Error ? err.message : String(err)}`);
    } finally {
      setBusy(null);
    }
  };

  const filteredTools = (tools ?? []).filter((t) => {
    if (!toolFilter.trim()) return true;
    const q = toolFilter.toLowerCase();
    return t.capability.toLowerCase().includes(q) || t.description.toLowerCase().includes(q);
  });

  const activeClient = MCP_CLIENTS.find((c) => c.id === client) ?? MCP_CLIENTS[0];

  return (
    <div className="h-full flex flex-col gap-4 overflow-auto pb-4">
      {error && <Alert variant="danger">{error}</Alert>}
      {toast && <Alert variant="success">{toast}</Alert>}

      {/* External MCP Servers comes first — the operator-actionable
          surface (install / inspect / uninstall) is what brought them
          to this tab. The Ordo MCP Server config (binary path, copy-
          paste setup commands, tool inventory) sits below as
          reference material. */}
      <SectionHeader
        icon={<Server size={22} />}
        title="External MCP Servers"
        sub="MCP servers Ordo has installed. Quarantined workers, taint tracking on every call."
        trailing={
          <Button onClick={() => setInstallOpen(true)} variant="primary" size="md">
            <span style={{ display: "inline-flex", alignItems: "center", gap: 4 }}>
              <Plus size={13} strokeWidth={2.5} /> Install MCP server
            </span>
          </Button>
        }
      />

      <div className="space-y-2">
        {(servers ?? []).map((s) => {
          const trustVariant: "success" | "warn" | "danger" =
            s.trust_state === "Trusted" || s.trust_state === "trusted"
              ? "success"
              : s.trust_state === "Quarantined" || s.trust_state === "quarantined"
              ? "danger"
              : "warn";
          return (
            <div key={s.server_id}>
              <ConfiguredRow
                icon={<Server size={14} />}
                name={s.server_id}
                subtitle={
                  <span>
                    {s.tool_count} tool{s.tool_count === 1 ? "" : "s"}
                    {s.privilege_tier ? ` · ${s.privilege_tier}` : ""}
                    {s.drift && s.drift !== "none" ? ` · drift: ${s.drift}` : ""}
                  </span>
                }
                rightBadge={<Badge variant={trustVariant}>{s.trust_state}</Badge>}
                actions={
                  <>
                    <Button onClick={() => void doInspect(s.server_id)} size="sm">
                      Inspect
                    </Button>
                    <Button onClick={() => void doUninstall(s.server_id)} variant="danger" size="sm">
                      <Trash2 size={12} />
                    </Button>
                  </>
                }
              />
              {inspectId === s.server_id && (
                <div style={{ marginTop: 6 }}>
                  <Card>
                    <pre
                      style={{
                        fontFamily: MONO,
                        fontSize: 11,
                        color: UI.parchment,
                        whiteSpace: "pre-wrap",
                        wordBreak: "break-word",
                        maxHeight: 320,
                        overflow: "auto",
                        margin: 0,
                      }}
                    >
                      {inspectBody}
                    </pre>
                  </Card>
                </div>
              )}
            </div>
          );
        })}
        {(servers ?? []).length === 0 && (
          <Card>
            <div style={{ textAlign: "center", padding: "16px 0" }}>
              <Serif size={14} italic color={UI.textMuted}>
                No external MCP servers installed yet.
              </Serif>
            </div>
          </Card>
        )}
      </div>

      {/* Ordo MCP Server — config + setup commands + advertised tools.
          Sits below the external servers because this is reference
          material (where ordo-mcp lives, how to wire it into Claude
          Desktop / Claude Code / Codex, what Ordo exposes), while
          the upper section is operator-actionable. */}
      <SectionHeader
        icon={<Server size={22} />}
        title="Ordo MCP Server"
        sub="Connect Ordo to MCP clients like Claude Code, Claude Desktop, and Codex."
      />

      <Card>
        <Field
          label="Server binary path"
          hint={
            binAutoDetected
              ? "Auto-detected next to the running runtime. Click Browse if you'd rather pick a different build."
              : binPath
              ? "Click Browse to pick a different build."
              : binDetectChecked.length > 0
              ? `Auto-detect didn't find a build. Click Browse to locate the ordo-mcp executable (looked under: ${binDetectChecked
                  .slice(0, 3)
                  .join(", ")}${binDetectChecked.length > 3 ? "…" : ""}).`
              : "Click Browse to locate your ordo-mcp executable."
          }
        >
          <div className="flex items-center gap-2">
            <div style={{ flex: 1 }}>
              <TextInput
                value={binPath}
                onChange={(v) => {
                  setBinPath(v);
                  setBinAutoDetected(false);
                }}
                placeholder={ORDO_MCP_BIN_DEFAULT}
              />
            </div>
            <Button onClick={() => void browseForBin()} size="md">
              <span style={{ display: "inline-flex", alignItems: "center", gap: 5 }}>
                <FolderOpen size={13} /> Browse…
              </span>
            </Button>
          </div>
        </Field>
      </Card>

      <Card>
        <div className="flex items-center justify-between mb-3">
          <div>
            <div
              style={{
                fontFamily: FRAUNCES,
                fontSize: 16,
                fontWeight: 600,
                color: UI.parchment,
              }}
            >
              Quick Setup
            </div>
            <div style={{ marginTop: 2 }}>
              <Mono size={11} color={UI.textMuted}>
                Copy and run the command for your tool
              </Mono>
            </div>
          </div>
        </div>
        <div className="mb-3">
          <TabPills
            items={MCP_CLIENTS.map((c) => ({ id: c.id, label: c.label }))}
            active={client}
            onChange={setClient}
          />
        </div>
        <CommandBlock command={activeClient.command(binPath || ORDO_MCP_BIN_DEFAULT)} />
      </Card>

      <Card>
        <div className="flex items-center justify-between mb-3">
          <div>
            <div
              style={{
                fontFamily: FRAUNCES,
                fontSize: 16,
                fontWeight: 600,
                color: UI.parchment,
              }}
            >
              MCP Tools
            </div>
            <div style={{ marginTop: 2 }}>
              <Mono size={11} color={UI.textMuted}>
                {tools ? `${tools.length} MCP tools advertised` : "loading…"}
              </Mono>
            </div>
          </div>
          <div className="flex items-center gap-2">
            <div style={{ width: 200 }}>
              <TextInput value={toolFilter} onChange={setToolFilter} placeholder="filter…" />
            </div>
            <Button onClick={() => setToolsOpen((v) => !v)} size="sm">
              {toolsOpen ? "collapse" : "expand"}
            </Button>
          </div>
        </div>
        <AnimatePresence>
          {toolsOpen && (
            <motion.div
              initial={{ height: 0, opacity: 0 }}
              animate={{ height: "auto", opacity: 1 }}
              exit={{ height: 0, opacity: 0 }}
              transition={{ duration: 0.2 }}
              style={{ overflow: "hidden" }}
            >
              <div className="grid grid-cols-3 gap-2">
                {filteredTools.slice(0, 96).map((t) => (
                  <ToolCard
                    key={t.capability}
                    icon={<Wrench size={13} />}
                    name={t.capability}
                    description={t.description}
                  />
                ))}
              </div>
              {filteredTools.length === 0 && (
                <Serif size={12} italic color={UI.textMuted}>
                  no tools matched the filter
                </Serif>
              )}
            </motion.div>
          )}
        </AnimatePresence>
      </Card>

      <Modal
        open={installOpen}
        onClose={() => setInstallOpen(false)}
        title="Install MCP Server"
        sub="Provide the install payload — server_id, module_path, capabilities, limits."
        width={620}
        footer={
          <>
            <Button onClick={() => setInstallOpen(false)} disabled={busy === "install"}>
              Cancel
            </Button>
            <Button onClick={() => void install()} disabled={busy === "install"} variant="primary">
              {busy === "install" ? "Installing…" : "Install"}
            </Button>
          </>
        }
      >
        <Field label="Install payload (JSON)" required>
          <Textarea value={installPayload} onChange={setInstallPayload} rows={10} />
        </Field>
      </Modal>
    </div>
  );
};

const fmtBytes = (n: number): string => {
  if (n < 1024) return `${n} B`;
  if (n < 1024 * 1024) return `${(n / 1024).toFixed(1)} KB`;
  if (n < 1024 * 1024 * 1024) return `${(n / 1024 / 1024).toFixed(1)} MB`;
  return `${(n / 1024 / 1024 / 1024).toFixed(2)} GB`;
};

// Tiny helper for surfaces whose initial fetch can outlive the mount
// (operator switching tabs while a request is mid-flight). Returns a
// ref + helper that lets async work bail out before it setStates on
// an unmounted component. We *want* in-flight requests to keep going
// to completion (autonomous-agent friendly — the tab can be doing
// work while you're elsewhere), but we don't want React to warn or
// race with the next mount's state.
const useCancelledRef = () => {
  const cancelled = useRef(false);
  useEffect(() => {
    cancelled.current = false;
    return () => {
      cancelled.current = true;
    };
  }, []);
  return cancelled;
};

const AppsSurface = () => {
  const [apps, setApps] = useState<AppRow[] | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState<string | null>(null);
  const [toast, setToast] = useState<string | null>(null);
  const [createOpen, setCreateOpen] = useState(false);
  const [appName, setAppName] = useState("");
  const [appDesc, setAppDesc] = useState("");
  const cancelled = useCancelledRef();

  const refresh = async () => {
    try {
      const a = await listApps();
      if (cancelled.current) return;
      setApps(a.apps);
      setError(null);
    } catch (err: unknown) {
      if (cancelled.current) return;
      setError(err instanceof Error ? err.message : String(err));
    }
  };

  useEffect(() => {
    void refresh();
  }, []);

  const addApp = async () => {
    if (!appName.trim()) return;
    setBusy("create-app");
    setToast(null);
    try {
      const out = await createApp({
        name: appName.trim(),
        description: appDesc.trim() || undefined,
      });
      if (cancelled.current) return;
      setToast(`app created: ${out.slug}`);
      setAppName("");
      setAppDesc("");
      setCreateOpen(false);
      await refresh();
    } catch (err: unknown) {
      if (cancelled.current) return;
      setToast(`create failed: ${err instanceof Error ? err.message : String(err)}`);
    } finally {
      if (!cancelled.current) setBusy(null);
    }
  };

  const togglePublish = async (app: AppRow) => {
    setBusy(app.id);
    setToast(null);
    try {
      if (app.status === "draft") {
        await publishApp(app.id);
        if (!cancelled.current) setToast(`${app.slug} published.`);
      } else if (app.status === "published") {
        await archiveApp(app.id);
        if (!cancelled.current) setToast(`${app.slug} archived.`);
      }
      await refresh();
    } catch (err: unknown) {
      if (cancelled.current) return;
      setToast(`action failed: ${err instanceof Error ? err.message : String(err)}`);
    } finally {
      if (!cancelled.current) setBusy(null);
    }
  };

  return (
    <div className="h-full flex flex-col gap-4 overflow-auto pb-4">
      <SectionHeader
        icon={<Boxes size={22} />}
        title={`Apps · ${apps?.length ?? 0}`}
        sub="Apps are deployable units. They move through draft → published → archived."
        trailing={
          <Button onClick={() => setCreateOpen(true)} variant="primary" size="md">
            <span style={{ display: "inline-flex", alignItems: "center", gap: 4 }}>
              <Plus size={13} strokeWidth={2.5} /> New app
            </span>
          </Button>
        }
      />
      {error && <Alert variant="danger">{error}</Alert>}
      {toast && <Alert variant="success">{toast}</Alert>}

      <div className="space-y-2">
        {(apps ?? []).map((a) => {
          const variant: "success" | "warn" | "neutral" =
            a.status === "published" ? "success" : a.status === "archived" ? "neutral" : "warn";
          return (
            <ConfiguredRow
              key={a.id}
              icon={<Boxes size={14} />}
              name={a.name}
              subtitle={
                <span>
                  {a.slug} · created {a.created_at?.slice(0, 10)}
                  {a.description ? ` · ${a.description}` : ""}
                </span>
              }
              rightBadge={<Badge variant={variant}>{a.status}</Badge>}
              actions={
                a.status !== "archived" && (
                  <Button
                    onClick={() => void togglePublish(a)}
                    disabled={busy === a.id}
                    size="sm"
                    variant={a.status === "draft" ? "primary" : "secondary"}
                  >
                    {busy === a.id
                      ? "…"
                      : a.status === "draft"
                      ? "Publish"
                      : "Archive"}
                  </Button>
                )
              }
            />
          );
        })}
        {(apps ?? []).length === 0 && (
          <Card>
            <div style={{ textAlign: "center", padding: "16px 0" }}>
              <Serif size={14} italic color={UI.textMuted}>
                No apps yet. Click "New app" above to create one.
              </Serif>
            </div>
          </Card>
        )}
      </div>

      <Modal
        open={createOpen}
        onClose={() => setCreateOpen(false)}
        title="Create New App"
        sub="Apps start as drafts. Slug is derived from the name unless you set one."
        width={520}
        footer={
          <>
            <Button onClick={() => setCreateOpen(false)} disabled={busy === "create-app"}>
              Cancel
            </Button>
            <Button
              onClick={() => void addApp()}
              disabled={busy === "create-app" || !appName.trim()}
              variant="primary"
            >
              {busy === "create-app" ? "Creating…" : "Create"}
            </Button>
          </>
        }
      >
        <div className="space-y-4">
          <Field label="App name" required>
            <TextInput
              value={appName}
              onChange={setAppName}
              placeholder="My new app"
              autoFocus
            />
          </Field>
          <Field label="Description" hint="Optional. Stored on the app row.">
            <TextInput
              value={appDesc}
              onChange={setAppDesc}
              placeholder="What does this app do?"
            />
          </Field>
        </div>
      </Modal>
    </div>
  );
};

// Files lives in the knowledge group (next to RAG and Memory) because
// uploaded artifacts feed retrieval and grounding, not deployment.
// Apps are deployable units; files are read-side material.
const FilesSurface = () => {
  const [files, setFiles] = useState<FileRow[] | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState<string | null>(null);
  const [toast, setToast] = useState<string | null>(null);
  const cancelled = useCancelledRef();

  const refresh = async () => {
    try {
      const f = await listFiles();
      if (cancelled.current) return;
      setFiles(f.files);
      setError(null);
    } catch (err: unknown) {
      if (cancelled.current) return;
      setError(err instanceof Error ? err.message : String(err));
    }
  };

  useEffect(() => {
    void refresh();
  }, []);

  const onUploadChange = async (e: React.ChangeEvent<HTMLInputElement>) => {
    const file = e.target.files?.[0];
    if (!file) return;
    setBusy("upload");
    setToast(null);
    try {
      const data_base64 = await fileToBase64(file);
      await uploadFileBase64({
        original_name: file.name,
        data_base64,
        content_type: file.type || "application/octet-stream",
        created_by: "operator",
      });
      if (!cancelled.current) setToast(`uploaded ${file.name}.`);
      await refresh();
    } catch (err: unknown) {
      if (cancelled.current) return;
      setToast(`upload failed: ${err instanceof Error ? err.message : String(err)}`);
    } finally {
      if (!cancelled.current) setBusy(null);
      e.target.value = "";
    }
  };

  return (
    <div className="h-full flex flex-col gap-4 overflow-auto pb-4">
      <SectionHeader
        icon={<FolderOpen size={22} />}
        title={`Files · ${files?.length ?? 0}`}
        sub="Uploaded artifacts the assistant can read from. Stored in user-files/ with metadata + SHA-256 in ordo.db (content-hashed for dedupe)."
        trailing={
          <label
            style={{
              cursor: busy === "upload" ? "wait" : "pointer",
              padding: "8px 14px",
              borderRadius: 6,
              fontFamily: MONO,
              fontSize: 12,
              fontWeight: 600,
              background: busy === "upload" ? "rgba(255,255,255,0.05)" : UI.primary,
              color: busy === "upload" ? UI.slate : UI.ink,
              display: "inline-flex",
              alignItems: "center",
              gap: 5,
            }}
          >
            <Plus size={13} strokeWidth={2.5} />
            {busy === "upload" ? "Uploading…" : "Upload file"}
            <input
              type="file"
              onChange={(e) => void onUploadChange(e)}
              style={{ display: "none" }}
              disabled={busy === "upload"}
            />
          </label>
        }
      />
      {error && <Alert variant="danger">{error}</Alert>}
      {toast && <Alert variant="success">{toast}</Alert>}

      <div className="space-y-2">
        {(files ?? []).map((f) => (
          <ConfiguredRow
            key={f.id}
            icon={<FileText size={14} />}
            name={f.original_name}
            subtitle={
              <span>
                {fmtBytes(f.size_bytes)} · {f.content_type} · {f.created_at?.slice(0, 10)}
              </span>
            }
            rightBadge={
              <Badge variant="neutral">{f.sha256_hex.slice(0, 8)}</Badge>
            }
          />
        ))}
        {(files ?? []).length === 0 && (
          <Card>
            <div style={{ textAlign: "center", padding: "16px 0" }}>
              <Serif size={14} italic color={UI.textMuted}>
                No files uploaded yet.
              </Serif>
            </div>
          </Card>
        )}
      </div>
    </div>
  );
};

function fileToBase64(file: File): Promise<string> {
  return new Promise((resolve, reject) => {
    const reader = new FileReader();
    reader.onload = () => {
      const result = reader.result as string;
      // Strip the "data:<mime>;base64," prefix.
      const idx = result.indexOf(",");
      resolve(idx >= 0 ? result.slice(idx + 1) : result);
    };
    reader.onerror = () => reject(reader.error ?? new Error("read failed"));
    reader.readAsDataURL(file);
  });
}

interface WebhookDraft {
  target_url: string;
  topics: string;
  description: string;
  secret: string;
}

const blankWebhookDraft = (): WebhookDraft => ({
  target_url: "",
  topics: "ordo.apps.event, ordo.files.event",
  description: "",
  secret: "",
});

// One-time secret reveal: when register returns a fresh secret we show
// it prominently so the operator can capture it before subsequent reads
// redact it. Stored only in component state.
interface RevealedSecret {
  id: string;
  url: string;
  secret: string;
}

const WebhooksSurface = () => {
  const [subs, setSubs] = useState<WebhookSubscription[] | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState<string | null>(null);
  const [toast, setToast] = useState<string | null>(null);
  const [createOpen, setCreateOpen] = useState(false);
  const [draft, setDraft] = useState<WebhookDraft>(blankWebhookDraft());
  const [revealed, setRevealed] = useState<RevealedSecret | null>(null);

  const refresh = async () => {
    try {
      const res = await listWebhooks();
      setSubs(res.subscriptions);
      setError(null);
    } catch (err: unknown) {
      setError(err instanceof Error ? err.message : String(err));
    }
  };

  useEffect(() => {
    void refresh();
  }, []);

  const submitDraft = async () => {
    if (!draft.target_url.trim()) return;
    setBusy("create");
    setToast(null);
    try {
      const res = await registerWebhook({
        target_url: draft.target_url.trim(),
        topics: draft.topics
          .split(",")
          .map((t) => t.trim())
          .filter(Boolean),
        description: draft.description.trim(),
        secret: draft.secret.trim() || undefined,
      });
      // The runtime returns the plaintext secret on register only —
      // surface it so the operator can capture it.
      setRevealed({
        id: res.subscription.id,
        url: res.subscription.target_url,
        secret: res.subscription.secret,
      });
      setToast(`subscription registered (${res.subscription.id.slice(0, 8)}).`);
      setDraft(blankWebhookDraft());
      setCreateOpen(false);
      await refresh();
    } catch (err: unknown) {
      setToast(`register failed: ${err instanceof Error ? err.message : String(err)}`);
    } finally {
      setBusy(null);
    }
  };

  const toggleActive = async (sub: WebhookSubscription) => {
    setBusy(sub.id);
    setToast(null);
    try {
      await updateWebhook(sub.id, { active: !sub.active });
      setToast(`${sub.id.slice(0, 8)}: ${sub.active ? "paused" : "resumed"}.`);
      await refresh();
    } catch (err: unknown) {
      setToast(`update failed: ${err instanceof Error ? err.message : String(err)}`);
    } finally {
      setBusy(null);
    }
  };

  const remove = async (sub: WebhookSubscription) => {
    if (!confirm(`Delete webhook to ${sub.target_url}?`)) return;
    setBusy(sub.id);
    setToast(null);
    try {
      await deleteWebhook(sub.id);
      setToast(`${sub.id.slice(0, 8)}: removed.`);
      await refresh();
    } catch (err: unknown) {
      setToast(`delete failed: ${err instanceof Error ? err.message : String(err)}`);
    } finally {
      setBusy(null);
    }
  };

  return (
    <div className="h-full flex flex-col gap-4 overflow-auto pb-4">
      <SectionHeader
        icon={<Webhook size={22} />}
        title="Webhooks"
        sub="HMAC-signed event subscriptions. Receivers verify on each delivery, replay-safe."
        trailing={
          <div className="flex items-center gap-2">
            <Button onClick={() => void refresh()} size="sm">
              <span style={{ display: "inline-flex", alignItems: "center", gap: 4 }}>
                <RefreshCcw size={11} /> Refresh
              </span>
            </Button>
            <Button
              onClick={() => {
                setDraft(blankWebhookDraft());
                setCreateOpen(true);
              }}
              variant="primary"
              size="md"
            >
              <span style={{ display: "inline-flex", alignItems: "center", gap: 4 }}>
                <Plus size={13} strokeWidth={2.5} /> New webhook
              </span>
            </Button>
          </div>
        }
      />
      {error && <Alert variant="danger">{error}</Alert>}
      {toast && <Alert variant="success">{toast}</Alert>}
      {revealed && (
        <Card
          style={{
            background: `linear-gradient(180deg, ${UI.primarySoft}, transparent)`,
            border: `1px solid ${UI.primaryBorder}`,
          }}
        >
          <div className="flex items-start justify-between mb-2">
            <Mono size={10} upper track="0.2em" color={UI.primary} weight={600}>
              one-time secret · capture now
            </Mono>
            <button
              onClick={() => setRevealed(null)}
              style={{
                background: "transparent",
                border: "none",
                color: UI.textMuted,
                cursor: "pointer",
              }}
              title="dismiss"
            >
              <X size={14} />
            </button>
          </div>
          <Serif size={13} color={UI.parchment} style={{ marginBottom: 10 }}>
            Subscription <code style={{ fontFamily: MONO }}>{revealed.id.slice(0, 8)}</code> registered for{" "}
            <code style={{ fontFamily: MONO }}>{revealed.url}</code>. Subsequent reads will return{" "}
            <code style={{ fontFamily: MONO }}>&lt;redacted&gt;</code> — copy this secret now.
          </Serif>
          <CopyableField value={revealed.secret} label="HMAC secret" />
        </Card>
      )}

      <div className="space-y-2">
        {(subs ?? []).map((s) => (
          <ConfiguredRow
            key={s.id}
            icon={<Webhook size={14} />}
            name={s.target_url}
            subtitle={
              <span>
                {s.id.slice(0, 8)} · {s.topics.length === 0 ? "all topics" : s.topics.join(" · ")}
                {s.description ? ` — ${s.description}` : ""}
              </span>
            }
            rightBadge={<Badge variant={s.active ? "success" : "neutral"}>{s.active ? "active" : "paused"}</Badge>}
            actions={
              <>
                <Button
                  onClick={() => void toggleActive(s)}
                  disabled={busy === s.id}
                  size="sm"
                >
                  {s.active ? "Pause" : "Resume"}
                </Button>
                <Button
                  onClick={() => void remove(s)}
                  disabled={busy === s.id}
                  variant="danger"
                  size="sm"
                >
                  <Trash2 size={12} />
                </Button>
              </>
            }
          />
        ))}
        {(subs ?? []).length === 0 && (
          <Card>
            <div style={{ textAlign: "center", padding: "16px 0" }}>
              <Serif size={14} italic color={UI.textMuted}>
                No webhook subscriptions yet. Click "New webhook" above to register one.
              </Serif>
            </div>
          </Card>
        )}
      </div>

      <Modal
        open={createOpen}
        onClose={() => setCreateOpen(false)}
        title="New webhook subscription"
        sub="Receivers get an HMAC-SHA256 signature on every delivery. Topics are matched against bus envelope topics; leave empty for all."
        width={620}
        footer={
          <>
            <Button onClick={() => setCreateOpen(false)} disabled={busy === "create"}>
              Cancel
            </Button>
            <Button
              onClick={() => void submitDraft()}
              disabled={busy === "create" || !draft.target_url.trim()}
              variant="primary"
            >
              {busy === "create" ? "Registering…" : "Register"}
            </Button>
          </>
        }
      >
        <div className="space-y-4">
          <Field
            label="Target URL"
            required
            hint="The receiver Ordo POSTs signed JSON to. Must be http:// or https://."
          >
            <TextInput
              value={draft.target_url}
              onChange={(v) => setDraft({ ...draft, target_url: v })}
              placeholder="https://your-service.example.com/ordo-hook"
              type="url"
              autoFocus
            />
          </Field>
          <Field
            label="Topics"
            hint="Comma-separated. Leave empty to subscribe to every bus topic."
          >
            <TextInput
              value={draft.topics}
              onChange={(v) => setDraft({ ...draft, topics: v })}
              placeholder="ordo.apps.event, ordo.files.event"
            />
          </Field>
          <Field label="Description">
            <TextInput
              value={draft.description}
              onChange={(v) => setDraft({ ...draft, description: v })}
              placeholder="What this webhook fans out to"
            />
          </Field>
          <Field
            label="Secret"
            hint="Optional. If blank, Ordo generates a random 32-byte hex secret and shows it once after registration."
          >
            <TextInput
              type="password"
              value={draft.secret}
              onChange={(v) => setDraft({ ...draft, secret: v })}
              placeholder="leave blank to generate"
            />
          </Field>
        </div>
      </Modal>
    </div>
  );
};

// Combines what used to be the separate Security and Medbay tabs.
// Both surfaces were log-driven observability views and read more
// honestly together: the audit ring is the security log, the
// self-heal case history is the health log, and surfacing them
// alongside the rules they enforce keeps cause + effect on one
// page. Three columns, top to bottom: rules + audit row, then
// self-heal cases as a full-width log below.
const SecurityHealthSurface = () => {
  const [rules, setRules] = useState<SecurityRule[] | null>(null);
  const [audit, setAudit] = useState<AuditEntry[] | null>(null);
  const [cases, setCases] = useState<SelfHealCaseRow[] | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState<string | null>(null);
  const [toast, setToast] = useState<string | null>(null);

  const refresh = async () => {
    try {
      const [r, a, c] = await Promise.all([
        listSecurityRules(),
        listSecurityAudit(50),
        listSelfHealCases(50),
      ]);
      setRules(r.rules);
      setAudit(a.entries);
      setCases(flattenCases(c));
      setError(null);
    } catch (err: unknown) {
      setError(err instanceof Error ? err.message : String(err));
    }
  };

  useEffect(() => {
    void refresh();
  }, []);

  const action = async (kind: "replay" | "pin" | "export", id: string) => {
    setBusy(`${kind}:${id}`);
    setToast(null);
    try {
      if (kind === "replay") await replaySelfHealCase(id);
      else if (kind === "pin") await pinSelfHealCase(id);
      else await exportSelfHealCase(id);
      setToast(`${kind} ok for case ${id.slice(0, 8)}.`);
      await refresh();
    } catch (err: unknown) {
      setToast(`${kind} failed: ${err instanceof Error ? err.message : String(err)}`);
    } finally {
      setBusy(null);
    }
  };

  return (
    <div className="h-full flex flex-col gap-4 overflow-auto pb-4">
      <SectionHeader
        icon={<ShieldCheck size={22} />}
        title="Security & Health"
        sub="Two logs live here. The audit ring records every gated capability call. The self-heal log records every issue the runtime classified and the fix it applied."
        trailing={
          <Button onClick={() => void refresh()} size="sm">
            <span style={{ display: "inline-flex", alignItems: "center", gap: 4 }}>
              <RefreshCcw size={11} /> Refresh
            </span>
          </Button>
        }
      />
      {error && <Alert variant="danger">{error}</Alert>}
      {toast && <Alert variant="success">{toast}</Alert>}

      <div className="grid grid-cols-2 gap-3">
        <Card>
          <Mono size={11} upper track="0.18em" color={UI.textMuted}>
            Rules · {rules?.length ?? 0}
          </Mono>
          <div className="space-y-1.5" style={{ marginTop: 12, maxHeight: 380, overflow: "auto" }}>
            {(rules ?? []).map((r) => {
              const variant: "danger" | "warn" | "neutral" =
                r.severity === "critical" || r.severity === "high"
                  ? "danger"
                  : r.severity === "medium"
                  ? "warn"
                  : "neutral";
              return (
                <div
                  key={r.id}
                  style={{
                    background: UI.cardBgRaised,
                    border: `1px solid ${UI.cardBorder}`,
                    borderRadius: 6,
                    padding: "8px 12px",
                  }}
                >
                  <div className="flex items-center justify-between">
                    <Mono size={11} color={UI.parchment}>
                      {r.id}
                    </Mono>
                    <Badge variant={variant}>{r.severity}</Badge>
                  </div>
                  <div style={{ marginTop: 4 }}>
                    <Mono size={10} color={UI.textMuted}>
                      {r.description}
                    </Mono>
                  </div>
                  <div style={{ marginTop: 2 }}>
                    <Mono size={9} color={UI.textDim}>
                      phases: {r.phases} · {r.enabled ? "enabled" : "disabled"}
                    </Mono>
                  </div>
                </div>
              );
            })}
            {(rules ?? []).length === 0 && (
              <Serif size={12} italic color={UI.textMuted}>
                No security rules configured.
              </Serif>
            )}
          </div>
        </Card>
        <Card>
          <Mono size={11} upper track="0.18em" color={UI.textMuted}>
            Audit ring · {audit?.length ?? 0}
          </Mono>
          <div className="space-y-1.5" style={{ marginTop: 12, maxHeight: 380, overflow: "auto" }}>
            {(audit ?? []).map((a) => {
              const variant: "success" | "danger" =
                a.outcome === "allow" || a.outcome === "ok" ? "success" : "danger";
              return (
                <div
                  key={a.id}
                  style={{
                    background: UI.cardBgRaised,
                    border: `1px solid ${UI.cardBorder}`,
                    borderRadius: 6,
                    padding: "8px 12px",
                  }}
                >
                  <div className="flex items-center justify-between">
                    <Mono size={11} color={UI.parchment}>
                      {a.capability}
                    </Mono>
                    <Badge variant={variant}>{a.outcome}</Badge>
                  </div>
                  <div style={{ marginTop: 4 }}>
                    <Mono size={10} color={UI.textDim}>
                      {a.scope} · {a.timestamp}
                    </Mono>
                  </div>
                  {a.detail && (
                    <div style={{ marginTop: 2 }}>
                      <Mono size={10} color={UI.textMuted}>
                        {a.detail}
                      </Mono>
                    </div>
                  )}
                </div>
              );
            })}
            {(audit ?? []).length === 0 && (
              <Serif size={12} italic color={UI.textMuted}>
                No recent audit entries.
              </Serif>
            )}
          </div>
        </Card>
      </div>

      <Card>
        <div className="flex items-center justify-between mb-3">
          <Mono size={11} upper track="0.18em" color={UI.textMuted}>
            Self-heal log · {cases?.length ?? 0}
          </Mono>
          <Serif size={11} italic color={UI.textDim}>
            LLM classifies. Algorithms repair. Pinned cases replay first.
          </Serif>
        </div>
        <div className="space-y-2.5">
          {(cases ?? []).map((c, i) => {
            const id = c.id ?? c.fingerprint ?? `c-${i}`;
            const replays = c.replay_count ?? c.replays ?? 0;
            const classified = c.classified ?? c.classification ?? "(unclassified)";
            const fix = c.fix ?? (c.actions ? c.actions.join(" → ") : "(no fix recorded)");
            return (
              <div
                key={id}
                style={
                  c.pinned
                    ? {
                        background: `linear-gradient(180deg, ${UI.primarySoft}, transparent)`,
                        border: `1px solid ${UI.primaryBorder}`,
                        borderRadius: 8,
                        padding: 12,
                      }
                    : {
                        background: UI.cardBgRaised,
                        border: `1px solid ${UI.cardBorder}`,
                        borderRadius: 8,
                        padding: 12,
                      }
                }
              >
                <div className="flex items-center gap-2 mb-2">
                  {c.pinned && <Pin size={12} color={UI.primary} />}
                  <Mono size={9} upper track="0.2em" color={UI.textDim}>
                    {id.slice(0, 12)} · replays {replays}
                  </Mono>
                  {c.last_seen_at && (
                    <Mono size={9} color={UI.textDim}>
                      · {c.last_seen_at}
                    </Mono>
                  )}
                </div>
                <Serif size={14} color={UI.parchment} style={{ marginBottom: 10 }}>
                  {c.symptom ?? "(no symptom recorded)"}
                </Serif>
                <div className="flex items-center gap-2 flex-wrap">
                  <Badge variant="info">classify · {classified}</Badge>
                  <span style={{ color: UI.textDim, fontSize: 11 }}>→</span>
                  <Badge variant="success">fix · {fix}</Badge>
                  <span style={{ flex: 1 }} />
                  {!c.pinned && (
                    <Button onClick={() => void action("pin", id)} disabled={busy?.endsWith(id)} size="sm">
                      <span style={{ display: "inline-flex", alignItems: "center", gap: 4 }}>
                        <Pin size={11} /> Pin
                      </span>
                    </Button>
                  )}
                  <Button onClick={() => void action("export", id)} disabled={busy?.endsWith(id)} size="sm">
                    Export
                  </Button>
                  <Button
                    onClick={() => void action("replay", id)}
                    disabled={busy?.endsWith(id)}
                    variant="primary"
                    size="sm"
                  >
                    <span style={{ display: "inline-flex", alignItems: "center", gap: 4 }}>
                      <RefreshCcw size={11} /> Replay
                    </span>
                  </Button>
                </div>
              </div>
            );
          })}
          {(cases ?? []).length === 0 && (
            <div style={{ textAlign: "center", padding: "16px 0" }}>
              <Serif size={14} italic color={UI.textMuted}>
                No remembered cases yet — the runtime is healthy.
              </Serif>
            </div>
          )}
        </div>
      </Card>
    </div>
  );
};

const ReviewSurface = () => {
  const [pending, setPending] = useState<ReviewRequest[] | null>(null);
  const [recent, setRecent] = useState<ReviewRequest[] | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState<string | null>(null);
  const [toast, setToast] = useState<string | null>(null);

  const refresh = async () => {
    try {
      const [p, r] = await Promise.all([listReviewPending(), listReviewRecent(20)]);
      setPending(p.pending);
      setRecent(r.recent);
      setError(null);
    } catch (err: unknown) {
      setError(err instanceof Error ? err.message : String(err));
    }
  };

  useEffect(() => {
    void refresh();
    const interval = setInterval(refresh, 5000);
    return () => clearInterval(interval);
  }, []);

  const decide = async (id: string, action: "approve" | "deny") => {
    setBusy(id);
    setToast(null);
    try {
      if (action === "approve") {
        await approveReview(id);
        setToast(`${id} approved.`);
      } else {
        await denyReview(id);
        setToast(`${id} denied.`);
      }
      await refresh();
    } catch (err: unknown) {
      setToast(`action failed: ${err instanceof Error ? err.message : String(err)}`);
    } finally {
      setBusy(null);
    }
  };

  return (
    <div className="h-full flex flex-col gap-4 overflow-auto pb-4">
      <SectionHeader
        icon={<Eye size={22} />}
        title="Review"
        sub="Anything Ordo produces that needs human approval flows here. Polls every 5 seconds."
      />
      {error && <Alert variant="danger">{error}</Alert>}
      {toast && <Alert variant="success">{toast}</Alert>}

      <SectionHeader
        icon={<Eye size={20} />}
        title={`Pending · ${pending?.length ?? 0}`}
        sub="Awaiting your decision."
      />
      <div className="space-y-2">
        {(pending ?? []).map((q) => (
          <Card key={q.id}>
            <div className="flex items-start justify-between mb-3">
              <div style={{ flex: 1, minWidth: 0 }}>
                <Serif size={15} weight={600} color={UI.parchment}>
                  {q.title}
                </Serif>
                <div style={{ marginTop: 4 }}>
                  <Mono size={10} color={UI.textMuted}>
                    {q.capability ? `from ${q.capability}` : q.id} · {q.created_at}
                  </Mono>
                </div>
              </div>
              <Badge variant="warn">{q.state}</Badge>
            </div>
            <div className="flex justify-end gap-2">
              <Button onClick={() => void decide(q.id, "deny")} disabled={busy === q.id} variant="danger" size="sm">
                <span style={{ display: "inline-flex", alignItems: "center", gap: 4 }}>
                  <X size={11} /> Deny
                </span>
              </Button>
              <Button onClick={() => void decide(q.id, "approve")} disabled={busy === q.id} variant="primary" size="sm">
                <span style={{ display: "inline-flex", alignItems: "center", gap: 4 }}>
                  <Check size={11} strokeWidth={3} /> Approve
                </span>
              </Button>
            </div>
          </Card>
        ))}
        {(pending ?? []).length === 0 && (
          <Card>
            <div style={{ textAlign: "center", padding: "16px 0" }}>
              <Serif size={14} italic color={UI.textMuted}>
                No requests waiting on you.
              </Serif>
            </div>
          </Card>
        )}
      </div>

      <SectionHeader
        icon={<Eye size={20} />}
        title={`Recent · ${recent?.length ?? 0}`}
        sub="Decisions you've already made."
      />
      <div className="space-y-1.5">
        {(recent ?? []).slice(0, 12).map((q) => {
          const variant: "success" | "danger" | "neutral" =
            q.state === "approved" ? "success" : q.state === "denied" ? "danger" : "neutral";
          return (
            <Card key={q.id} padded={false}>
              <div
                style={{
                  padding: "10px 14px",
                  display: "flex",
                  alignItems: "center",
                  gap: 12,
                }}
              >
                <Badge variant={variant}>{q.state}</Badge>
                <Serif size={12} color={UI.parchment} style={{ flex: 1, minWidth: 0 }}>
                  {q.title}
                </Serif>
                <Mono size={10} color={UI.textDim}>
                  {q.decided_at ?? q.created_at}
                </Mono>
              </div>
            </Card>
          );
        })}
      </div>
    </div>
  );
};

const RuntimeSurface = () => {
  const [profileData, setProfileData] = useState<RuntimeProfile | null>(null);
  const [storage, setStorage] = useState<RuntimeStorage | null>(null);
  const [settings, setSettings] = useState<RuntimeSettingsSnapshot | null>(null);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const [toast, setToast] = useState<string | null>(null);
  const [healBudgetGb, setHealBudgetGb] = useState(0.5);
  const [timeoutSecs, setTimeoutSecs] = useState<number>(() => loadTimeoutPreset());
  const [strictness, setStrictness] = useState<StrictnessPreset>(() =>
    loadStrictnessPreset(),
  );
  const commitStrictness = (id: StrictnessPreset) => {
    saveStrictnessPreset(id);
    setStrictness(id);
  };
  const [timeoutBusy, setTimeoutBusy] = useState(false);

  const refresh = async () => {
    setLoading(true);
    try {
      const [p, s, set] = await Promise.all([
        fetchRuntimeProfile(),
        fetchRuntimeStorage(),
        fetchRuntimeSettings(),
      ]);
      setProfileData(p);
      setStorage(s);
      setSettings(set);
      setHealBudgetGb(bytesToGb(s.self_heal_history_budget_bytes));
      setError(null);
    } catch (err: unknown) {
      setError(err instanceof Error ? err.message : String(err));
    } finally {
      setLoading(false);
    }
  };

  useEffect(() => {
    void refresh();
  }, []);

  const persistedProfile =
    (settings?.persisted?.profile as string | undefined) ?? null;
  const restartPending =
    persistedProfile !== null && persistedProfile !== profileData?.profile;

  const updateProfile = async (id: string) => {
    if (!profileData || profileData.profile === id) return;
    setBusy(true);
    setToast(null);
    try {
      const res = await updateRuntimeSettings({ profile: id });
      setToast(
        res.restart_required
          ? `profile saved as "${id}". restart \`ordo serve\` to activate.`
          : `profile updated to "${id}".`,
      );
      await refresh();
    } catch (err: unknown) {
      setToast(`update failed: ${err instanceof Error ? err.message : String(err)}`);
    } finally {
      setBusy(false);
    }
  };

  const commitHealBudget = async (gb: number) => {
    setBusy(true);
    setToast(null);
    try {
      await updateRuntimeSettings({ self_heal_history_budget_bytes: gbToBytes(gb) });
      setToast(`self-heal history budget saved (${gb} GB).`);
      await refresh();
    } catch (err: unknown) {
      setToast(`update failed: ${err instanceof Error ? err.message : String(err)}`);
    } finally {
      setBusy(false);
    }
  };

  // Apply a timeout preset: persist the operator's choice locally,
  // then propagate it into every configured cloud credential's
  // `extras.timeout_secs` so the runtime's per-credential override
  // honors the new value uniformly. New credentials inherit the
  // preset at creation time (see CloudSurface).
  //
  // The runtime REPLACES (not merges) extras on upsert, so we read
  // the existing bag, merge in the new key, and send the whole
  // thing back. Any `***` redaction sentinels in extras (legacy
  // rows or secret-shaped keys) are stripped before the round-trip
  // so we don't accidentally persist the placeholder over a real
  // value.
  const commitTimeoutPreset = async (secs: number) => {
    setTimeoutBusy(true);
    setToast(null);
    try {
      saveTimeoutPreset(secs);
      setTimeoutSecs(secs);
      const list = await listCloudCredentials();
      let touched = 0;
      for (const c of list.credentials) {
        const existingExtras = (c.extras ?? {}) as Record<string, string>;
        const cleanExtras: Record<string, string> = {};
        for (const [k, v] of Object.entries(existingExtras)) {
          // Skip redacted placeholders — writing them back would
          // overwrite a real secret with the literal string "***".
          if (v !== "***") cleanExtras[k] = v;
        }
        cleanExtras.timeout_secs = String(secs);
        await upsertCloudCredential({
          service: c.service,
          extras: cleanExtras,
        });
        touched += 1;
      }
      const minutes = secs / 60;
      const minLabel = Number.isInteger(minutes) ? `${minutes} min` : `${secs} s`;
      setToast(
        touched === 0
          ? `response timeout set to ${minLabel}. New credentials will use this value.`
          : `response timeout set to ${minLabel}. Applied to ${touched} configured ${touched === 1 ? "provider" : "providers"}.`,
      );
    } catch (err: unknown) {
      setToast(
        `timeout update failed: ${err instanceof Error ? err.message : String(err)}`,
      );
    } finally {
      setTimeoutBusy(false);
    }
  };

  return (
    <div className="h-full flex flex-col gap-4 overflow-auto pb-4">
      <SectionHeader
        icon={<Cpu size={22} />}
        title="Runtime"
        sub="How Ordo boots. Profiles change what activates eagerly. Budgets persist to ordo.db."
        trailing={
          <Button onClick={() => void refresh()} disabled={loading || busy} size="md">
            <span style={{ display: "inline-flex", alignItems: "center", gap: 4 }}>
              <RefreshCcw size={11} /> {loading ? "Loading…" : "Refresh"}
            </span>
          </Button>
        }
      />
      {error && <Alert variant="danger">failed to load runtime state: {error}</Alert>}
      {restartPending && (
        <Alert variant="warn">
          <span style={{ fontWeight: 600 }}>Restart pending.</span> Persisted profile is{" "}
          <code style={{ fontFamily: MONO, color: UI.amber }}>{persistedProfile}</code>;
          effective profile is{" "}
          <code style={{ fontFamily: MONO, color: UI.jade }}>{profileData?.profile}</code>.
          Stop and re-run <code style={{ fontFamily: MONO }}>ordo serve</code> to apply.
        </Alert>
      )}
      {toast && <Alert variant="success">{toast}</Alert>}

      <Card>
        <Mono size={11} upper track="0.18em" color={UI.textMuted}>
          Profile
        </Mono>
        <div className="grid grid-cols-3 gap-2" style={{ marginTop: 12 }}>
          {PROFILES.map((p) => {
            const active = profileData?.profile === p.id;
            const persisted = persistedProfile === p.id;
            return (
              <button
                key={p.id}
                onClick={() => void updateProfile(p.id)}
                disabled={busy || loading || active}
                style={{
                  textAlign: "left",
                  padding: "12px 14px",
                  borderRadius: 8,
                  background: active ? UI.primarySoft : UI.cardBgRaised,
                  border: `1px solid ${active ? UI.primaryBorder : UI.cardBorder}`,
                  cursor: active ? "default" : busy || loading ? "wait" : "pointer",
                  opacity: busy && !active ? 0.6 : 1,
                  transition: "all 0.15s",
                }}
              >
                <div className="flex items-center justify-between">
                  <Mono
                    size={11}
                    upper
                    track="0.15em"
                    color={active ? UI.primary : UI.parchment}
                    weight={600}
                  >
                    {p.id}
                  </Mono>
                  {persisted && !active && <Badge variant="warn">PENDING</Badge>}
                </div>
                <div style={{ marginTop: 4 }}>
                  <Mono size={10} color={UI.textDim}>
                    {p.note}
                  </Mono>
                </div>
              </button>
            );
          })}
        </div>
        {profileData && (
          <div
            style={{
              marginTop: 14,
              paddingTop: 14,
              borderTop: `1px solid ${UI.cardBorder}`,
              display: "grid",
              gridTemplateColumns: "1fr 1fr",
              gap: "6px 24px",
            }}
          >
            <RuntimeStat label="rag" value={profileData.rag_enabled ? profileData.rag_activation_mode : "disabled"} />
            <RuntimeStat label="knowledge" value={profileData.knowledge_enabled ? profileData.knowledge_activation_mode : "disabled"} />
            <RuntimeStat label="embedding backend" value={profileData.embedding_backend} />
            <RuntimeStat label="embedding dims" value={String(profileData.embedding_dimensions)} />
            <RuntimeStat label="llama.cpp" value={profileData.llama_cpp_configured ? "configured" : "not configured"} />
            <RuntimeStat label="control api" value={profileData.control_api_bind} />
          </div>
        )}
      </Card>

      <Card>
        <div className="flex items-center justify-between">
          <Mono size={11} upper track="0.18em" color={UI.textMuted}>
            Self-heal history budget
          </Mono>
          <Button
            onClick={() => void commitHealBudget(healBudgetGb)}
            disabled={busy || loading}
            variant="primary"
            size="sm"
          >
            {busy ? "Saving…" : "Save"}
          </Button>
        </div>
        <div style={{ marginTop: 12 }}>
          <Slider
            value={healBudgetGb}
            max={5}
            onChange={setHealBudgetGb}
            unit="GB"
            color={UI.amber}
          />
        </div>
        <div style={{ marginTop: 10 }}>
          <Mono size={10} color={UI.textDim}>
            self_heal_history_budget_bytes ·{" "}
            {storage ? gbToBytes(healBudgetGb).toLocaleString() : "—"} bytes
          </Mono>
        </div>
      </Card>

      {/* Response timeout — operator-facing preset for how long the
          runtime waits on an LLM call before giving up. The runtime
          honors a per-credential override (extras.timeout_secs);
          this card writes that field uniformly across every
          configured provider so the operator picks once. Three
          presets cover the meaningful range — fast/balanced/slow —
          without exposing the raw seconds value. */}
      <Card>
        <div className="flex items-center justify-between">
          <div>
            <Mono size={11} upper track="0.18em" color={UI.textMuted}>
              Response timeout
            </Mono>
            <div style={{ marginTop: 4 }}>
              <Mono size={10} color={UI.textDim}>
                Maximum time the runtime waits on a single LLM call
                before giving up. Local reasoning models with big
                context need more headroom than fast cloud APIs.
              </Mono>
            </div>
          </div>
        </div>
        <div className="grid grid-cols-3 gap-2" style={{ marginTop: 12 }}>
          {TIMEOUT_PRESETS.map((p) => {
            const active = timeoutSecs === p.secs;
            return (
              <button
                key={p.secs}
                onClick={() => void commitTimeoutPreset(p.secs)}
                disabled={timeoutBusy || active}
                style={{
                  textAlign: "left",
                  padding: "12px 14px",
                  borderRadius: 8,
                  background: active ? UI.primarySoft : UI.cardBgRaised,
                  border: `1px solid ${active ? UI.primaryBorder : UI.cardBorder}`,
                  cursor: active
                    ? "default"
                    : timeoutBusy
                    ? "wait"
                    : "pointer",
                  opacity: timeoutBusy && !active ? 0.6 : 1,
                  transition: "all 0.15s",
                }}
              >
                <div className="flex items-center justify-between">
                  <Mono
                    size={11}
                    upper
                    track="0.15em"
                    color={active ? UI.primary : UI.parchment}
                    weight={600}
                  >
                    {p.label}
                  </Mono>
                  {active && <Badge variant="info">CURRENT</Badge>}
                </div>
                <div style={{ marginTop: 4 }}>
                  <Mono size={10} color={UI.textDim}>
                    {p.sub}
                  </Mono>
                </div>
              </button>
            );
          })}
        </div>
        <div style={{ marginTop: 10 }}>
          <Mono size={10} color={UI.textDim}>
            Applies to every configured cloud provider. New providers
            inherit this value automatically.
          </Mono>
        </div>
      </Card>

      {/* Untrusted-content strictness — controls how the assistant
          handles `<untrusted_web_content>` blocks (output of the
          Strainer). The runtime reads metadata.untrusted_strictness
          per turn and assembles the bootstrap system prompt
          accordingly. Off is debug-only; medium is recommended. */}
      <Card>
        <div className="flex items-center justify-between">
          <div>
            <Mono size={11} upper track="0.18em" color={UI.textMuted}>
              Untrusted-content strictness
            </Mono>
            <div style={{ marginTop: 4 }}>
              <Mono size={10} color={UI.textDim}>
                How strictly the assistant treats web content wrapped in
                <code style={{ fontFamily: MONO, marginLeft: 4, marginRight: 4 }}>
                  &lt;untrusted_web_content&gt;
                </code>
                tags (output of the Strainer). Off disables the rule
                entirely — debug only.
              </Mono>
            </div>
          </div>
        </div>
        <div className="grid grid-cols-4 gap-2" style={{ marginTop: 12 }}>
          {STRICTNESS_PRESETS.map((p) => {
            const active = strictness === p.id;
            const isWarn = p.warn === true;
            const borderColor = active
              ? isWarn
                ? UI.red
                : UI.primaryBorder
              : UI.cardBorder;
            const bg = active
              ? isWarn
                ? "rgba(232,93,93,0.12)"
                : UI.primarySoft
              : UI.cardBgRaised;
            return (
              <button
                key={p.id}
                onClick={() => commitStrictness(p.id)}
                disabled={active}
                style={{
                  textAlign: "left",
                  padding: "12px 14px",
                  borderRadius: 8,
                  background: bg,
                  border: `1px solid ${borderColor}`,
                  cursor: active ? "default" : "pointer",
                  transition: "all 0.15s",
                }}
              >
                <div className="flex items-center justify-between">
                  <Mono
                    size={11}
                    upper
                    track="0.15em"
                    color={
                      active
                        ? isWarn
                          ? UI.red
                          : UI.primary
                        : UI.parchment
                    }
                    weight={600}
                  >
                    {p.label}
                  </Mono>
                  {active && (
                    <Badge variant={isWarn ? "danger" : "info"}>
                      CURRENT
                    </Badge>
                  )}
                </div>
                <div style={{ marginTop: 4 }}>
                  <Mono size={10} color={UI.textDim}>
                    {p.sub}
                  </Mono>
                </div>
              </button>
            );
          })}
        </div>
        <div style={{ marginTop: 10 }}>
          <Mono size={10} color={UI.textDim}>
            Sent on every turn as
            <code style={{ fontFamily: MONO, marginLeft: 4, marginRight: 4 }}>
              metadata.untrusted_strictness
            </code>
            — the runtime reads it and picks the matching bootstrap-prompt
            rule appendix.
          </Mono>
        </div>
      </Card>

      {/* Bus — runtime-level note about the Tokio event bus and the
          channel WS endpoints the studio currently uses. Used to be
          its own tab but it's a system property, not a orchestration, so
          it sits here as reference material below the response
          timeout. */}
      <Card>
        <div className="flex items-center gap-2" style={{ marginBottom: 8 }}>
          <Radio size={14} color={UI.textMuted} />
          <Mono size={11} upper track="0.18em" color={UI.textMuted}>
            Bus
          </Mono>
        </div>
        <Serif size={12} italic color={UI.textMuted}>
          The Tokio bus is the truth — topics, IDs, latencies, the
          system itself, not just a log.
        </Serif>
        <div style={{ marginTop: 12 }}>
          <Alert variant="warn">
            <span style={{ fontWeight: 600 }}>Backend gap.</span> The
            runtime exposes per-channel WebSocket streams (
            <code style={{ fontFamily: MONO }}>/ws/assistant/:session</code>,{" "}
            <code style={{ fontFamily: MONO }}>/ws/review</code>) but a
            generic <code style={{ fontFamily: MONO }}>/ws/bus</code>{" "}
            firehose for "every envelope on every topic" isn't
            implemented yet. Wiring it is a small handler in{" "}
            <code style={{ fontFamily: MONO }}>ordo-control</code> that
            subscribes to <code style={{ fontFamily: MONO }}>"ordo.*"</code>{" "}
            and forwards to the WS.
          </Alert>
        </div>
        <div style={{ marginTop: 12 }}>
          <Mono size={10} upper track="0.15em" color={UI.textDim}>
            What works today
          </Mono>
          <ul
            style={{
              marginTop: 8,
              paddingLeft: 18,
              color: UI.parchment,
              listStyle: "disc",
            }}
          >
            <li style={{ marginBottom: 4 }}>
              <Mono size={11} color={UI.parchment}>
                Assistant
              </Mono>{" "}
              <Serif size={12} italic color={UI.textMuted}>
                tab streams per-turn events while a turn is in flight.
              </Serif>
            </li>
            <li style={{ marginBottom: 4 }}>
              <Mono size={11} color={UI.parchment}>
                Review
              </Mono>{" "}
              <Serif size={12} italic color={UI.textMuted}>
                tab polls /api/review/pending every 5 s (websocket
                upgrade is a one-line swap).
              </Serif>
            </li>
            <li>
              <Mono size={11} color={UI.parchment}>
                Security
              </Mono>{" "}
              <Serif size={12} italic color={UI.textMuted}>
                tab shows the audit-ring tail — the structured slice of
                bus traffic that crossed a gate.
              </Serif>
            </li>
          </ul>
        </div>
      </Card>
    </div>
  );
};

// ─── Shell ───
const formatChatTimestamp = (date: Date): string =>
  date.toLocaleString([], {
    month: "short",
    day: "2-digit",
    hour: "2-digit",
    minute: "2-digit",
  });

const tsNow = (): string => formatChatTimestamp(new Date());

// Persistent chat-session key. Stored in localStorage so reloading
// the app or reopening the Tauri window picks the conversation back
// up where it left off. The runtime persists sessions in SQLite,
// so a stored id survives runtime restarts; a 404 on rehydrate just
// means the row was pruned and we start fresh.
const SESSION_ID_KEY = "ordo:chat_session_id";
const ORDO_THEME_KEY = "ordo:theme";

// Active mode-scoped workspace key. The picker writes here; the
// next session-creation reads it to bind the new session to the
// chosen mode. Mode is FIXED per session (architectural promise),
// so changing this only affects new sessions — never rewrites the
// stored session's mode field.
const ACTIVE_MODE_KEY = "ordo:active_mode";
const FALLBACK_MODE_ID = "general";
const MODE_UI_SETTINGS_KEY = "ordo:mode_ui_settings";
const OS_SPECIALIST_MODE_IDS = new Set([
  "windows_tech_specialist",
  "linux_tech_specialist",
  "apple_os_tech_specialist",
]);
const modeDefaultsEnabled = (modeId: string): boolean => !OS_SPECIALIST_MODE_IDS.has(modeId);
const isTemporarySpecialistMode = (modeId: string): boolean => OS_SPECIALIST_MODE_IDS.has(modeId);

const readStoredTheme = (): OrdoTheme => {
  if (typeof window === "undefined") return "dark";
  try {
    return window.localStorage.getItem(ORDO_THEME_KEY) === "bright" ? "bright" : "dark";
  } catch {
    return "dark";
  }
};

const persistTheme = (theme: OrdoTheme) => {
  if (typeof window === "undefined") return;
  window.localStorage.setItem(ORDO_THEME_KEY, theme);
};

interface OptionalRagSetting {
  enabled: boolean;
  storageLimitMb: number;
}

type ModeCollaborationPolicy = "off" | "suggest" | "ask" | "auto";

interface ModeCollaborationSetting {
  policy: ModeCollaborationPolicy;
  allowedModeIds: string[];
  allowSubagents: boolean;
  maxCollaborators: number;
}

interface ModeUiSetting {
  enabled: boolean;
  ragLimitMb: number;
  allowCloudModels?: boolean;
  optionalRags?: Record<string, OptionalRagSetting>;
  collaboration?: ModeCollaborationSetting;
}

interface RagCatalogEntry {
  id: string;
  label: string;
  groups: string[];
}

const OPTIONAL_RAG_GROUPS: Array<{ group: string; items: string[] }> = [
  {
    group: "Science",
    items: ["Physics", "Chemistry", "Biology", "Earth Science", "Space Science", "Mathematics", "Formal Science"],
  },
  {
    group: "Physics",
    items: [
      "Classical Mechanics", "Thermodynamics", "Statistical Mechanics", "Fluid Dynamics", "Acoustics",
      "Electromagnetism", "Optics", "Photonics", "Quantum Mechanics", "Quantum Field Theory",
      "Special Relativity", "General Relativity", "Particle Physics", "Nuclear Physics",
      "Atomic & Molecular Physics", "Condensed Matter Physics", "Solid State Physics",
      "Plasma Physics", "Astrophysics", "Cosmology", "Biophysics", "Geophysics", "Computational Physics",
    ],
  },
  {
    group: "Chemistry",
    items: [
      "Organic Chemistry", "Inorganic Chemistry", "Physical Chemistry", "Analytical Chemistry",
      "Biochemistry", "Electrochemistry", "Photochemistry", "Thermochemistry", "Quantum Chemistry",
      "Polymer Chemistry", "Materials Chemistry", "Organometallic Chemistry", "Medicinal Chemistry",
      "Nuclear Chemistry", "Environmental Chemistry", "Green Chemistry", "Surface Chemistry",
      "Spectroscopy", "Chromatography", "Crystallography", "Stoichiometry", "Chemical Kinetics", "Catalysis",
    ],
  },
  {
    group: "Biology",
    items: [
      "Molecular Biology", "Cell Biology", "Genetics", "Genomics", "Microbiology", "Virology",
      "Bacteriology", "Mycology", "Parasitology", "Immunology", "Biochemistry", "Biotechnology",
      "Bioinformatics", "Ecology", "Evolutionary Biology", "Developmental Biology", "Anatomy",
      "Physiology", "Neuroscience", "Botany", "Zoology", "Marine Biology", "Entomology",
      "Ornithology", "Paleontology", "Taxonomy", "Epidemiology",
    ],
  },
  {
    group: "Earth Science",
    items: [
      "Geology", "Mineralogy", "Petrology", "Volcanology", "Seismology", "Geomorphology",
      "Hydrology", "Glaciology", "Meteorology", "Climatology", "Oceanography", "Atmospheric Science",
      "Soil Science", "Geochemistry", "Geophysics", "Geodesy",
    ],
  },
  {
    group: "Space Science",
    items: [
      "Astronomy", "Astrophysics", "Cosmology", "Planetary Science", "Stellar Astronomy",
      "Galactic Astronomy", "Exoplanetology", "Astrobiology", "Heliophysics", "Aeronautics",
      "Astronautics", "Orbital Mechanics",
    ],
  },
  {
    group: "Mathematics",
    items: [
      "Arithmetic", "Algebra", "Linear Algebra", "Abstract Algebra", "Geometry", "Trigonometry",
      "Calculus", "Differential Equations", "Real Analysis", "Complex Analysis", "Number Theory",
      "Topology", "Combinatorics", "Discrete Mathematics", "Graph Theory", "Probability",
      "Statistics", "Set Theory", "Mathematical Logic", "Numerical Analysis", "Optimization",
      "Game Theory", "Cryptography (mathematical)", "Category Theory",
    ],
  },
  {
    group: "Technology / Engineering",
    items: [
      "Computer Science", "Software Engineering", "Electrical Engineering", "Electronics",
      "Communications", "Mechanical Engineering", "Civil Engineering", "Chemical Engineering",
      "Aerospace Engineering", "Biomedical Engineering", "Materials Science", "Energy",
      "Robotics", "Manufacturing",
    ],
  },
  {
    group: "Computer Science",
    items: [
      "Algorithms", "Data Structures", "Computational Complexity", "Operating Systems",
      "Computer Architecture", "Networking", "Databases", "Distributed Systems",
      "Concurrency & Parallelism", "Programming Languages", "Compilers", "Formal Methods",
      "Computer Graphics", "Computer Vision", "Machine Learning", "Deep Learning",
      "Natural Language Processing", "Reinforcement Learning", "Artificial Intelligence",
      "Cryptography", "Cybersecurity", "Human-Computer Interaction", "Embedded Systems",
      "Quantum Computing", "Bioinformatics",
    ],
  },
  {
    group: "Electrical Engineering",
    items: [
      "Circuit Theory", "Analog Electronics", "Digital Electronics", "Power Systems",
      "Power Electronics", "Control Systems", "Signal Processing", "Digital Signal Processing",
      "Microelectronics", "VLSI Design", "Semiconductors", "Instrumentation", "Electromagnetics", "Photovoltaics",
    ],
  },
  {
    group: "Communications",
    items: [
      "Telecommunications", "RF (Radio Frequency) Engineering", "Antenna Theory", "Wave Propagation",
      "Microwave Engineering", "Modulation (AM/FM/PM/QAM)", "Signal Modulation & Demodulation",
      "Information Theory", "Coding Theory", "Error Correction", "Spread Spectrum",
      "Wireless Communications", "Cellular Networks (3G/4G/5G/6G)", "Satellite Communications",
      "Optical Fiber Communications", "Network Protocols", "Software-Defined Radio", "Radar",
      "Sonar", "Spectrum Management", "EMC / EMI (Electromagnetic Compatibility/Interference)",
    ],
  },
  {
    group: "RF / Signals",
    items: [
      "Transmission Lines", "Waveguides", "Impedance Matching", "Filters (RF/microwave)",
      "Mixers & Amplifiers", "Oscillators", "Phase-Locked Loops", "Frequency Synthesis",
      "Signal Intelligence (SIGINT)", "Electronic Warfare", "GNSS / GPS", "Doppler & Ranging", "Spectral Analysis",
    ],
  },
  {
    group: "Mechanical Engineering",
    items: [
      "Statics", "Dynamics", "Kinematics", "Thermodynamics (applied)", "Heat Transfer",
      "Fluid Mechanics", "Mechatronics", "Tribology", "Machine Design", "HVAC",
      "Combustion", "Acoustics & Vibration",
    ],
  },
  {
    group: "Civil Engineering",
    items: [
      "Structural Engineering", "Geotechnical Engineering", "Transportation Engineering",
      "Hydraulic Engineering", "Environmental Engineering", "Surveying", "Construction Management",
    ],
  },
  {
    group: "Chemical Engineering",
    items: [
      "Process Engineering", "Reaction Engineering", "Thermodynamics (chemical)",
      "Transport Phenomena", "Separation Processes", "Petrochemicals", "Catalysis (applied)",
    ],
  },
  {
    group: "Aerospace Engineering",
    items: ["Aerodynamics", "Propulsion", "Avionics", "Flight Dynamics", "Structures", "Astronautics"],
  },
  {
    group: "Materials Science",
    items: ["Metallurgy", "Ceramics", "Polymers", "Composites", "Nanomaterials", "Semiconductors", "Crystallography", "Corrosion"],
  },
  {
    group: "Energy",
    items: [
      "Nuclear Engineering", "Renewable Energy", "Solar", "Wind", "Hydropower", "Geothermal",
      "Fossil Fuels", "Energy Storage", "Battery Technology", "Smart Grid",
    ],
  },
  {
    group: "Medicine & Health",
    items: [
      "Internal Medicine", "Surgery", "Anatomy", "Physiology", "Pathology", "Pharmacology",
      "Toxicology", "Immunology", "Cardiology", "Neurology", "Oncology", "Endocrinology",
      "Gastroenterology", "Nephrology", "Pulmonology", "Hematology", "Dermatology",
      "Pediatrics", "Geriatrics", "Obstetrics & Gynecology", "Psychiatry", "Radiology",
      "Anesthesiology", "Emergency Medicine", "Public Health", "Epidemiology", "Nutrition",
      "Dentistry", "Veterinary Medicine",
    ],
  },
  {
    group: "Pharmacology",
    items: [
      "Pharmacokinetics", "Pharmacodynamics", "Clinical Pharmacology", "Neuropharmacology",
      "Psychopharmacology", "Cardiovascular Pharmacology", "Chemotherapy", "Toxicology",
      "Pharmacognosy", "Drug Design", "Pharmaceutics", "Pharmacogenomics", "Posology (dosing)",
    ],
  },
  {
    group: "Social Science",
    items: [
      "Psychology", "Cognitive Science", "Sociology", "Anthropology", "Archaeology",
      "Economics", "Political Science", "International Relations", "Law", "Criminology",
      "Geography (human)", "Linguistics", "Education", "Communication Studies",
    ],
  },
  {
    group: "Humanities",
    items: [
      "History", "Philosophy", "Logic", "Ethics", "Religion / Theology", "Literature",
      "Linguistics", "Languages", "Art History", "Music Theory", "Performing Arts",
      "Architecture", "Cultural Studies",
    ],
  },
  {
    group: "Business & Applied",
    items: ["Accounting", "Finance", "Marketing", "Management", "Operations / Supply Chain", "Entrepreneurship", "Project Management", "Statistics (applied)"],
  },
];

const slugId = (value: string): string =>
  value
    .toLowerCase()
    .replace(/&/g, "and")
    .replace(/[^a-z0-9]+/g, "_")
    .replace(/^_+|_+$/g, "");

const OPTIONAL_RAG_CATALOG: RagCatalogEntry[] = (() => {
  const map = new Map<string, RagCatalogEntry>();
  for (const group of OPTIONAL_RAG_GROUPS) {
    for (const label of group.items) {
      const id = `rag.${slugId(label)}`;
      const existing = map.get(id);
      if (existing) {
        if (!existing.groups.includes(group.group)) existing.groups.push(group.group);
      } else {
        map.set(id, { id, label, groups: [group.group] });
      }
    }
  }
  return [...map.values()].sort((a, b) => a.label.localeCompare(b.label));
})();

const optionalRagLabel = (id: string): string =>
  OPTIONAL_RAG_CATALOG.find((entry) => entry.id === id)?.label ?? id.replace(/^rag\./, "");

const enabledOptionalRags = (setting: ModeUiSetting): Array<RagCatalogEntry & OptionalRagSetting> =>
  Object.entries(setting.optionalRags ?? {})
    .filter(([, rag]) => rag.enabled && rag.storageLimitMb > 0)
    .map(([id, rag]) => {
      const catalog = OPTIONAL_RAG_CATALOG.find((entry) => entry.id === id) ?? {
        id,
        label: optionalRagLabel(id),
        groups: [],
      };
      return { ...catalog, ...rag };
    });

const DEFAULT_MODE_COLLABORATION: ModeCollaborationSetting = {
  policy: "suggest",
  allowedModeIds: [],
  allowSubagents: false,
  maxCollaborators: 1,
};

const modeCollaborationSetting = (setting: ModeUiSetting): ModeCollaborationSetting => ({
  ...DEFAULT_MODE_COLLABORATION,
  ...(setting.collaboration ?? {}),
  allowedModeIds: Array.isArray(setting.collaboration?.allowedModeIds)
    ? setting.collaboration.allowedModeIds
    : [],
  maxCollaborators: Math.max(1, Math.min(5, setting.collaboration?.maxCollaborators ?? 1)),
});

const loadModeUiSettings = (): Record<string, ModeUiSetting> => {
  if (typeof window === "undefined") return {};
  try {
    const raw = window.localStorage.getItem(MODE_UI_SETTINGS_KEY);
    if (!raw) return {};
    const parsed = JSON.parse(raw);
    return typeof parsed === "object" && parsed !== null ? parsed as Record<string, ModeUiSetting> : {};
  } catch {
    return {};
  }
};

const saveModeUiSettings = (settings: Record<string, ModeUiSetting>) => {
  if (typeof window === "undefined") return;
  window.localStorage.setItem(MODE_UI_SETTINGS_KEY, JSON.stringify(settings));
};

const modeUiSetting = (
  settings: Record<string, ModeUiSetting>,
  mode: AssistantMode,
): ModeUiSetting => ({
  enabled: settings[mode.id]?.enabled ?? modeDefaultsEnabled(mode.id),
  ragLimitMb: settings[mode.id]?.ragLimitMb ?? (mode.id === "dreaming" ? 2048 : mode.id === "diagnostic" ? 1024 : 512),
  allowCloudModels: settings[mode.id]?.allowCloudModels ?? false,
  optionalRags: settings[mode.id]?.optionalRags ?? {},
  collaboration: settings[mode.id]?.collaboration,
});

const readStoredSessionId = (): string | undefined => {
  if (typeof window === "undefined") return undefined;
  try {
    const raw = window.localStorage.getItem(SESSION_ID_KEY);
    return raw && raw.length > 0 ? raw : undefined;
  } catch {
    return undefined;
  }
};

const readStoredActiveMode = (): string => {
  if (typeof window === "undefined") return FALLBACK_MODE_ID;
  try {
    const raw = window.localStorage.getItem(ACTIVE_MODE_KEY);
    return raw && raw.length > 0 ? raw : FALLBACK_MODE_ID;
  } catch {
    return FALLBACK_MODE_ID;
  }
};

// Format a server-side ISO timestamp to the shell's HH:MM display.
// Falls back to tsNow() if the string can't be parsed — defensive
// against schema drift.
const tsFromIso = (iso: string): string => {
  try {
    const d = new Date(iso);
    if (Number.isNaN(d.getTime())) return tsNow();
    return formatChatTimestamp(d);
  } catch {
    return tsNow();
  }
};

// Module-level — used by the inspector panel and the WS event
// effect. 50 entries is enough to cover a substantive operator
// review session without being a memory hog.
const MODE_EVENT_LOG_CAP = 50;

interface ModeEventLogEntry {
  /** One of: mode_bound, mode_memory_scope_applied,
   *  mode_tool_filter_applied, cross_mode_consult_requested,
   *  cross_mode_consult_approved, cross_mode_consult_denied,
   *  cross_mode_consult_completed. */
  kind: string;
  /** HH:MM display. */
  ts: string;
  /** Pre-rendered single-line summary. */
  summary: string;
  /** Raw event for the operator who clicks "show raw". */
  raw: TurnEvent;
}

// One-liner summary for a mode-related TurnEvent. Used by the
// inspector panel's recent-activity log so the operator can see a
// quick "what happened, when" without expanding the raw JSON.
const renderModeEventSummary = (e: TurnEvent): string => {
  switch (e.event) {
    case "mode_bound":
      return `bound to ${(e as { mode_label?: string }).mode_label ?? "?"} (${
        (e as { allowed_tool_lane_count?: number }).allowed_tool_lane_count ?? 0
      } lanes, ${(e as { rag_domains?: string[] }).rag_domains?.length ?? 0} RAG domains)`;
    case "mode_memory_scope_applied":
      return `recall returned ${
        (e as { facts_visible?: number }).facts_visible ?? 0
      } facts in ${
        (e as { visible_scopes?: string[] }).visible_scopes?.join(",") ?? "?"
      }`;
    case "mode_tool_filter_applied":
      return `tool filter dropped ${
        (e as { filtered_count?: number }).filtered_count ?? 0
      } caps (kept ${(e as { kept_capabilities?: number }).kept_capabilities ?? 0})`;
    case "cross_mode_consult_requested": {
      const r = e as { active_mode?: string; target_mode?: string; reason?: string };
      return `consult requested: ${r.active_mode} -> ${r.target_mode} (${r.reason ?? ""})`;
    }
    case "cross_mode_consult_approved": {
      const r = e as { active_mode?: string; target_mode?: string };
      return `consult approved: ${r.active_mode} -> ${r.target_mode}`;
    }
    case "cross_mode_consult_denied": {
      const r = e as { active_mode?: string; target_mode?: string; reason?: string };
      return `consult denied: ${r.active_mode} -> ${r.target_mode} (${r.reason ?? ""})`;
    }
    case "cross_mode_consult_completed": {
      const r = e as { active_mode?: string; target_mode?: string; turn_id?: string };
      return `consult completed: ${r.active_mode} <- ${r.target_mode} (${r.turn_id ?? "turn"})`;
    }
    default:
      return e.event;
  }
};

// Render the persisted Turn list as ChatMessage[]. Each Turn becomes
// a (user, assistant) pair preserving the original timestamp. The
// model name (when present) goes into meta so the dock pill renders.
const turnsToChatMessages = (turns: AssistantTurnRecord[]): ChatMessage[] => {
  const out: ChatMessage[] = [];
  for (const t of turns) {
    const ts = tsFromIso(t.created_at);
    out.push({ role: "user", text: t.user_message, ts });
    const meta: string[] = [];
    if (t.model) meta.push(t.model);
    if (t.credential_service) meta.push(t.credential_service);
    out.push({
      role: "assistant",
      text: t.assistant_response,
      ts,
      meta: meta.length > 0 ? meta : undefined,
    });
  }
  return out;
};

const sessionOptionLabel = (
  session: AssistantSessionRecord,
  modes: AssistantMode[],
): string => {
  const modeLabel =
    modes.find((mode) => mode.id === session.mode)?.label ??
    session.mode ??
    "general";
  const title = session.title?.trim() || `Session ${session.id.slice(0, 8)}`;
  const updated = session.updated_at ?? session.created_at;
  let when = "recent";
  try {
    const d = new Date(updated);
    if (!Number.isNaN(d.getTime())) {
      when = d.toLocaleDateString(undefined, {
        month: "short",
        day: "numeric",
      });
    }
  } catch {
    when = "recent";
  }
  const turns =
    typeof session.turn_count === "number"
      ? `${session.turn_count} turn${session.turn_count === 1 ? "" : "s"}`
      : "stored";
  return `${title} - ${modeLabel} - ${when} - ${turns}`;
};

const ModesSurface = ({
  modes,
  activeMode,
  settings,
  onModeChange,
  onSettingsChange,
}: {
  modes: AssistantMode[];
  activeMode: string;
  settings: Record<string, ModeUiSetting>;
  onModeChange: (mode: string) => void;
  onSettingsChange: (modeId: string, patch: Partial<ModeUiSetting>) => void;
}) => {
  const [ragSearch, setRagSearch] = useState("");
  const [ragPicker, setRagPicker] = useState<Record<string, string>>({});
  const visibleModes =
    modes.length > 0
      ? modes
      : [
          {
            id: FALLBACK_MODE_ID,
            label: "General Assistant",
            description: "Default Ordo assistant workspace.",
            memory_scope: ["global"],
            rag_domains: [],
            allowed_tool_lanes: [],
            blocked_tool_capabilities: [],
            policies: [],
            planner_bias: [],
            persona: [],
          },
        ];
  const catalogOptions = useMemo(() => {
    const q = ragSearch.trim().toLowerCase();
    const matches = OPTIONAL_RAG_CATALOG.filter((entry) => {
      if (!q) return true;
      return (
        entry.label.toLowerCase().includes(q) ||
        entry.groups.some((group) => group.toLowerCase().includes(q))
      );
    });
    return matches.slice(0, 120);
  }, [ragSearch]);
  const setOptionalRag = (
    modeId: string,
    ui: ModeUiSetting,
    ragId: string,
    next: OptionalRagSetting | null,
  ) => {
    const optionalRags = { ...(ui.optionalRags ?? {}) };
    if (next) {
      optionalRags[ragId] = next;
    } else {
      delete optionalRags[ragId];
    }
    onSettingsChange(modeId, { optionalRags });
  };
  const addOptionalRag = (modeId: string, ui: ModeUiSetting, ragId: string) => {
    if (!ragId) return;
    setOptionalRag(modeId, ui, ragId, { enabled: false, storageLimitMb: 0 });
    setRagPicker((prev) => ({ ...prev, [modeId]: "" }));
  };
  const setCollaboration = (
    modeId: string,
    ui: ModeUiSetting,
    patch: Partial<ModeCollaborationSetting>,
  ) => {
    onSettingsChange(modeId, {
      collaboration: {
        ...modeCollaborationSetting(ui),
        ...patch,
      },
    });
  };
  const toggleCollaborator = (modeId: string, ui: ModeUiSetting, collaboratorId: string) => {
    const collaboration = modeCollaborationSetting(ui);
    const allowedModeIds = collaboration.allowedModeIds.includes(collaboratorId)
      ? collaboration.allowedModeIds.filter((id) => id !== collaboratorId)
      : [...collaboration.allowedModeIds, collaboratorId];
    setCollaboration(modeId, ui, { allowedModeIds });
  };
  return (
    <div className="h-full flex flex-col gap-4 overflow-auto pb-4">
      <SurfaceTitle
        kicker="ordo - modes"
        title="Mode management"
        sub="Turn modes on or off, choose the foreground mode, and opt into extra RAG domains only when a mode needs them."
      />
      <Card>
        <div className="flex items-start justify-between gap-4">
          <div>
            <Mono size={11} upper track="0.18em" color={UI.textMuted}>
              Optional RAG catalog
            </Mono>
            <div style={{ marginTop: 5 }}>
              <Serif size={13} italic color={UI.textMuted}>
                Extra knowledge domains are disabled by default with 0 MB reserved. Selecting one adds it to a mode, but it stays cold until you enable it and assign storage.
              </Serif>
            </div>
          </div>
          <Badge variant="neutral">{OPTIONAL_RAG_CATALOG.length} domains</Badge>
        </div>
        <div style={{ marginTop: 12 }}>
          <TextInput
            value={ragSearch}
            onChange={setRagSearch}
            placeholder="Search optional RAGs by field or group..."
          />
        </div>
      </Card>
      <div className="grid gap-3" style={{ gridTemplateColumns: "repeat(auto-fit, minmax(340px, 1fr))" }}>
        {visibleModes.map((mode) => {
          const ui = modeUiSetting(settings, mode);
          const active = activeMode === mode.id;
          const temporarySpecialist = isTemporarySpecialistMode(mode.id);
          const activeOptionalRags = enabledOptionalRags(ui);
          const selectedOptionalRags = Object.entries(ui.optionalRags ?? {});
          const collaboration = modeCollaborationSetting(ui);
          const collaboratorModes = visibleModes.filter((candidate) => candidate.id !== mode.id);
          return (
            <Card key={mode.id}>
              <div className="flex items-start justify-between gap-3">
                <div style={{ minWidth: 0 }}>
                  <div style={{ fontFamily: FRAUNCES, fontSize: 18, fontWeight: 650, color: UI.parchment }}>
                    {mode.label}
                  </div>
                  <Mono size={10} color={UI.textMuted}>
                    {mode.id}
                  </Mono>
                </div>
                <div className="flex items-center gap-1.5">
                  {active && <Badge variant="primary">active</Badge>}
                  {!ui.enabled && <Badge variant="warn">paused</Badge>}
                  {temporarySpecialist && <Badge variant="neutral">locked built-in</Badge>}
                  {temporarySpecialist && <Badge variant="info">auto-off</Badge>}
                </div>
              </div>
                  <div style={{ marginTop: 10 }}>
                    <Serif size={13} italic color={UI.textMuted}>
                      {mode.description || "No description provided."}
                    </Serif>
                  </div>
              {temporarySpecialist && (
                <div style={{ marginTop: 12 }}>
                  <Alert variant="warn">
                    Temporary OS specialist mode. It starts paused, requires operator permission gating for local actions,
                    and Ordo returns it to off after the active task turn completes.
                  </Alert>
                </div>
              )}
              <div className="grid grid-cols-2 gap-2" style={{ marginTop: 14 }}>
                <Button
                  size="sm"
                  variant={active ? "primary" : "secondary"}
                  disabled={active || !ui.enabled}
                  onClick={() => onModeChange(mode.id)}
                >
                  {active ? "Current" : "Use Mode"}
                </Button>
                <Button
                  size="sm"
                  variant={ui.enabled ? "secondary" : "primary"}
                  onClick={() => onSettingsChange(mode.id, { enabled: !ui.enabled })}
                >
                  <span style={{ display: "inline-flex", alignItems: "center", gap: 5 }}>
                    {ui.enabled ? <Pause size={12} /> : <Play size={12} />}
                    {ui.enabled ? "Pause" : "Resume"}
                  </span>
                </Button>
              </div>
              <div style={{ marginTop: 14 }}>
                <Field label="Base RAG storage budget" hint={`${ui.ragLimitMb} MB reserved for this mode's default retrieved context.`}>
                  <input
                    type="range"
                    min={0}
                    max={8192}
                    step={128}
                    value={ui.ragLimitMb}
                    onChange={(event) =>
                      onSettingsChange(mode.id, { ragLimitMb: Number(event.target.value) })
                    }
                    style={{ width: "100%" }}
                  />
                </Field>
              </div>
              <div className="flex flex-wrap gap-1.5" style={{ marginTop: 12 }}>
                {(mode.rag_domains ?? []).slice(0, 3).map((domain) => (
                  <Badge key={domain} variant="info">{domain}</Badge>
                ))}
                {(mode.allowed_tool_lanes ?? []).slice(0, 3).map((lane) => (
                  <Badge key={lane} variant="neutral">{lane}</Badge>
                ))}
              </div>
              <div style={{ marginTop: 16, paddingTop: 14, borderTop: `1px solid ${UI.cardBorder}` }}>
                <Field label="Add optional RAG" hint="Added domains start disabled at 0 MB, so they do not index or consume storage until enabled.">
                  <div className="flex gap-2">
                    <Select
                      value={ragPicker[mode.id] ?? ""}
                      onChange={(value) => setRagPicker((prev) => ({ ...prev, [mode.id]: value }))}
                      options={[
                        { value: "", label: "Choose a domain..." },
                        ...catalogOptions
                          .filter((entry) => !(ui.optionalRags ?? {})[entry.id])
                          .map((entry) => ({
                            value: entry.id,
                            label: `${entry.label} - ${entry.groups.slice(0, 2).join(" / ")}`,
                          })),
                      ]}
                    />
                    <Button
                      size="sm"
                      disabled={!ragPicker[mode.id]}
                      onClick={() => addOptionalRag(mode.id, ui, ragPicker[mode.id] ?? "")}
                    >
                      Add
                    </Button>
                  </div>
                </Field>
                <div className="space-y-2" style={{ marginTop: 10 }}>
                  {selectedOptionalRags.length === 0 && (
                    <Mono size={10} color={UI.textDim}>
                      No optional RAGs assigned.
                    </Mono>
                  )}
                  {selectedOptionalRags.map(([ragId, rag]) => {
                    const label = optionalRagLabel(ragId);
                    return (
                      <div
                        key={ragId}
                        className="flex items-center gap-2"
                        style={{
                          padding: "9px 10px",
                          borderRadius: 8,
                          border: `1px solid ${rag.enabled ? UI.primaryBorder : UI.cardBorder}`,
                          background: rag.enabled ? UI.primarySoft : UI.cardBgRaised,
                        }}
                      >
                        <div style={{ minWidth: 0, flex: 1 }}>
                          <Mono size={10} color={rag.enabled ? UI.primary : UI.parchment} weight={600}>
                            {label}
                          </Mono>
                          <div style={{ marginTop: 2 }}>
                            <Mono size={9} color={UI.textDim}>
                              {rag.enabled ? `${rag.storageLimitMb} MB enabled` : "disabled - 0 MB"}
                            </Mono>
                          </div>
                        </div>
                        <ToggleSwitch
                          checked={rag.enabled}
                          onChange={(enabled) =>
                            setOptionalRag(mode.id, ui, ragId, {
                              enabled,
                              storageLimitMb: enabled ? Math.max(rag.storageLimitMb, 256) : 0,
                            })
                          }
                        />
                        <input
                          type="number"
                          value={rag.storageLimitMb}
                          min={0}
                          max={8192}
                          step={128}
                          disabled={!rag.enabled}
                          onChange={(event) => {
                            const value = Number(event.target.value);
                            setOptionalRag(mode.id, ui, ragId, {
                              enabled: value > 0,
                              storageLimitMb: Math.max(0, value),
                            });
                          }}
                          style={{
                            width: 92,
                            height: 34,
                            borderRadius: 8,
                            border: `1px solid ${UI.inputBorder}`,
                            background: UI.inputBg,
                            color: UI.parchment,
                            padding: "0 8px",
                            fontFamily: MONO,
                            fontSize: 11,
                            opacity: rag.enabled ? 1 : 0.45,
                          }}
                        />
                        <Button size="sm" onClick={() => setOptionalRag(mode.id, ui, ragId, null)}>
                          <Trash2 size={12} />
                        </Button>
                      </div>
                    );
                  })}
                </div>
                {activeOptionalRags.length > 0 && (
                  <div className="flex flex-wrap gap-1.5" style={{ marginTop: 10 }}>
                    {activeOptionalRags.slice(0, 6).map((rag) => (
                      <Badge key={rag.id} variant="success">{rag.label}</Badge>
                    ))}
                  </div>
                )}
              </div>
              <div style={{ marginTop: 16, paddingTop: 14, borderTop: `1px solid ${UI.cardBorder}` }}>
                <div className="flex items-start justify-between gap-3">
                  <div>
                    <Mono size={10} upper track="0.18em" color={UI.textMuted}>
                      collaboration
                    </Mono>
                    <div style={{ marginTop: 4 }}>
                      <Serif size={12} italic color={UI.textMuted}>
                        Surgical cross-mode expertise. The active mode never reads another mode's RAG or memory; it consults that mode's agent and receives only the agent's answer.
                      </Serif>
                    </div>
                  </div>
                  <Badge variant={collaboration.policy === "off" ? "neutral" : "info"}>
                    {collaboration.policy}
                  </Badge>
                </div>
                <div className="grid grid-cols-2 gap-2" style={{ marginTop: 12 }}>
                  <Field label="Policy" hint="Suggest recommends; Ask requires approval; Auto consult is only for trusted allowlists.">
                    <Select<ModeCollaborationPolicy>
                      value={collaboration.policy}
                      onChange={(policy) => setCollaboration(mode.id, ui, { policy })}
                      options={[
                        { value: "off", label: "Off" },
                        { value: "suggest", label: "Suggest only" },
                        { value: "ask", label: "Ask before consult" },
                        { value: "auto", label: "Auto for allowlist" },
                      ]}
                    />
                  </Field>
                  <Field label="Max collaborators" hint="Keeps collaboration narrow.">
                    <input
                      type="number"
                      min={1}
                      max={5}
                      step={1}
                      value={collaboration.maxCollaborators}
                      onChange={(event) =>
                        setCollaboration(mode.id, ui, {
                          maxCollaborators: Math.max(1, Math.min(5, Number(event.target.value))),
                        })
                      }
                      style={{
                        width: "100%",
                        height: 36,
                        borderRadius: 8,
                        border: `1px solid ${UI.inputBorder}`,
                        background: UI.inputBg,
                        color: UI.parchment,
                        padding: "0 10px",
                        fontFamily: MONO,
                        fontSize: 11,
                      }}
                    />
                  </Field>
                </div>
                <div className="flex items-center justify-between gap-3" style={{ marginTop: 10 }}>
                  <Mono size={10} color={UI.textMuted}>
                    Allow subagents for collaboration
                  </Mono>
                  <ToggleSwitch
                    checked={collaboration.allowSubagents}
                    onChange={(allowSubagents) => setCollaboration(mode.id, ui, { allowSubagents })}
                  />
                </div>
                <div style={{ marginTop: 12 }}>
                  <Mono size={10} upper track="0.16em" color={UI.textDim}>
                    allowed collaborators
                  </Mono>
                  <div className="flex flex-wrap gap-1.5" style={{ marginTop: 8 }}>
                    {collaboratorModes.length === 0 && (
                      <Mono size={10} color={UI.textDim}>No other modes registered.</Mono>
                    )}
                    {collaboratorModes.map((candidate) => {
                      const selected = collaboration.allowedModeIds.includes(candidate.id);
                      return (
                        <button
                          key={candidate.id}
                          type="button"
                          onClick={() => toggleCollaborator(mode.id, ui, candidate.id)}
                          style={{
                            borderRadius: 999,
                            border: `1px solid ${selected ? UI.primaryBorder : UI.cardBorder}`,
                            background: selected ? UI.primarySoft : UI.cardBgRaised,
                            color: selected ? UI.primary : UI.textMuted,
                            padding: "5px 9px",
                            fontFamily: MONO,
                            fontSize: 10,
                            cursor: "pointer",
                          }}
                        >
                          {candidate.label}
                        </button>
                      );
                    })}
                  </div>
                </div>
              </div>
            </Card>
          );
        })}
      </div>
    </div>
  );
};

const ToggleSwitch = ({ checked, onChange }: { checked: boolean; onChange: (value: boolean) => void }) => (
  <button
    type="button"
    onClick={() => onChange(!checked)}
    style={{
      width: 42,
      height: 24,
      borderRadius: 999,
      border: `1px solid ${checked ? UI.primaryBorder : UI.cardBorderStrong}`,
      background: checked ? UI.primary : "rgba(255,255,255,0.08)",
      position: "relative",
      cursor: "pointer",
      flexShrink: 0,
    }}
  >
    <span
      style={{
        position: "absolute",
        width: 18,
        height: 18,
        borderRadius: 999,
        background: checked ? UI.ink : UI.parchment,
        left: checked ? 20 : 3,
        top: 2,
        transition: "left 0.15s",
      }}
    />
  </button>
);

const SettingsList = ({ children }: { children: ReactNode }) => (
  <Card padded={false} style={{ overflow: "hidden" }}>
    {children}
  </Card>
);

const SettingsRow = ({
  title,
  sub,
  control,
}: {
  title: string;
  sub?: ReactNode;
  control?: ReactNode;
}) => (
  <div
    className="flex items-start justify-between gap-4"
    style={{
      minHeight: 76,
      padding: "14px 16px",
      borderBottom: `1px solid ${UI.cardBorder}`,
      flexWrap: "wrap",
    }}
  >
    <div style={{ minWidth: 0, flex: "1 1 220px" }}>
      <Mono size={12} color={UI.parchment} weight={600}>
        {title}
      </Mono>
      {sub && (
        <div style={{ marginTop: 5 }}>
          <Mono size={11} color={UI.textMuted}>
            {sub}
          </Mono>
        </div>
      )}
    </div>
    {control && <div style={{ flex: "1 1 180px", minWidth: 0, maxWidth: "100%" }}>{control}</div>}
  </div>
);

const SmallSelect = <T extends string>({
  value,
  onChange,
  options,
}: {
  value: T;
  onChange: (value: T) => void;
  options: Array<{ value: T; label: string }>;
}) => (
  <Select value={value} onChange={onChange} options={options} />
);

const SimpleSettingsSurface = ({
  icon,
  title,
  sub,
  children,
}: {
  icon: ReactNode;
  title: string;
  sub: string;
  children: ReactNode;
}) => (
  <div className="h-full flex flex-col gap-4 overflow-auto pb-4">
    <SectionHeader icon={icon} title={title} sub={sub} />
    {children}
  </div>
);

const publishUxiDebugEvent = (
  source: string,
  action: string,
  message: string,
  detail: Record<string, unknown> = {},
  level: "INFO" | "WARN" | "ERROR" = "INFO",
) => {
  const entry = {
    id:
      typeof crypto !== "undefined" && "randomUUID" in crypto
        ? crypto.randomUUID()
        : `${Date.now().toString(36)}${Math.random().toString(36).slice(2, 6)}`,
    ts: new Date().toISOString(),
    level,
    action,
    message,
    ...detail,
  };
  if (typeof window !== "undefined") {
    window.dispatchEvent(
      new CustomEvent("ordo:debug-event", {
        detail: {
          source,
          topic: `${source}.${action}`,
          ...entry,
        },
      }),
    );
  }
  console.debug(`[${source}]`, entry);
};

const GeneralSettingsSurface = () => {
  const [workMode, setWorkMode] = useState<"coding" | "everyday">("coding");
  const [defaultPermissions, setDefaultPermissions] = useState(true);
  const [autoReview, setAutoReview] = useState(true);
  const [fullAccess, setFullAccess] = useState(true);
  const [destination, setDestination] = useState("visual-studio");
  const [environment, setEnvironment] = useState("windows-native");
  const [shell, setShell] = useState("powershell");
  const [language, setLanguage] = useState("auto");
  const [bottomPanel, setBottomPanel] = useState(true);
  const [speed, setSpeed] = useState("standard");
  const [suggestedPrompts, setSuggestedPrompts] = useState(true);
  const [turnNotify, setTurnNotify] = useState("unfocused");
  const [permissionNotify, setPermissionNotify] = useState(true);
  const [questionNotify, setQuestionNotify] = useState(true);
  const setTracked = <T,>(
    key: string,
    setter: (value: T) => void,
  ) => (value: T) => {
    setter(value);
    publishUxiDebugEvent("ordo.settings.general", "setting_changed", `${key} changed.`, { key, value });
  };
  return (
    <SimpleSettingsSurface
      icon={<SettingsIcon size={22} />}
      title="General"
      sub="Operator preferences for Ordo's default behavior, permissions, environment, and notifications."
    >
      <Card>
        <Mono size={11} upper track="0.18em" color={UI.textMuted}>Work mode</Mono>
        <div className="grid grid-cols-2 gap-3" style={{ marginTop: 12 }}>
          {[
            { id: "coding", label: "For coding", sub: "More technical responses and control" },
            { id: "everyday", label: "For everyday work", sub: "Same power, less technical detail" },
          ].map((mode) => (
            <button
              key={mode.id}
              type="button"
              onClick={() => setTracked("work_mode", setWorkMode)(mode.id as "coding" | "everyday")}
              style={{
                textAlign: "left",
                padding: 16,
                borderRadius: 10,
                border: `1px solid ${workMode === mode.id ? UI.primaryBorder : UI.cardBorder}`,
                background: workMode === mode.id ? UI.primarySoft : UI.cardBgRaised,
                cursor: "pointer",
              }}
            >
              <Mono size={13} color={UI.parchment} weight={600}>{mode.label}</Mono>
              <div style={{ marginTop: 6 }}><Mono size={11} color={UI.textMuted}>{mode.sub}</Mono></div>
            </button>
          ))}
        </div>
      </Card>
      <SettingsList>
        <SettingsRow title="Default permissions" sub="Ordo can read and edit files in the active workspace by default." control={<ToggleSwitch checked={defaultPermissions} onChange={setTracked("default_permissions", setDefaultPermissions)} />} />
        <SettingsRow title="Auto-review" sub="Review requests for additional access automatically before asking the operator." control={<ToggleSwitch checked={autoReview} onChange={setTracked("auto_review", setAutoReview)} />} />
        <SettingsRow title="Full access" sub="Allow full local file and command access for this project session." control={<ToggleSwitch checked={fullAccess} onChange={setTracked("full_access", setFullAccess)} />} />
      </SettingsList>
      <SettingsList>
        <SettingsRow title="Default open destination" sub="Where files and folders open by default." control={<SmallSelect value={destination} onChange={setTracked("default_open_destination", setDestination)} options={[{ value: "visual-studio", label: "Visual Studio" }, { value: "file-explorer", label: "File Explorer" }, { value: "system-default", label: "System default" }]} />} />
        <SettingsRow title="Agent environment" sub="Choose where the agent runs." control={<SmallSelect value={environment} onChange={setTracked("agent_environment", setEnvironment)} options={[{ value: "windows-native", label: "Windows native" }, { value: "wsl", label: "WSL" }, { value: "container", label: "Container" }]} />} />
        <SettingsRow title="Integrated terminal shell" sub="Choose which shell opens in the integrated terminal." control={<SmallSelect value={shell} onChange={setTracked("integrated_terminal_shell", setShell)} options={[{ value: "powershell", label: "PowerShell" }, { value: "cmd", label: "Command Prompt" }, { value: "bash", label: "Bash" }]} />} />
        <SettingsRow title="Language" sub="Language for the app UI." control={<SmallSelect value={language} onChange={setTracked("language", setLanguage)} options={[{ value: "auto", label: "Auto Detect" }, { value: "en", label: "English" }]} />} />
        <SettingsRow title="Bottom panel" sub="Show the bottom panel control in the app header." control={<ToggleSwitch checked={bottomPanel} onChange={setTracked("bottom_panel", setBottomPanel)} />} />
        <SettingsRow title="Speed" sub="Inference tier used across chats, subagents, and compaction." control={<SmallSelect value={speed} onChange={setTracked("speed", setSpeed)} options={[{ value: "standard", label: "Standard" }, { value: "fast", label: "Fast" }, { value: "deep", label: "Deep" }]} />} />
        <SettingsRow title="Code review" sub="Start review inline when possible, or launch a separate review chat." control={<Badge variant="info">Inline</Badge>} />
        <SettingsRow title="Suggested prompts" sub="Suggest what to do next from project files and connected apps." control={<ToggleSwitch checked={suggestedPrompts} onChange={setTracked("suggested_prompts", setSuggestedPrompts)} />} />
        <SettingsRow title="Import work from other AI apps" sub="Bring over setup, projects, and recent chats." control={<Button size="sm">Import</Button>} />
        <SettingsRow title="Open source licenses" sub="Third-party notices for bundled dependencies." control={<Button size="sm">View</Button>} />
      </SettingsList>
      <SettingsList>
        <SettingsRow title="Turn completion notifications" sub="Set when Ordo alerts you that a turn has finished." control={<SmallSelect value={turnNotify} onChange={setTracked("turn_completion_notifications", setTurnNotify)} options={[{ value: "unfocused", label: "Only when unfocused" }, { value: "always", label: "Always" }, { value: "never", label: "Never" }]} />} />
        <SettingsRow title="Enable permission notifications" sub="Show alerts when permission is required." control={<ToggleSwitch checked={permissionNotify} onChange={setTracked("permission_notifications", setPermissionNotify)} />} />
        <SettingsRow title="Enable question notifications" sub="Show alerts when input is needed to continue." control={<ToggleSwitch checked={questionNotify} onChange={setTracked("question_notifications", setQuestionNotify)} />} />
      </SettingsList>
    </SimpleSettingsSurface>
  );
};

const AppearanceSettingsSurface = ({
  theme,
  onThemeChange,
}: {
  theme: OrdoTheme;
  onThemeChange: (theme: OrdoTheme) => void;
}) => {
  const bright = theme === "bright";
  return (
    <SimpleSettingsSurface
      icon={<Palette size={22} />}
      title="Appearance"
      sub="Visual preferences for the Ordo desktop shell."
    >
      <SettingsList>
        <SettingsRow
          title="Bright mode"
          sub="Use a parchment-light interface. Dark remains the default."
          control={
            <ToggleSwitch
              checked={bright}
              onChange={(checked) => onThemeChange(checked ? "bright" : "dark")}
            />
          }
        />
        <SettingsRow
          title="Current theme"
          sub={bright ? "Bright parchment surfaces with Ordo lamp accents." : "Dark ink surfaces with Ordo lamp accents."}
          control={<Badge variant={bright ? "warn" : "neutral"}>{bright ? "bright" : "dark"}</Badge>}
        />
      </SettingsList>
      <Card>
        <Mono size={11} upper track="0.18em" color={UI.textMuted}>
          Preview
        </Mono>
        <div className="grid grid-cols-2 gap-3" style={{ marginTop: 12 }}>
          {(["dark", "bright"] as OrdoTheme[]).map((item) => {
            const selected = theme === item;
            return (
              <button
                key={item}
                type="button"
                onClick={() => onThemeChange(item)}
                style={{
                  minHeight: 92,
                  textAlign: "left",
                  padding: 14,
                  borderRadius: 10,
                  border: `1px solid ${selected ? UI.primaryBorder : UI.cardBorder}`,
                  background: selected ? UI.primarySoft : UI.cardBgRaised,
                  color: UI.parchment,
                  cursor: "pointer",
                }}
              >
                <Mono size={13} color={UI.parchment} weight={700}>
                  {item === "dark" ? "Dark" : "Bright"}
                </Mono>
                <div style={{ marginTop: 8 }}>
                  <Mono size={11} color={UI.textMuted}>
                    {item === "dark"
                      ? "Default Ordo ink surface."
                      : "Light parchment surface for brighter rooms."}
                  </Mono>
                </div>
              </button>
            );
          })}
        </div>
      </Card>
    </SimpleSettingsSurface>
  );
};

const PlaceholderSettingsSurface = ({ kind, icon }: { kind: string; icon: ReactNode }) => (
  <SimpleSettingsSurface icon={icon} title={kind} sub={`${kind} settings for the Ordo local runtime.`}>
    <SettingsList>
      <SettingsRow title={`${kind} defaults`} sub="Configure defaults for this workspace." control={<Button size="sm">Configure</Button>} />
      <SettingsRow title="Sync with runtime" sub="Persist these preferences into Ordo's local settings store." control={<Badge variant="neutral">local</Badge>} />
    </SettingsList>
  </SimpleSettingsSurface>
);

const CustomMcpSettingsSurface = () => {
  const [transport, setTransport] = useState<"stdio" | "http">("http");
  const [name, setName] = useState("");
  const [url, setUrl] = useState("");
  const [tokenEnv, setTokenEnv] = useState("MCP_BEARER_TOKEN");
  const [headerKey, setHeaderKey] = useState("");
  const [headerValue, setHeaderValue] = useState("");
  const saveCustomMcp = () => {
    publishUxiDebugEvent("ordo.settings.mcp", "custom_mcp_save_requested", "Custom MCP server save requested.", {
      name,
      transport,
      url,
      token_env: tokenEnv,
      has_static_header: Boolean(headerKey.trim() || headerValue.trim()),
    });
  };
  return (
    <SimpleSettingsSurface
      icon={<Server size={22} />}
      title="Connect to a custom MCP"
      sub="Register custom MCP servers without mixing them into plugin inventory."
    >
      <Card>
        <Field label="Name" required>
          <TextInput value={name} onChange={setName} placeholder="MCP server name" />
        </Field>
        <div className="grid grid-cols-2 gap-1" style={{ marginTop: 12 }}>
          <Button onClick={() => { setTransport("stdio"); publishUxiDebugEvent("ordo.settings.mcp", "transport_selected", "Custom MCP transport selected.", { transport: "stdio" }); }} variant={transport === "stdio" ? "primary" : "secondary"}>STDIO</Button>
          <Button onClick={() => { setTransport("http"); publishUxiDebugEvent("ordo.settings.mcp", "transport_selected", "Custom MCP transport selected.", { transport: "http" }); }} variant={transport === "http" ? "primary" : "secondary"}>Streamable HTTP</Button>
        </div>
      </Card>
      <SettingsList>
        <SettingsRow title="URL" sub="Streamable HTTP MCP endpoint." control={<TextInput value={url} onChange={setUrl} placeholder="https://mcp.example.com/mcp" />} />
        <SettingsRow title="Bearer token env var" sub="Environment variable containing the token." control={<TextInput value={tokenEnv} onChange={setTokenEnv} placeholder="MCP_BEARER_TOKEN" />} />
        <SettingsRow title="Headers" sub="Optional static header." control={<div className="grid grid-cols-2 gap-2"><TextInput value={headerKey} onChange={setHeaderKey} placeholder="Key" /><TextInput value={headerValue} onChange={setHeaderValue} placeholder="Value" /></div>} />
        <SettingsRow title="Headers from environment variables" sub="Map an HTTP header to an environment variable." control={<Button size="sm" onClick={() => publishUxiDebugEvent("ordo.settings.mcp", "add_env_header_requested", "Environment-backed MCP header row requested.")}><Plus size={12} /> Add variable</Button>} />
      </SettingsList>
      <div className="flex justify-end">
        <Button variant="primary" disabled={!name.trim() || (transport === "http" && !url.trim())} onClick={saveCustomMcp}>Save</Button>
      </div>
    </SimpleSettingsSurface>
  );
};

const DeviceConnectionsSurface = () => {
  const [mode, setMode] = useState<"control" | "ssh">("ssh");
  const [host, setHost] = useState("");
  const [user, setUser] = useState("");
  const selectMode = (nextMode: "control" | "ssh") => {
    setMode(nextMode);
    publishUxiDebugEvent("ordo.settings.connections", "connection_mode_selected", "Connection mode selected.", { mode: nextMode });
  };
  const addSshConnection = () => {
    publishUxiDebugEvent("ordo.settings.connections", "ssh_connection_add_requested", "SSH connection add requested.", {
      host,
      user,
    });
  };
  return (
    <SimpleSettingsSurface
      icon={<Globe size={22} />}
      title="Connections"
      sub="Control this PC or connect Ordo to remote devices through SSH."
    >
      <div className="flex items-center gap-6" style={{ borderBottom: `1px solid ${UI.cardBorder}`, paddingBottom: 10 }}>
        <Button onClick={() => selectMode("control")} variant={mode === "control" ? "primary" : "ghost"}>Control this PC</Button>
        <Button onClick={() => selectMode("ssh")} variant={mode === "ssh" ? "primary" : "ghost"}>SSH</Button>
      </div>
      {mode === "ssh" ? (
        <Card>
          <Mono size={12} color={UI.parchment} weight={600}>SSH connections from this PC</Mono>
          <div
            style={{
              marginTop: 16,
              border: `1px solid ${UI.cardBorderStrong}`,
              borderRadius: 8,
              minHeight: 150,
              display: "grid",
              placeItems: "center",
              textAlign: "center",
            }}
          >
            <div>
              <div className="flex items-center justify-center gap-3">
                <Laptop size={28} color={UI.textMuted} />
                <Server size={28} color={UI.textMuted} />
              </div>
              <div style={{ marginTop: 12 }}><Mono size={12} color={UI.textMuted}>Connect to a remote device through SSH.</Mono></div>
              <div className="grid grid-cols-2 gap-2" style={{ marginTop: 14 }}>
                <TextInput value={host} onChange={setHost} placeholder="host or IP" />
                <TextInput value={user} onChange={setUser} placeholder="user" />
              </div>
              <div style={{ marginTop: 10 }}><Button variant="primary" disabled={!host.trim()} onClick={addSshConnection}>Add</Button></div>
            </div>
          </div>
        </Card>
      ) : (
        <SettingsList>
          <SettingsRow title="Control this PC" sub="Permit Ordo to control this local desktop through approved actions." control={<Badge variant="info">local</Badge>} />
          <SettingsRow title="Require confirmation" sub="Ask before keyboard, mouse, or window control actions." control={<ToggleSwitch checked onChange={(checked) => publishUxiDebugEvent("ordo.settings.connections", "local_control_confirmation_changed", "Local control confirmation setting changed.", { checked })} />} />
        </SettingsList>
      )}
    </SimpleSettingsSurface>
  );
};

type RemoteChannelId = "email" | "signal" | "matrix" | "telegram" | "sms";

const RemoteCommunicationSurface = ({ refreshKey = 0 }: { refreshKey?: number }) => {
  const [channel, setChannel] = useState<RemoteChannelId>("email");
  const [catalogReady, setCatalogReady] = useState(false);
  const [catalogDescription, setCatalogDescription] = useState("Checking local connection catalog...");
  const [emailAddress, setEmailAddress] = useState("");
  const [displayName, setDisplayName] = useState("Ordo");
  const [imapHost, setImapHost] = useState("");
  const [imapPort, setImapPort] = useState("993");
  const [smtpHost, setSmtpHost] = useState("");
  const [smtpPort, setSmtpPort] = useState("587");
  const [imapUsername, setImapUsername] = useState("");
  const [secret, setSecret] = useState("");
  const [authorizedSenders, setAuthorizedSenders] = useState("");
  const [commandPrefix, setCommandPrefix] = useState("ordo:");
  const [validation, setValidation] = useState<string | null>(null);
  const remoteChannels: Array<{
    id: RemoteChannelId;
    label: string;
    sub: string;
    status: "native" | "planned";
    glyph: typeof Mail;
  }> = [
    { id: "email", label: "Email", sub: "IMAP command intake and SMTP replies.", status: "native", glyph: Mail },
    { id: "signal", label: "Signal", sub: "Linked-device secure message bridge.", status: "planned", glyph: MessageSquare },
    { id: "matrix", label: "Matrix", sub: "Room-based command and alert bridge.", status: "planned", glyph: Network },
    { id: "telegram", label: "Telegram", sub: "Bot-token command and notification bridge.", status: "planned", glyph: Send },
    { id: "sms", label: "SMS", sub: "Phone-number or provider-backed fallback channel.", status: "planned", glyph: Radio },
  ];
  const selectedChannel = remoteChannels.find((item) => item.id === channel) ?? remoteChannels[0];

  useEffect(() => {
    let cancelled = false;
    void listConnectionTypes()
      .then((res) => {
        if (cancelled) return;
        const emailType = res.types.find((type) => type.id === "email" || type.service === "email");
        setCatalogReady(Boolean(emailType));
        setCatalogDescription(
          emailType?.description ??
            "Email connection type is not present in the local catalog yet.",
        );
      })
      .catch((err: unknown) => {
        if (cancelled) return;
        setCatalogReady(false);
        setCatalogDescription(err instanceof Error ? err.message : String(err));
      });
    return () => {
      cancelled = true;
    };
  }, [refreshKey]);

  const selectChannel = (nextChannel: RemoteChannelId) => {
    setChannel(nextChannel);
    setValidation(null);
    publishUxiDebugEvent("ordo.remote_communication", "channel_selected", "Remote communication channel selected.", {
      channel: nextChannel,
    });
  };

  const preparePlannedChannel = () => {
    publishUxiDebugEvent("ordo.remote_communication", "planned_channel_requested", "Remote communication channel planning requested.", {
      channel,
      label: selectedChannel.label,
    });
  };

  const requiredMissing = [
    ["Email address", emailAddress],
    ["IMAP host", imapHost],
    ["SMTP host", smtpHost],
    ["Username", imapUsername],
    ["App password", secret],
  ].filter(([, value]) => !String(value).trim());

  const validateEmailConfig = () => {
    if (requiredMissing.length > 0) {
      const message = `Missing ${requiredMissing.map(([label]) => label).join(", ")}.`;
      setValidation(message);
      publishUxiDebugEvent("ordo.email", "email_config_validation_failed", message, {
        missing: requiredMissing.map(([label]) => label),
      }, "WARN");
      return;
    }
    const message = "Email configuration fields are ready for vault-backed save/test wiring.";
    setValidation(message);
    publishUxiDebugEvent("ordo.email", "email_config_validated", message, {
      email_address: emailAddress,
      imap_host: imapHost,
      imap_port: imapPort,
      smtp_host: smtpHost,
      smtp_port: smtpPort,
      command_prefix: commandPrefix,
      authorized_sender_count: authorizedSenders.split(/\r?\n/).map((item) => item.trim()).filter(Boolean).length,
    });
  };

  const saveEmailConfig = () => {
    publishUxiDebugEvent("ordo.email", "email_config_save_requested", "Email config save requested.", {
      email_address: emailAddress,
      display_name: displayName,
      imap_host: imapHost,
      imap_port: imapPort,
      smtp_host: smtpHost,
      smtp_port: smtpPort,
      imap_username: imapUsername,
      command_prefix: commandPrefix,
      has_secret: Boolean(secret.trim()),
      authorized_sender_count: authorizedSenders.split(/\r?\n/).map((item) => item.trim()).filter(Boolean).length,
    });
    setValidation("Save is queued for the future vault-backed email connection endpoint.");
  };

  return (
    <SimpleSettingsSurface
      icon={<Mail size={22} />}
      title="Remote Communication"
      sub="Channels that let Ordo receive commands and send replies outside the main desktop UXI."
    >
      <div className="grid gap-3" style={{ gridTemplateColumns: "repeat(auto-fit, minmax(190px, 1fr))" }}>
        {remoteChannels.map((item) => {
          const Icon = item.glyph;
          const active = item.id === channel;
          return (
            <button
              key={item.id}
              type="button"
              onClick={() => selectChannel(item.id)}
              style={{
                textAlign: "left",
                borderRadius: 10,
                border: `1px solid ${active ? UI.primaryBorder : UI.cardBorder}`,
                background: active ? UI.primarySoft : UI.cardBg,
                padding: 16,
                cursor: "pointer",
              }}
            >
              <div className="flex items-center justify-between gap-2">
                <div className="flex items-center gap-2">
                  <Icon size={16} color={active ? UI.primary : UI.textMuted} />
                  <Mono size={12} color={UI.parchment} weight={700}>{item.label}</Mono>
                </div>
                <Badge variant={item.status === "native" ? "success" : "neutral"}>{item.status}</Badge>
              </div>
              <div style={{ marginTop: 8 }}><Mono size={11} color={UI.textMuted}>{item.sub}</Mono></div>
            </button>
          );
        })}
      </div>

      {channel !== "email" ? (
        <>
          <Card>
            <div className="flex items-start justify-between gap-4">
              <div>
                <Mono size={11} upper track="0.16em" color={UI.textMuted}>Planned channel</Mono>
                <div style={{ marginTop: 8 }}>
                  <span style={{ fontFamily: FRAUNCES, fontSize: 22, fontWeight: 650, color: UI.parchment }}>
                    {selectedChannel.label}
                  </span>
                </div>
                <div style={{ marginTop: 8 }}>
                  <Mono size={12} color={UI.textMuted}>
                    This slot is reserved for a native Ordo remote channel. It is surfaced now so the UXI has a stable home before backend pairing, vault secrets, and command intake are wired.
                  </Mono>
                </div>
              </div>
              <Badge variant="warn">skeleton</Badge>
            </div>
          </Card>
          <SettingsList>
            <SettingsRow title={`${selectedChannel.label} identity`} sub="Linked account, device name, or bot identity." control={<TextInput value="" onChange={() => undefined} placeholder="coming next" />} />
            <SettingsRow title="Authorized senders" sub="One account, phone number, or room per line." control={<Textarea value="" onChange={() => undefined} placeholder="trusted sender list" rows={4} />} />
            <SettingsRow title="Command prefix" sub="Prefix that marks an inbound message as an Ordo command." control={<TextInput value="ordo:" onChange={() => undefined} placeholder="ordo:" />} />
            <SettingsRow title="Runtime state" sub="No runtime bridge is installed for this channel yet." control={<Badge variant="neutral">not wired</Badge>} />
          </SettingsList>
          <div className="flex justify-end">
            <Button onClick={preparePlannedChannel}><Plus size={13} /> Queue channel wiring</Button>
          </div>
        </>
      ) : (
        <>
      <div className="grid gap-3" style={{ gridTemplateColumns: "repeat(auto-fit, minmax(240px, 1fr))" }}>
        <Card>
          <Mono size={11} upper track="0.16em" color={UI.textMuted}>Native crate</Mono>
          <div style={{ marginTop: 8 }}><Mono size={14} color={UI.parchment} weight={700}>ordo-email</Mono></div>
          <div style={{ marginTop: 8 }}><Mono size={11} color={UI.textMuted}>Polls IMAP, extracts command subjects, publishes bus events, and sends SMTP replies.</Mono></div>
        </Card>
        <Card>
          <Mono size={11} upper track="0.16em" color={UI.textMuted}>Connection catalog</Mono>
          <div style={{ marginTop: 8 }}><Badge variant={catalogReady ? "success" : "warn"}>{catalogReady ? "registered" : "not registered"}</Badge></div>
          <div style={{ marginTop: 8 }}><Mono size={11} color={UI.textMuted}>{catalogDescription}</Mono></div>
        </Card>
        <Card>
          <Mono size={11} upper track="0.16em" color={UI.textMuted}>Command prefix</Mono>
          <div style={{ marginTop: 8 }}><Mono size={14} color={LAMP} weight={700}>{commandPrefix || "ordo:"}</Mono></div>
          <div style={{ marginTop: 8 }}><Mono size={11} color={UI.textMuted}>Only messages using this subject prefix should become Ordo commands.</Mono></div>
        </Card>
      </div>

      <SettingsList>
        <SettingsRow title="Email address" sub="Address Ordo polls and sends from." control={<TextInput value={emailAddress} onChange={(value) => { setEmailAddress(value); if (!imapUsername) setImapUsername(value); }} placeholder="ordo@example.com" />} />
        <SettingsRow title="Display name" sub="Name shown on outgoing replies." control={<TextInput value={displayName} onChange={setDisplayName} placeholder="Ordo" />} />
        <SettingsRow title="IMAP host" sub="Inbox server used for command polling." control={<TextInput value={imapHost} onChange={setImapHost} placeholder="imap.gmail.com" />} />
        <SettingsRow title="IMAP port" sub="Defaults to 993 for TLS." control={<TextInput value={imapPort} onChange={setImapPort} placeholder="993" />} />
        <SettingsRow title="SMTP host" sub="Outgoing reply server." control={<TextInput value={smtpHost} onChange={setSmtpHost} placeholder="smtp.gmail.com" />} />
        <SettingsRow title="SMTP port" sub="Defaults to 587 for STARTTLS." control={<TextInput value={smtpPort} onChange={setSmtpPort} placeholder="587" />} />
        <SettingsRow title="Username" sub="IMAP login username, usually the same as the email address." control={<TextInput value={imapUsername} onChange={setImapUsername} placeholder="ordo@example.com" />} />
        <SettingsRow title="App password" sub="Use an app password or vault secret, never your mailbox login password." control={<TextInput value={secret} onChange={setSecret} placeholder="stored in vault later" type="password" />} />
        <SettingsRow title="Authorized senders" sub="One sender per line. Empty means accept all senders." control={<Textarea value={authorizedSenders} onChange={setAuthorizedSenders} placeholder="you@example.com" rows={4} />} />
        <SettingsRow title="Command prefix" sub="Subject prefix that marks a message as an Ordo command." control={<TextInput value={commandPrefix} onChange={setCommandPrefix} placeholder="ordo:" />} />
      </SettingsList>

      {validation && (
        <Alert variant={validation.startsWith("Missing") ? "warn" : "success"}>
          {validation}
        </Alert>
      )}

      <div className="flex justify-end gap-2">
        <Button onClick={validateEmailConfig}>Validate fields</Button>
        <Button variant="primary" onClick={saveEmailConfig} disabled={requiredMissing.length > 0}>Save email channel</Button>
      </div>
        </>
      )}
    </SimpleSettingsSurface>
  );
};

const BUILD_STEPS: BuildStep[] = [
  "intake",
  "blueprint",
  "crate_build",
  "crate_couple",
  "build_test",
  "launch_proof",
];

const BUILD_ERROR_CLASSES: BuildErrorClass[] = [
  "bounded_mechanical",
  "blueprint_amendment",
  "compile_errors",
  "compile_warnings",
  "architectural_violation",
  "stub_detected",
  "couple_debt",
  "launch_proof_missing",
  "runtime_panic",
  "unbounded_ownership",
  "retry_exhausted",
  "unknown",
];

const buildStepLabel = (step: BuildStep): string =>
  step
    .split("_")
    .map((part) => part[0].toUpperCase() + part.slice(1))
    .join(" ");

const buildStepSkill = (step: BuildStep): string => {
  const skills: Record<BuildStep, string> = {
    intake: "ordo-build-intake",
    blueprint: "ordo-build-blueprint",
    crate_build: "ordo-crate-build",
    crate_couple: "ordo-crate-couple",
    build_test: "ordo-build-test",
    launch_proof: "ordo-launch-proof",
  };
  return skills[step];
};

const buildStatusVariant = (status: BuildLedger["status"]): "success" | "warn" | "danger" | "neutral" => {
  if (status === "complete") return "success";
  if (status === "halted") return "danger";
  if (status === "active") return "warn";
  return "neutral";
};

const BuildsSurface = () => {
  const [builds, setBuilds] = useState<BuildLedger[]>([]);
  const [activeBuilds, setActiveBuilds] = useState<string[]>([]);
  const [selectedId, setSelectedId] = useState<string | null>(null);
  const [projectId, setProjectId] = useState("");
  const [gateStatus, setGateStatus] = useState<"pass" | "fail" | "deferred">("pass");
  const [gateSummary, setGateSummary] = useState("");
  const [gateDetails, setGateDetails] = useState("");
  const [gateErrorClass, setGateErrorClass] = useState<BuildErrorClass>("compile_warnings");
  const [deferredReason, setDeferredReason] = useState("");
  const [loading, setLoading] = useState(true);
  const [busy, setBusy] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [notice, setNotice] = useState<string | null>(null);

  const selected = useMemo(
    () => builds.find((build) => build.build_id === selectedId) ?? builds[0] ?? null,
    [builds, selectedId],
  );

  const refreshBuilds = async (source = "builds.refreshed") => {
    try {
      setLoading(true);
      const out = await listBuilds();
      setBuilds(out.builds ?? []);
      setActiveBuilds(out.active_builds ?? []);
      setError(null);
      setSelectedId((current) => {
        if (current && (out.builds ?? []).some((build) => build.build_id === current)) return current;
        return out.active_builds?.[0] ?? out.builds?.[0]?.build_id ?? null;
      });
      publishUxiDebugEvent("ordo.builds", source, "Build ledgers loaded from runtime.", {
        count: out.builds?.length ?? 0,
        active_count: out.active_builds?.length ?? 0,
      });
    } catch (err) {
      const message = err instanceof Error ? err.message : String(err);
      setError(message);
      publishUxiDebugEvent("ordo.builds", "builds.sync_failed", "Build ledger sync failed.", { error: message }, "ERROR");
    } finally {
      setLoading(false);
    }
  };

  useEffect(() => {
    void refreshBuilds("builds.initial_load");
  }, []);

  const beginBuild = async () => {
    const trimmed = projectId.trim();
    if (!trimmed) return;
    setBusy("start");
    setNotice(null);
    try {
      const out = await startBuild(trimmed);
      setBuilds((current) => [out.ledger, ...current.filter((build) => build.build_id !== out.ledger.build_id)]);
      setActiveBuilds((current) => [out.ledger.build_id, ...current.filter((id) => id !== out.ledger.build_id)]);
      setSelectedId(out.ledger.build_id);
      setProjectId("");
      setNotice(`Build started for ${out.ledger.project_id}. Released ${buildStepSkill(out.ledger.current_step)}.`);
      publishUxiDebugEvent("ordo.builds", "build.started", "Build spine started.", {
        build_id: out.ledger.build_id,
        project_id: out.ledger.project_id,
        step: out.ledger.current_step,
      });
    } catch (err) {
      const message = err instanceof Error ? err.message : String(err);
      setError(message);
      publishUxiDebugEvent("ordo.builds", "build.start_failed", "Build start failed.", { error: message }, "ERROR");
    } finally {
      setBusy(null);
    }
  };

  const submitGate = async () => {
    if (!selected || !gateSummary.trim()) return;
    setBusy("gate");
    setNotice(null);
    const evidence = {
      summary: gateSummary.trim(),
      details: gateDetails
        .split(/\r?\n/)
        .map((detail) => detail.trim())
        .filter(Boolean),
      artifacts: [],
      checked_at: new Date().toISOString(),
    };
    const outcome: BuildGateOutcome =
      gateStatus === "pass"
        ? { status: "pass", evidence }
        : gateStatus === "fail"
          ? { status: "fail", error_class: gateErrorClass, evidence }
          : { status: "deferred", reason: deferredReason.trim() || gateSummary.trim(), evidence };
    const result: BuildGateResult = {
      build_id: selected.build_id,
      project_id: selected.project_id,
      step: selected.current_step,
      outcome,
    };
    try {
      const out = await submitBuildGateResult(selected.build_id, result);
      setBuilds((current) => current.map((build) => (build.build_id === out.ledger.build_id ? out.ledger : build)));
      setActiveBuilds((current) =>
        out.ledger.status === "active"
          ? current.includes(out.ledger.build_id)
            ? current
            : [out.ledger.build_id, ...current]
          : current.filter((id) => id !== out.ledger.build_id),
      );
      setGateSummary("");
      setGateDetails("");
      setDeferredReason("");
      setNotice(`Gate recorded: ${out.decision}. Current step: ${buildStepLabel(out.ledger.current_step)}.`);
      publishUxiDebugEvent("ordo.builds", "build.gate_recorded", "Build gate result recorded.", {
        build_id: out.ledger.build_id,
        decision: out.decision,
        step: out.ledger.current_step,
        status: out.ledger.status,
      });
    } catch (err) {
      const message = err instanceof Error ? err.message : String(err);
      setError(message);
      publishUxiDebugEvent("ordo.builds", "build.gate_failed", "Build gate result failed.", { error: message }, "ERROR");
    } finally {
      setBusy(null);
    }
  };

  const currentIndex = selected ? BUILD_STEPS.indexOf(selected.current_step) : -1;

  return (
    <SimpleSettingsSurface
      icon={<Wrench size={22} />}
      title="Builds"
      sub="Native build spine for Ordo's Rust coder: intake, blueprint, crate build, crate coupling, build tests, and launch proof."
    >
      <div className="flex justify-between gap-3 flex-wrap">
        <div style={{ flex: "1 1 420px" }}>
          <Mono size={12} upper track="0.16em" color={UI.textMuted}>Start build</Mono>
          <div className="flex gap-2" style={{ marginTop: 8 }}>
            <TextInput value={projectId} onChange={setProjectId} placeholder="Project id or local workspace name" />
            <Button variant="primary" onClick={() => void beginBuild()} disabled={busy === "start" || !projectId.trim()}>
              <Plus size={13} /> Start
            </Button>
          </div>
        </div>
        <div className="flex items-end gap-2">
          <Button onClick={() => void refreshBuilds()} disabled={loading}>
            <RefreshCcw size={13} /> Refresh
          </Button>
        </div>
      </div>

      {error && <Alert variant="danger">{error}</Alert>}
      {notice && <Alert variant="success">{notice}</Alert>}

      <div className="grid gap-3" style={{ gridTemplateColumns: "minmax(280px, 0.9fr) minmax(360px, 1.4fr)" }}>
        <Card>
          <div className="flex items-center justify-between gap-2">
            <Mono size={11} upper track="0.18em" color={UI.textMuted}>Ledgers</Mono>
            <Badge variant="neutral">{builds.length}</Badge>
          </div>
          <div className="space-y-2" style={{ marginTop: 12 }}>
            {loading ? (
              <Mono size={12} color={UI.textMuted}>Loading build ledgers...</Mono>
            ) : builds.length === 0 ? (
              <Mono size={12} color={UI.textMuted}>No build ledgers yet.</Mono>
            ) : (
              builds.map((build) => (
                <button
                  key={build.build_id}
                  type="button"
                  onClick={() => setSelectedId(build.build_id)}
                  style={{
                    width: "100%",
                    textAlign: "left",
                    padding: 12,
                    borderRadius: 8,
                    border: `1px solid ${selected?.build_id === build.build_id ? UI.primaryBorder : UI.cardBorder}`,
                    background: selected?.build_id === build.build_id ? UI.primarySoft : UI.cardBgRaised,
                    cursor: "pointer",
                  }}
                >
                  <div className="flex items-center justify-between gap-2">
                    <Mono size={12} color={UI.parchment} weight={700}>{build.project_id}</Mono>
                    <Badge variant={buildStatusVariant(build.status)}>{build.status}</Badge>
                  </div>
                  <div style={{ marginTop: 7 }}>
                    <Mono size={10} color={UI.textMuted}>
                      {build.build_id.slice(0, 8)} · {buildStepLabel(build.current_step)}
                    </Mono>
                  </div>
                  {activeBuilds.includes(build.build_id) && (
                    <div style={{ marginTop: 8 }}>
                      <Badge variant="warn">active</Badge>
                    </div>
                  )}
                </button>
              ))
            )}
          </div>
        </Card>

        <Card>
          {selected ? (
            <div className="space-y-4">
              <div className="flex items-start justify-between gap-3 flex-wrap">
                <div>
                  <Mono size={11} upper track="0.18em" color={UI.textMuted}>Selected build</Mono>
                  <div style={{ marginTop: 6 }}>
                    <Serif size={20} weight={600}>{selected.project_id}</Serif>
                  </div>
                  <div style={{ marginTop: 6, overflowWrap: "anywhere" }}>
                    <Mono size={10} color={UI.textMuted}>{selected.build_id}</Mono>
                  </div>
                </div>
                <Badge variant={buildStatusVariant(selected.status)}>{selected.status}</Badge>
              </div>

              <div className="flex gap-2 flex-wrap">
                {BUILD_STEPS.map((step, index) => {
                  const current = step === selected.current_step;
                  const complete = currentIndex >= 0 && index < currentIndex;
                  return (
                    <div
                      key={step}
                      style={{
                        border: `1px solid ${current ? UI.primaryBorder : UI.cardBorder}`,
                        background: current ? UI.primarySoft : complete ? `${UI.jade}12` : UI.cardBgRaised,
                        borderRadius: 999,
                        padding: "7px 10px",
                      }}
                    >
                      <Mono size={10} color={current ? UI.primary : complete ? UI.jade : UI.textMuted} weight={current ? 700 : 500}>
                        {index + 1}. {buildStepLabel(step)}
                      </Mono>
                    </div>
                  );
                })}
              </div>

              <SettingsList>
                <SettingsRow title="Current skill" sub={buildStepSkill(selected.current_step)} control={<Badge variant="primary">{selected.current_step}</Badge>} />
                <SettingsRow title="Autonomous correction" sub="Eligibility can be recorded, but the operator still owns approval boundaries." control={<Badge variant={selected.autonomous_correction ? "warn" : "neutral"}>{selected.autonomous_correction ? "armed" : "off"}</Badge>} />
                <SettingsRow title="Deferred debt" sub="Must be cleared before build tests and launch proof can pass." control={<Badge variant={selected.deferred_debt.length > 0 ? "warn" : "success"}>{selected.deferred_debt.length}</Badge>} />
                <SettingsRow title="Couple markers" sub="COUPLE markers are temporary and must be removed before proof." control={<Badge variant={selected.couple_markers.length > 0 ? "warn" : "success"}>{selected.couple_markers.length}</Badge>} />
              </SettingsList>

              <Card>
                <Mono size={11} upper track="0.18em" color={UI.textMuted}>Manual gate result</Mono>
                <div className="grid gap-3" style={{ gridTemplateColumns: "repeat(auto-fit, minmax(220px, 1fr))", marginTop: 12 }}>
                  <SettingsRow title="Outcome" control={<SmallSelect value={gateStatus} onChange={setGateStatus} options={[
                    { value: "pass", label: "Pass" },
                    { value: "fail", label: "Fail" },
                    { value: "deferred", label: "Deferred" },
                  ]} />} />
                  {gateStatus === "fail" && (
                    <SettingsRow title="Error class" control={<SmallSelect value={gateErrorClass} onChange={setGateErrorClass} options={BUILD_ERROR_CLASSES.map((value) => ({ value, label: value.replace(/_/g, " ") }))} />} />
                  )}
                  {gateStatus === "deferred" && (
                    <SettingsRow title="Deferred reason" control={<TextInput value={deferredReason} onChange={setDeferredReason} placeholder="What is being carried forward?" />} />
                  )}
                </div>
                <div style={{ marginTop: 12 }}>
                  <Textarea value={gateSummary} onChange={setGateSummary} rows={3} placeholder="Gate evidence summary. A model claim is not enough; cite the check or artifact." />
                </div>
                <div style={{ marginTop: 10 }}>
                  <Textarea value={gateDetails} onChange={setGateDetails} rows={3} placeholder="Optional details, one per line." />
                </div>
                <div className="flex justify-end" style={{ marginTop: 12 }}>
                  <Button variant="primary" onClick={() => void submitGate()} disabled={busy === "gate" || selected.status !== "active" || !gateSummary.trim()}>
                    Record gate
                  </Button>
                </div>
              </Card>
            </div>
          ) : (
            <div style={{ textAlign: "center", padding: 36 }}>
              <Mono size={13} color={UI.textMuted}>Start a build to create the first ledger.</Mono>
            </div>
          )}
        </Card>
      </div>
    </SimpleSettingsSurface>
  );
};

const RoutinesSurface = ({ modes }: { modes: AssistantMode[] }) => {
  type CronKind = "cron" | "heartbeat" | "routine" | "webhook" | "local_event" | "dreaming" | "coding";
  type CronStatus = "enabled" | "paused";
  type CronJob = {
    id: string;
    name: string;
    kind: CronKind;
    schedule: string;
    mode: string;
    instruction: string;
    workspacePath: string;
    status: CronStatus;
    approvalRequired: boolean;
    allowSubagents: boolean;
    maxSubagents: number;
    createdAt: string;
    updatedAt: string;
  };
  const blankJob = (): CronJob => ({
    id:
      typeof crypto !== "undefined" && "randomUUID" in crypto
        ? crypto.randomUUID()
        : `job-${Date.now().toString(36)}${Math.random().toString(36).slice(2, 6)}`,
    name: "",
    kind: "cron",
    schedule: "",
    mode: "general",
    instruction: "",
    workspacePath: "",
    status: "enabled",
    approvalRequired: true,
    allowSubagents: false,
    maxSubagents: 1,
    createdAt: new Date().toISOString(),
    updatedAt: new Date().toISOString(),
  });
  const intervalFromSchedule = (schedule: string, fallback: number) => {
    const lower = schedule.toLowerCase();
    const explicit = lower.match(/(\d+)\s*(second|seconds|sec|secs|minute|minutes|min|mins|hour|hours|day|days)/);
    if (!explicit) return fallback;
    const value = Math.max(1, Number(explicit[1]) || 1);
    const unit = explicit[2];
    if (unit.startsWith("second") || unit.startsWith("sec")) return value;
    if (unit.startsWith("minute") || unit.startsWith("min")) return value * 60;
    if (unit.startsWith("hour")) return value * 3600;
    if (unit.startsWith("day")) return value * 86400;
    return fallback;
  };
  const kindFromTrigger = (trigger: AutomationTrigger, intent: AutomationIntent, metadata: Record<string, string>): CronKind => {
    if (metadata.ui_kind && ["cron", "heartbeat", "routine", "webhook", "local_event", "dreaming", "coding"].includes(metadata.ui_kind)) {
      return metadata.ui_kind as CronKind;
    }
    if (typeof trigger === "object" && "Cron" in trigger) return "cron";
    if (typeof trigger === "object" && "Heartbeat" in trigger) return "DreamingReview" in intent ? "dreaming" : "heartbeat";
    if ("CodingAutomation" in intent) return "coding";
    if (typeof trigger === "object" && "Webhook" in trigger) return "webhook";
    if (typeof trigger === "object" && "LocalSignal" in trigger) return "local_event";
    return "routine";
  };
  const scheduleFromTrigger = (trigger: AutomationTrigger, metadata: Record<string, string>) => {
    if (metadata.schedule) return metadata.schedule;
    if (typeof trigger === "object" && "Cron" in trigger) return trigger.Cron.expression;
    if (typeof trigger === "object" && "Heartbeat" in trigger) return `every ${trigger.Heartbeat.every_seconds}s`;
    if (typeof trigger === "object" && "IntervalSeconds" in trigger) return `every ${trigger.IntervalSeconds}s`;
    if (typeof trigger === "object" && "Webhook" in trigger) return trigger.Webhook.path;
    if (typeof trigger === "object" && "LocalSignal" in trigger) return trigger.LocalSignal.name;
    if (typeof trigger === "object" && "Event" in trigger) return trigger.Event.topic;
    if (typeof trigger === "object" && "At" in trigger) return trigger.At;
    return "";
  };
  const modeFromSpec = (spec: AutomationSpec) => {
    if (spec.metadata.mode) return spec.metadata.mode;
    if (typeof spec.scope === "object" && "Mode" in spec.scope) return spec.scope.Mode.mode;
    if ("SpawnSubagent" in spec.intent) return spec.intent.SpawnSubagent.mode;
    if ("DreamingReview" in spec.intent) return spec.intent.DreamingReview.mode;
    if ("ConsultMode" in spec.intent) return spec.intent.ConsultMode.target_mode;
    if ("CodingAutomation" in spec.intent) return spec.intent.CodingAutomation.mode;
    return "general";
  };
  const instructionFromSpec = (spec: AutomationSpec) => {
    if (spec.metadata.instruction) return spec.metadata.instruction;
    if ("SpawnSubagent" in spec.intent) return spec.intent.SpawnSubagent.goal;
    if ("ConsultMode" in spec.intent) return spec.intent.ConsultMode.question;
    if ("DreamingReview" in spec.intent) return `Dreaming review: ${spec.intent.DreamingReview.signal_window}`;
    if ("DiagnosticSweep" in spec.intent) return `Diagnostic sweep: ${spec.intent.DiagnosticSweep.profile}`;
    if ("CodingAutomation" in spec.intent) return spec.intent.CodingAutomation.goal;
    return spec.description;
  };
  const workspaceFromSpec = (spec: AutomationSpec) => {
    if (spec.metadata.workspace_path) return spec.metadata.workspace_path;
    if ("CodingAutomation" in spec.intent) return spec.intent.CodingAutomation.workspace_path;
    if (typeof spec.scope === "object" && "Workspace" in spec.scope) return spec.scope.Workspace.path;
    return "";
  };
  const jobFromSpec = (spec: AutomationSpec): CronJob => {
    const metadata = spec.metadata ?? {};
    const maxSubagents = Math.min(5, Math.max(1, Number(metadata.max_subagents ?? ("SpawnSubagent" in spec.intent ? spec.intent.SpawnSubagent.max_iterations : 1)) || 1));
    const kind = kindFromTrigger(spec.trigger, spec.intent, metadata);
    return {
      id: spec.id,
      name: spec.name,
      kind,
      schedule: scheduleFromTrigger(spec.trigger, metadata),
      mode: modeFromSpec(spec),
      instruction: instructionFromSpec(spec),
      workspacePath: workspaceFromSpec(spec),
      status: spec.enabled ? "enabled" : "paused",
      approvalRequired: metadata.approval_required === "true" || JSON.stringify(spec.approval).includes("AtOrAbove") || spec.approval === "Always",
      allowSubagents: metadata.allow_subagents === "true" || "SpawnSubagent" in spec.intent,
      maxSubagents,
      createdAt: spec.created_at,
      updatedAt: spec.updated_at,
    };
  };
  const triggerFromJob = (job: CronJob): AutomationTrigger => {
    const schedule = job.schedule.trim();
    if (job.kind === "cron") {
      return { Cron: { expression: schedule || "operator-defined", timezone: Intl.DateTimeFormat().resolvedOptions().timeZone || "UTC" } };
    }
    if (job.kind === "heartbeat" || job.kind === "dreaming") {
      return { Heartbeat: { every_seconds: intervalFromSchedule(schedule, job.kind === "dreaming" ? 7200 : 3600), jitter_seconds: 0, resume_thread: null } };
    }
    if (job.kind === "webhook") return { Webhook: { path: schedule || `/automations/${job.id}` } };
    if (job.kind === "local_event") return { LocalSignal: { name: schedule || job.name } };
    return "Manual";
  };
  const intentFromJob = (job: CronJob): AutomationIntent => {
    if (job.kind === "dreaming") {
      return { DreamingReview: { mode: job.mode || "dreaming", signal_window: job.instruction || "recent" } };
    }
    if (job.kind === "coding") {
      const risk = job.approvalRequired ? "LocalWrite" : "LocalRead";
      return {
        CodingAutomation: {
          workspace_path: job.workspacePath.trim() || "operator-selected",
          mode: job.mode || "rust_vibe_coder",
          goal: job.instruction,
          max_subagents: Math.min(5, Math.max(1, Number(job.maxSubagents) || 1)),
          write_policy: job.approvalRequired ? "ProposeDiff" : "InspectOnly",
          commit_policy: "NeverCommit",
          dependency_policy: "NoDependencyChanges",
          risk,
        },
      };
    }
    const risk = job.approvalRequired ? "LocalWrite" : "SafeReadOnly";
    return {
      SpawnSubagent: {
        mode: job.mode || "general",
        goal: job.instruction,
        max_iterations: Math.min(5, Math.max(1, Number(job.maxSubagents) || 1)),
        risk,
      },
    };
  };
  const scopeFromJob = (job: CronJob): AutomationScope =>
    job.kind === "coding"
      ? { Workspace: { path: job.workspacePath.trim() || "operator-selected" } }
      : job.kind === "dreaming"
        ? { Mode: { mode: "dreaming" } }
        : { Mode: { mode: job.mode || "general" } };
  const specFromJob = (job: CronJob): AutomationSpec => {
    const now = new Date().toISOString();
    return {
      id: job.id,
      name: job.name.trim(),
      description: job.instruction.trim(),
      enabled: job.status === "enabled",
      trigger: triggerFromJob(job),
      intent: intentFromJob(job),
      scope: scopeFromJob(job),
      approval: job.approvalRequired ? { AtOrAbove: "LocalWrite" } : "Never",
      created_at: job.createdAt || now,
      updated_at: now,
      metadata: {
        ui_kind: job.kind,
        schedule: job.schedule.trim(),
        mode: job.mode.trim() || "general",
        instruction: job.instruction.trim(),
        workspace_path: job.workspacePath.trim(),
        approval_required: String(job.approvalRequired),
        allow_subagents: String(job.allowSubagents),
        max_subagents: String(Math.min(5, Math.max(1, Number(job.maxSubagents) || 1))),
      },
    };
  };
  const [jobs, setJobs] = useState<CronJob[]>([]);
  const [draft, setDraft] = useState("");
  const [editing, setEditing] = useState<CronJob | null>(null);
  const [form, setForm] = useState<CronJob>(blankJob);
  const [loading, setLoading] = useState(true);
  const [syncError, setSyncError] = useState<string | null>(null);
  const modeOptions = useMemo(
    () =>
      (modes.length > 0 ? modes : [{ id: "general", label: "General" } as AssistantMode]).map((mode) => ({
        value: mode.id,
        label: mode.label || mode.id,
      })),
    [modes],
  );
  const refreshJobs = async (event = "automation.refreshed") => {
    try {
      setLoading(true);
      const out = await listAutomations();
      const next = (out.automations ?? []).map(jobFromSpec);
      setJobs(next);
      setSyncError(null);
      publishUxiDebugEvent("ordo.automation", event, "Automation records loaded from runtime.", { count: next.length });
    } catch (err) {
      const message = err instanceof Error ? err.message : String(err);
      setSyncError(message);
      publishUxiDebugEvent("ordo.automation", "automation.sync_failed", "Automation sync failed.", { error: message });
    } finally {
      setLoading(false);
    }
  };
  useEffect(() => {
    void refreshJobs("automation.initial_load");
  }, []);
  const beginNew = (patch: Partial<CronJob> = {}) => {
    const now = new Date().toISOString();
    setEditing(null);
    setForm({ ...blankJob(), ...patch, createdAt: now, updatedAt: now });
  };
  const beginEdit = (job: CronJob) => {
    setEditing(job);
    setForm(job);
  };
  const saveForm = () => {
    const trimmedName = form.name.trim();
    const trimmedInstruction = form.instruction.trim();
    if (!trimmedName || !trimmedInstruction) return;
    const updated: CronJob = {
      ...form,
      name: trimmedName,
      schedule: form.schedule.trim(),
      mode: form.mode.trim() || (form.kind === "coding" ? "rust_vibe_coder" : "general"),
      instruction: trimmedInstruction,
      workspacePath: form.workspacePath.trim(),
      maxSubagents: Math.min(5, Math.max(1, Number(form.maxSubagents) || 1)),
      updatedAt: new Date().toISOString(),
    };
    const save = async () => {
      try {
        if (editing) await deleteAutomation(editing.id);
        await createAutomation(specFromJob(updated));
        publishUxiDebugEvent("ordo.automation", editing ? "automation.updated" : "automation.created", editing ? "Automation updated." : "Automation created.", {
          automation_id: updated.id,
          automation_name: updated.name,
          kind: updated.kind,
          status: updated.status,
        });
        setEditing(null);
        setForm(blankJob());
        await refreshJobs("automation.saved_refresh");
      } catch (err) {
        const message = err instanceof Error ? err.message : String(err);
        setSyncError(message);
        publishUxiDebugEvent("ordo.automation", "automation.save_failed", "Automation save failed.", { error: message });
      }
    };
    void save();
  };
  const toggleJob = (job: CronJob) => {
    const run = async () => {
      const status: CronStatus = job.status === "enabled" ? "paused" : "enabled";
      try {
        if (status === "enabled") await enableAutomation(job.id);
        else await disableAutomation(job.id);
        publishUxiDebugEvent("ordo.automation", status === "enabled" ? "automation.enabled" : "automation.disabled", `Automation ${status}.`, {
          automation_id: job.id,
          automation_name: job.name,
          status,
        });
        await refreshJobs("automation.toggle_refresh");
      } catch (err) {
        const message = err instanceof Error ? err.message : String(err);
        setSyncError(message);
        publishUxiDebugEvent("ordo.automation", "automation.toggle_failed", "Automation toggle failed.", { error: message });
      }
    };
    void run();
  };
  const approveJob = (job: CronJob) => {
    const run = async () => {
      try {
        await approveAutomation(job.id);
        publishUxiDebugEvent("ordo.automation", "automation.approved", "Automation approved.", {
          automation_id: job.id,
          automation_name: job.name,
        });
        await refreshJobs("automation.approve_refresh");
      } catch (err) {
        const message = err instanceof Error ? err.message : String(err);
        setSyncError(message);
        publishUxiDebugEvent("ordo.automation", "automation.approve_failed", "Automation approval failed.", { error: message });
      }
    };
    void run();
  };
  const runTick = () => {
    const run = async () => {
      try {
        const out = await tickAutomations();
        publishUxiDebugEvent("ordo.automation", "automation.tick", "Automation tick requested.", { event_count: out.events?.length ?? 0 });
        await refreshJobs("automation.tick_refresh");
      } catch (err) {
        const message = err instanceof Error ? err.message : String(err);
        setSyncError(message);
        publishUxiDebugEvent("ordo.automation", "automation.tick_failed", "Automation tick failed.", { error: message });
      }
    };
    void run();
  };
  const deleteJob = (job: CronJob) => {
    const run = async () => {
      try {
        await deleteAutomation(job.id);
        publishUxiDebugEvent("ordo.automation", "automation.deleted", "Automation deleted.", {
          automation_id: job.id,
          automation_name: job.name,
        });
        if (editing?.id === job.id) {
          setEditing(null);
          setForm(blankJob());
        }
        await refreshJobs("automation.delete_refresh");
      } catch (err) {
        const message = err instanceof Error ? err.message : String(err);
        setSyncError(message);
        publishUxiDebugEvent("ordo.automation", "automation.delete_failed", "Automation delete failed.", { error: message });
      }
    };
    void run();
  };
  const templates = [
    ["System health check", "Monitor infrastructure and services for errors and outages.", "Runs daily at 8:00 AM", "cron"],
    ["Provider availability check", "Check local and cloud providers, then report degraded lanes.", "Runs every 30 minutes", "cron"],
    ["MCP/plugin audit", "Review MCP and plugin availability without mixing their catalogs.", "Runs every Monday", "cron"],
    ["Memory maintenance heartbeat", "Return to this project and review memory promotion candidates.", "Heartbeat, project continuity", "heartbeat"],
    ["Dreaming reflection review", "Analyze completed work, failures, corrections, and improvement proposals.", "Manual or low-priority heartbeat", "dreaming"],
    ["Remote device check", "Inspect whether paired devices reconnected and log connection status.", "Runs every 15 minutes", "cron"],
    ["Coding warning audit", "Run checks in the selected workspace and propose warning fixes without applying them.", "Manual or daily", "coding"],
    ["Coding regression review", "Inspect recent project changes, summarize risk, and propose a test plan.", "Before release", "coding"],
  ];
  return (
    <SimpleSettingsSurface icon={<Zap size={22} />} title="Automation" sub="Cron jobs, heartbeats, routines, webhooks, local events, and bounded dreaming reviews.">
      <div className="flex justify-between gap-3 flex-wrap">
        <div>
          <Mono size={12} upper track="0.16em" color={UI.textMuted}>Autonomy condition</Mono>
          <div style={{ marginTop: 6, maxWidth: 780 }}>
            <Mono size={12} color={UI.textMuted}>
              Automations are operator-managed records here. Heartbeats resume a specific thread or project check.
              Dreaming is advisory reflection only. Subagents are allowed per job, but execution must stay bounded,
              logged, and approval-gated.
            </Mono>
          </div>
        </div>
        <div className="flex gap-2 flex-wrap">
          <Button onClick={() => void refreshJobs()}><RefreshCcw size={13} /> Refresh</Button>
          <Button onClick={runTick}>Tick now</Button>
          <Button variant="primary" onClick={() => { beginNew(); publishUxiDebugEvent("ordo.automation", "new_automation_requested", "New automation requested."); }}><Plus size={13} /> New automation</Button>
        </div>
      </div>
      {syncError && <Alert variant="warn">Automation runtime sync failed: {syncError}</Alert>}
      <TextInput value={draft} onChange={setDraft} placeholder="What do you want automated?" />
      <div className="flex gap-2 flex-wrap">
        {["Summarize open PRs every weekday morning", "Triage new issues each morning", "Draft release notes when a PR merges"].map((prompt) => (
          <Button key={prompt} size="sm" onClick={() => { setDraft(prompt); beginNew({ name: prompt, instruction: prompt, schedule: "operator-defined" }); publishUxiDebugEvent("ordo.automation", "routine_prompt_selected", "Routine prompt template selected.", { prompt }); }}>{prompt}</Button>
        ))}
      </div>
      <Card>
        <div className="grid gap-3" style={{ gridTemplateColumns: "repeat(auto-fit, minmax(300px, 1fr))" }}>
          <SettingsRow title="Name" sub="Operator-visible label." control={<TextInput value={form.name} onChange={(value) => setForm((current) => ({ ...current, name: value }))} placeholder="System health check" />} />
          <SettingsRow title="Kind" sub="Trigger family." control={<SmallSelect value={form.kind} onChange={(value) => setForm((current) => ({ ...current, kind: value, mode: value === "coding" && current.mode === "general" ? "rust_vibe_coder" : current.mode }))} options={[
            { value: "cron", label: "Cron" },
            { value: "heartbeat", label: "Heartbeat" },
            { value: "routine", label: "Routine" },
            { value: "webhook", label: "Webhook" },
            { value: "local_event", label: "Local event" },
            { value: "dreaming", label: "Dreaming" },
            { value: "coding", label: "Coding" },
          ]} />} />
          <SettingsRow title="Schedule" sub="Human readable for now." control={<TextInput value={form.schedule} onChange={(value) => setForm((current) => ({ ...current, schedule: value }))} placeholder="weekdays 08:30" />} />
          <SettingsRow title="Mode" sub="Mode agent to run as." control={<SmallSelect value={form.mode} onChange={(value) => setForm((current) => ({ ...current, mode: value }))} options={modeOptions} />} />
          {form.kind === "coding" && (
            <SettingsRow
              title="Workspace"
              sub="Local project folder the coding automation may inspect. Writes still require approval."
              control={<TextInput value={form.workspacePath} onChange={(value) => setForm((current) => ({ ...current, workspacePath: value }))} placeholder="C:\\Users\\you\\Projects\\app" />}
            />
          )}
          <SettingsRow title="Approval gate" sub="Require operator approval for risky actions." control={<ToggleSwitch checked={form.approvalRequired} onChange={(value) => setForm((current) => ({ ...current, approvalRequired: value }))} />} />
          <SettingsRow title="Subagents" sub="Allow bounded helper agents." control={<ToggleSwitch checked={form.allowSubagents} onChange={(value) => setForm((current) => ({ ...current, allowSubagents: value }))} />} />
          <SettingsRow title="Max subagents" sub="Hard per-run collaborator cap." control={<TextInput value={String(form.maxSubagents)} onChange={(value) => setForm((current) => ({ ...current, maxSubagents: Number(value) || 1 }))} placeholder="1" />} />
        </div>
        <div style={{ marginTop: 12 }}>
          <Textarea value={form.instruction} onChange={(value) => setForm((current) => ({ ...current, instruction: value }))} rows={4} placeholder="What should Ordo do when this fires?" />
        </div>
        <div className="flex justify-end gap-2" style={{ marginTop: 12 }}>
          <Button onClick={() => { setEditing(null); setForm(blankJob()); }}>Clear</Button>
          <Button variant="primary" onClick={saveForm} disabled={!form.name.trim() || !form.instruction.trim()}>
            {editing ? "Save changes" : "Create job"}
          </Button>
        </div>
      </Card>
      <Card>
        {loading ? (
          <div style={{ textAlign: "center", padding: 36 }}><Mono size={13} color={UI.textMuted}>Loading automations...</Mono></div>
        ) : jobs.length === 0 ? (
          <div style={{ textAlign: "center", padding: 36 }}><Mono size={13} color={UI.textMuted}>No automations yet.</Mono></div>
        ) : (
          <div className="space-y-3">
            {jobs.map((job) => (
              <div key={job.id} className="flex items-start justify-between gap-3" style={{ padding: 12, border: `1px solid ${UI.cardBorder}`, borderRadius: 8, background: UI.cardBgRaised, flexWrap: "wrap" }}>
                <div style={{ minWidth: 260, flex: "1 1 420px" }}>
                  <div className="flex items-center gap-2 flex-wrap">
                    <Mono size={13} color={UI.parchment} weight={700}>{job.name}</Mono>
                    <Badge variant={job.status === "enabled" ? "success" : "neutral"}>{job.status}</Badge>
                  <Badge variant={job.kind === "dreaming" ? "warn" : job.kind === "heartbeat" ? "info" : "neutral"}>{job.kind}</Badge>
                    {job.allowSubagents && <Badge variant="info">subagents {job.maxSubagents}</Badge>}
                  </div>
                  <div style={{ marginTop: 7 }}><Mono size={11} color={UI.textMuted}>{job.schedule || "No schedule set"} - mode: {job.mode}</Mono></div>
                  {job.kind === "coding" && (
                    <div style={{ marginTop: 7, overflowWrap: "anywhere" }}>
                      <Mono size={11} color={UI.textMuted}>workspace: {job.workspacePath || "operator-selected"}</Mono>
                    </div>
                  )}
                  <div style={{ marginTop: 7, overflowWrap: "anywhere" }}><Mono size={11} color={UI.textMuted}>{job.instruction}</Mono></div>
                </div>
                <div className="flex gap-2 shrink-0 flex-wrap justify-end" style={{ maxWidth: "100%" }}>
                  <Button size="sm" onClick={() => toggleJob(job)}>{job.status === "enabled" ? "Pause" : "Enable"}</Button>
                  {job.approvalRequired && <Button size="sm" onClick={() => approveJob(job)}>Approve</Button>}
                  <Button size="sm" onClick={() => beginEdit(job)}>Edit</Button>
                  <Button size="sm" variant="danger" onClick={() => deleteJob(job)}>Delete</Button>
                </div>
              </div>
            ))}
          </div>
        )}
      </Card>
      <Mono size={11} color={UI.textMuted}>Or start from a template</Mono>
      <div className="grid gap-3" style={{ gridTemplateColumns: "repeat(auto-fit, minmax(320px, 1fr))" }}>
        {templates.map(([title, sub, schedule, kind]) => (
          <Card key={title}>
            <button
              type="button"
              onClick={() => beginNew({ name: title, instruction: sub, schedule, kind: kind as CronKind })}
              style={{
                width: "100%",
                textAlign: "left",
                border: 0,
                background: "transparent",
                padding: 0,
                cursor: "pointer",
              }}
            >
              <Mono size={13} color={UI.parchment} weight={600}>{title}</Mono>
              <div style={{ marginTop: 8 }}><Mono size={11} color={UI.textMuted}>{sub}</Mono></div>
              <div style={{ marginTop: 8 }}><Badge variant="neutral">{schedule}</Badge></div>
            </button>
          </Card>
        ))}
      </div>
    </SimpleSettingsSurface>
  );
};

const DreamingSurface = ({
  modes,
  activeMode,
  settings,
  onModeChange,
  onSettingsChange,
}: {
  modes: AssistantMode[];
  activeMode: string;
  settings: Record<string, ModeUiSetting>;
  onModeChange: (mode: string) => void;
  onSettingsChange: (modeId: string, patch: Partial<ModeUiSetting>) => void;
}) => {
  const dreamingMode = modes.find((mode) => mode.id === "dreaming") ?? {
    id: "dreaming",
    label: "Dreaming",
    description: "Advisory self-learning and reflection mode.",
    memory_scope: ["global", "mode:dreaming", "heartbeat", "hook"],
    rag_domains: [
      "self_learning_tree",
      "dreaming_reflections",
      "corrections",
      "failure_analysis",
      "event_logs",
      "promotion_candidates",
      "approved_lessons",
      "architecture_drift",
    ],
    allowed_tool_lanes: ["filesystem.read_", "knowledge.", "memory.list_", "logic.", "self_heal."],
    blocked_tool_capabilities: ["filesystem.write_file", "files.delete"],
    policies: ["advisory_only", "operator_approval_required_for_promotion", "no_hidden_autonomy"],
    planner_bias: [],
    persona: ["reflection_engine", "memory_governor"],
  };
  const ui = modeUiSetting(settings, dreamingMode);
  const [cadence, setCadence] = useState(() => {
    if (typeof window === "undefined") return "heartbeat";
    return window.localStorage.getItem("ordo:dreaming_cadence") || "heartbeat";
  });
  const [promotionGate, setPromotionGate] = useState(() => {
    if (typeof window === "undefined") return "operator";
    return window.localStorage.getItem("ordo:dreaming_promotion_gate") || "operator";
  });
  const setStored = (key: string, value: string, setter: (value: string) => void) => {
    setter(value);
    if (typeof window !== "undefined") window.localStorage.setItem(key, value);
    publishUxiDebugEvent("ordo.dreaming", "dreaming_setting_changed", "Dreaming setting changed.", {
      key,
      value,
    });
  };
  const tree = [
    ["Signals", "Corrections, failed jobs, rejected output, hook events, heartbeat reviews."],
    ["Reflections", "Short analysis of what happened, what worked, and what should change."],
    ["Promotion candidates", "Tentative lessons waiting for repeated evidence or operator confirmation."],
    ["Approved lessons", "Confirmed rules that can move into project, mode, or global memory."],
    ["Archive", "Cold history kept for audit and later review, not loaded by default."],
  ];
  return (
    <div className="h-full flex flex-col gap-4 overflow-auto pb-4">
      <SurfaceTitle
        kicker="ordo - dreaming"
        title="Dreaming"
        sub="A self-learning reflection mode with a tree-shaped RAG. It reviews evidence and proposes lessons; it does not silently change Ordo."
      />
      <Card>
        <div className="flex items-start justify-between gap-4 flex-wrap">
          <div style={{ maxWidth: 760 }}>
            <Mono size={11} upper track="0.18em" color={UI.textMuted}>
              mode status
            </Mono>
            <div style={{ marginTop: 7 }}>
              <Serif size={15} italic color={UI.textMuted}>
                Dreaming is on by default as a normal mode. It can use its own self-learning tree RAG,
                but promotions into durable rules require operator approval.
              </Serif>
            </div>
          </div>
          <div className="flex items-center gap-2">
            <Badge variant={ui.enabled ? "success" : "warn"}>{ui.enabled ? "enabled" : "paused"}</Badge>
            <Badge variant={activeMode === "dreaming" ? "primary" : "neutral"}>
              {activeMode === "dreaming" ? "active" : "available"}
            </Badge>
          </div>
        </div>
        <div className="grid gap-3" style={{ marginTop: 16, gridTemplateColumns: "repeat(auto-fit, minmax(300px, 1fr))" }}>
          <SettingsRow
            title="Mode enabled"
            sub="Keeps Dreaming visible and selectable."
            control={<ToggleSwitch checked={ui.enabled} onChange={(enabled) => onSettingsChange("dreaming", { enabled })} />}
          />
          <SettingsRow
            title="Use mode"
            sub="Switch the assistant into Dreaming."
            control={<Button variant={activeMode === "dreaming" ? "primary" : "secondary"} disabled={!ui.enabled} onClick={() => onModeChange("dreaming")}>{activeMode === "dreaming" ? "Current" : "Use Dreaming"}</Button>}
          />
          <SettingsRow
            title="Tree RAG budget"
            sub={`${ui.ragLimitMb} MB reserved for reflection context.`}
            control={<TextInput value={String(ui.ragLimitMb)} onChange={(value) => onSettingsChange("dreaming", { ragLimitMb: Math.max(0, Number(value) || 0) })} placeholder="2048" />}
          />
          <SettingsRow
            title="Cadence"
            sub="How Dreaming should normally be triggered."
            control={<SmallSelect value={cadence} onChange={(value) => setStored("ordo:dreaming_cadence", value, setCadence)} options={[
              { value: "heartbeat", label: "Heartbeat" },
              { value: "manual", label: "Manual only" },
              { value: "cron", label: "Cron job" },
            ]} />}
          />
          <SettingsRow
            title="Promotion gate"
            sub="Nothing durable is learned without this gate."
            control={<SmallSelect value={promotionGate} onChange={(value) => setStored("ordo:dreaming_promotion_gate", value, setPromotionGate)} options={[
              { value: "operator", label: "Operator approval" },
              { value: "three_hits", label: "3 hits + approval" },
              { value: "review_queue", label: "Review queue" },
            ]} />}
          />
        </div>
      </Card>
      <div className="grid gap-3" style={{ gridTemplateColumns: "repeat(auto-fit, minmax(260px, 1fr))" }}>
        {tree.map(([title, sub], index) => (
          <Card key={title}>
            <div className="flex items-center gap-2">
              <Badge variant="info">{index + 1}</Badge>
              <Mono size={13} color={UI.parchment} weight={700}>{title}</Mono>
            </div>
            <div style={{ marginTop: 10 }}>
              <Mono size={11} color={UI.textMuted}>{sub}</Mono>
            </div>
          </Card>
        ))}
      </div>
      <Card>
        <Mono size={11} upper track="0.18em" color={UI.textMuted}>
          RAG domains
        </Mono>
        <div className="flex flex-wrap gap-1.5" style={{ marginTop: 10 }}>
          {dreamingMode.rag_domains.map((domain) => (
            <Badge key={domain} variant={domain === "self_learning_tree" ? "primary" : "neutral"}>{domain}</Badge>
          ))}
        </div>
      </Card>
    </div>
  );
};

const DiagnosticSurface = ({
  modes,
  activeMode,
  settings,
  onModeChange,
  onSettingsChange,
}: {
  modes: AssistantMode[];
  activeMode: string;
  settings: Record<string, ModeUiSetting>;
  onModeChange: (mode: string) => void;
  onSettingsChange: (modeId: string, patch: Partial<ModeUiSetting>) => void;
}) => {
  const diagnosticMode = modes.find((mode) => mode.id === "diagnostic") ?? {
    id: "diagnostic",
    label: "Diagnostic",
    description: "Always-on Ordo self-diagnosis, repair planning, and bounded maintenance. Cloud models are denied by default unless allowed.",
    memory_scope: ["global", "mode:diagnostic"],
    rag_domains: [
      "diagnostic_self_learning_tree",
      "diagnostic_cases",
      "diagnostic_repair_log",
      "diagnostic_event_traces",
      "diagnostic_recommendations",
      "diagnostic_quarantine",
    ],
    allowed_tool_lanes: ["filesystem.read_", "knowledge.", "memory.list_", "memory.remember_", "logic.", "runtime.describe_", "files.", "self_heal.", "mcp.", "ssh.", "api.", "rest.", "automation.", "logs."],
    blocked_tool_capabilities: ["web.search", "web.strain", "filesystem.write_file", "files.delete", "runtime.update_settings", "automation.create", "automation.delete", "automation.approve", "automation.enable", "automation.disable", "automation.tick", "logs.clear", "logs.delete", "logs.write", "cloud.rest.request", "ssh.execute"],
    policies: ["cloud_models_denied_by_default", "diagnostic_rag_private", "no_web_access", "no_core_source_changes", "operator_approval_required_for_mutation"],
    planner_bias: [],
    persona: ["ordo_diagnostician", "maintenance_operator", "security_contained"],
    cross_mode_borrow_policy: "deny",
    cross_mode_consult_policy: "deny",
  };
  const ui = modeUiSetting(settings, diagnosticMode);
  const [maintenanceGate, setMaintenanceGate] = useState(() => {
    if (typeof window === "undefined") return "approval";
    return window.localStorage.getItem("ordo:diagnostic_maintenance_gate") || "approval";
  });
  const setStored = (key: string, value: string, setter: (value: string) => void) => {
    setter(value);
    if (typeof window !== "undefined") window.localStorage.setItem(key, value);
    publishUxiDebugEvent("ordo.diagnostic", "diagnostic_setting_changed", "Diagnostic setting changed.", {
      key,
      value,
    });
  };
  const containment = [
    ["Cloud denied by default", "Diagnostic refuses non-local credentials unless the operator flips the cloud-model toggle for the task."],
    ["Private memory", "New diagnostic facts stay in mode:diagnostic; other modes cannot borrow this mode."],
    ["No web", "Search, crawl, fetch, and web strain capabilities are blocked from this mode."],
    ["No core edits", "Core Rust, Tauri/WebView, hooks, security, and policy changes are recommendation-only."],
  ];
  const maintenance = [
    ["MCP and plugins", "Inspect, install, remove, and repair through approved maintenance routes."],
    ["Skills and modes", "Audit registrations and propose repairs without opening diagnostic memory to other modes."],
    ["Provider profiles", "Check local model credentials and compatible API descriptors without exposing secrets."],
    ["SSH and APIs", "Validate descriptors and connection metadata; command execution stays blocked."],
  ];
  return (
    <div className="h-full flex flex-col gap-4 overflow-auto pb-4">
      <SurfaceTitle
        kicker="ordo - diagnostic"
        title="Diagnostic"
        sub="Always-on self-diagnosis with wide visibility and bounded hands. It learns locally, recommends repairs, and keeps its own evidence tree isolated."
      />
      <Card>
        <div className="flex items-start justify-between gap-4 flex-wrap">
          <div style={{ maxWidth: 760 }}>
            <Mono size={11} upper track="0.18em" color={UI.textMuted}>
              containment status
            </Mono>
            <div style={{ marginTop: 7 }}>
              <Serif size={15} italic color={UI.textMuted}>
                Diagnostic is on by default, denies cloud models unless explicitly allowed, and can maintain peripheral configuration only through approved maintenance tools.
              </Serif>
            </div>
          </div>
          <div className="flex items-center gap-2">
            <Badge variant={ui.enabled ? "success" : "warn"}>{ui.enabled ? "enabled" : "paused"}</Badge>
            <Badge variant={activeMode === "diagnostic" ? "primary" : "neutral"}>
              {activeMode === "diagnostic" ? "active" : "available"}
            </Badge>
            <Badge variant="neutral">no web</Badge>
          </div>
        </div>
        <div className="grid gap-3" style={{ marginTop: 16, gridTemplateColumns: "repeat(auto-fit, minmax(300px, 1fr))" }}>
          <SettingsRow
            title="Mode enabled"
            sub="Keeps Diagnostic active and visible in Ordo."
            control={<ToggleSwitch checked={ui.enabled} onChange={(enabled) => onSettingsChange("diagnostic", { enabled })} />}
          />
          <SettingsRow
            title="Use mode"
            sub="Switch the assistant into Diagnostic."
            control={<Button variant={activeMode === "diagnostic" ? "primary" : "secondary"} disabled={!ui.enabled} onClick={() => onModeChange("diagnostic")}>{activeMode === "diagnostic" ? "Current" : "Use Diagnostic"}</Button>}
          />
          <SettingsRow
            title="Self-learning RAG budget"
            sub={`${ui.ragLimitMb} MB reserved for diagnostic evidence and repair history.`}
            control={<TextInput value={String(ui.ragLimitMb)} onChange={(value) => onSettingsChange("diagnostic", { ragLimitMb: Math.max(0, Number(value) || 0) })} placeholder="1024" />}
          />
          <SettingsRow
            title="Cloud model access"
            sub="Denied by default. Allow only when you want Diagnostic to use the selected cloud provider for this diagnostic task."
            control={
              <div className="flex items-center justify-end gap-2">
                <Badge variant={ui.allowCloudModels ? "warn" : "success"}>
                  {ui.allowCloudModels ? "cloud allowed" : "cloud denied"}
                </Badge>
                <ToggleSwitch
                  checked={ui.allowCloudModels === true}
                  onChange={(allowCloudModels) => onSettingsChange("diagnostic", { allowCloudModels })}
                />
              </div>
            }
          />
          <SettingsRow
            title="Maintenance gate"
            sub="Mutating repairs remain operator controlled."
            control={<SmallSelect value={maintenanceGate} onChange={(value) => setStored("ordo:diagnostic_maintenance_gate", value, setMaintenanceGate)} options={[
              { value: "approval", label: "Operator approval" },
              { value: "review_queue", label: "Review queue" },
              { value: "recommend_only", label: "Recommend only" },
            ]} />}
          />
        </div>
      </Card>
      <div className="grid gap-3" style={{ gridTemplateColumns: "repeat(auto-fit, minmax(260px, 1fr))" }}>
        {containment.map(([title, sub]) => (
          <Card key={title}>
            <div className="flex items-center gap-2">
              <ShieldCheck size={15} color={UI.lamp} />
              <Mono size={13} color={UI.parchment} weight={700}>{title}</Mono>
            </div>
            <div style={{ marginTop: 10 }}>
              <Mono size={11} color={UI.textMuted}>{sub}</Mono>
            </div>
          </Card>
        ))}
      </div>
      <Card>
        <Mono size={11} upper track="0.18em" color={UI.textMuted}>
          bounded maintenance surface
        </Mono>
        <div className="grid gap-2" style={{ marginTop: 12, gridTemplateColumns: "repeat(auto-fit, minmax(260px, 1fr))" }}>
          {maintenance.map(([title, sub]) => (
            <SettingsRow key={title} title={title} sub={sub} control={<Badge variant="neutral">gated</Badge>} />
          ))}
        </div>
      </Card>
      <Card>
        <Mono size={11} upper track="0.18em" color={UI.textMuted}>
          private RAG domains
        </Mono>
        <div className="flex flex-wrap gap-1.5" style={{ marginTop: 10 }}>
          {diagnosticMode.rag_domains.map((domain) => (
            <Badge key={domain} variant={domain === "diagnostic_self_learning_tree" ? "primary" : "neutral"}>{domain}</Badge>
          ))}
        </div>
      </Card>
    </div>
  );
};

const ProjectsSurface = ({
  scope,
  onScopeChange,
}: {
  scope: WorkspaceScope;
  onScopeChange: (scope: WorkspaceScope) => void;
}) => {
  const [toast, setToast] = useState<string | null>(null);
  const [localPathDraft, setLocalPathDraft] = useState(scope.localPath);
  const [cloudProviderDraft, setCloudProviderDraft] = useState<CloudWorkspaceProvider>(scope.cloudProvider);
  const [cloudRefDraft, setCloudRefDraft] = useState(scope.cloudRef);
  useEffect(() => {
    setLocalPathDraft(scope.localPath);
    setCloudProviderDraft(scope.cloudProvider);
    setCloudRefDraft(scope.cloudRef);
  }, [scope.localPath, scope.cloudProvider, scope.cloudRef]);
  const setScope = (patch: Partial<WorkspaceScope>) => {
    const next = normalizeWorkspaceScope({ ...scope, ...patch });
    onScopeChange(next);
  };
  const pickLocalFolder = async () => {
    try {
      const mod = await import("@tauri-apps/plugin-dialog");
      const picked = await mod.open({
        multiple: false,
        directory: true,
        title: "Select Ordo project workspace",
        defaultPath: scope.localPath || undefined,
      });
      if (typeof picked === "string" && picked.length > 0) {
        const label = picked.split(/[\\/]/).filter(Boolean).pop() ?? "Local project";
        setLocalPathDraft(picked);
        onScopeChange(
          normalizeWorkspaceScope({
            ...scope,
            kind: "local",
            label,
            localPath: picked,
            sandboxEnabled: true,
          }),
        );
        setToast(`Workspace selected: ${label}`);
      }
    } catch (err) {
      setToast(
        `folder picker unavailable: ${err instanceof Error ? err.message : String(err)}. Paste a path manually.`,
      );
    }
  };
  return (
    <SimpleSettingsSurface icon={<Briefcase size={22} />} title="Projects" sub="Choose the workspace Ordo should reason and operate against.">
      {toast && <Alert>{toast}</Alert>}
      <Card>
        <div className="flex items-start justify-between gap-4">
          <div>
            <Mono size={11} upper track="0.18em" color={UI.textMuted}>
              active workspace
            </Mono>
            <div style={{ marginTop: 8 }}>
              <Serif size={20} color={UI.parchment}>
                {workspaceScopeLabel(scope)}
              </Serif>
            </div>
            <div style={{ marginTop: 8 }}>
              <Mono size={11} color={UI.textMuted}>
                {scope.kind === "ordo"
                  ? "Using Ordo's internal memory and RAG."
                  : scope.kind === "local"
                  ? "Using the selected local project folder. Internal Ordo RAG is disabled by default for turns in this scope."
                  : "Using a cloud project reference. Internal Ordo RAG is disabled by default for turns in this scope."}
              </Mono>
            </div>
          </div>
          <Badge variant={scope.kind === "ordo" ? "neutral" : "success"}>
            {scope.kind === "ordo" ? "internal" : scope.sandboxEnabled ? "sandbox requested" : "unsandboxed"}
          </Badge>
        </div>
      </Card>

      <div className="grid gap-4" style={{ gridTemplateColumns: "repeat(auto-fit, minmax(320px, 1fr))" }}>
        <Card>
          <div className="flex items-start gap-3">
            <FolderOpen size={20} color={scope.kind === "ordo" ? UI.primary : UI.textMuted} />
            <div style={{ flex: 1 }}>
              <Mono size={13} color={UI.parchment} weight={700}>Ordo internal</Mono>
              <div style={{ marginTop: 8 }}><Mono size={11} color={UI.textMuted}>Use Ordo's own memory, RAG, files, and installed knowledge.</Mono></div>
              <div style={{ marginTop: 14 }}><Button size="sm" variant={scope.kind === "ordo" ? "primary" : "secondary"} onClick={() => setScope(DEFAULT_WORKSPACE_SCOPE)}>Use internal</Button></div>
            </div>
          </div>
        </Card>

        <Card>
          <div className="flex items-start gap-3">
            <FolderUp size={20} color={scope.kind === "local" ? UI.primary : UI.textMuted} />
            <div style={{ flex: 1, minWidth: 0 }}>
              <Mono size={13} color={UI.parchment} weight={700}>Local project folder</Mono>
              <div style={{ marginTop: 8 }}><Mono size={11} color={UI.textMuted}>Select a project root. Ordo should treat this folder as the project boundary.</Mono></div>
              <div className="flex gap-2" style={{ marginTop: 14 }}>
                <TextInput value={localPathDraft} onChange={setLocalPathDraft} placeholder="C:\\projects\\my-app" />
                <Button size="sm" onClick={() => void pickLocalFolder()}>Browse</Button>
              </div>
              <div style={{ marginTop: 10 }}><Button size="sm" variant={scope.kind === "local" ? "primary" : "secondary"} disabled={!localPathDraft.trim()} onClick={() => setScope({ kind: "local", localPath: localPathDraft, label: localPathDraft.split(/[\\/]/).filter(Boolean).pop() || "Local project" })}>Use local</Button></div>
            </div>
          </div>
        </Card>

        <Card>
          <div className="flex items-start gap-3">
            <Globe size={20} color={scope.kind === "cloud" ? UI.primary : UI.textMuted} />
            <div style={{ flex: 1, minWidth: 0 }}>
              <Mono size={13} color={UI.parchment} weight={700}>Cloud project</Mono>
              <div style={{ marginTop: 8 }}><Mono size={11} color={UI.textMuted}>Reference a GitHub repository or Hugging Face repo/dataset for future cloud sync/indexing.</Mono></div>
              <div className="grid gap-2" style={{ gridTemplateColumns: "150px minmax(0, 1fr)", marginTop: 14 }}>
                <Select
                  value={cloudProviderDraft}
                  onChange={(value) => setCloudProviderDraft(value as CloudWorkspaceProvider)}
                  options={[
                    { value: "github", label: "GitHub" },
                    { value: "huggingface", label: "Hugging Face" },
                  ]}
                />
                <TextInput value={cloudRefDraft} onChange={setCloudRefDraft} placeholder={cloudProviderDraft === "github" ? "owner/repo" : "org/name"} />
              </div>
              <div style={{ marginTop: 10 }}><Button size="sm" variant={scope.kind === "cloud" ? "primary" : "secondary"} disabled={!cloudRefDraft.trim()} onClick={() => setScope({ kind: "cloud", cloudProvider: cloudProviderDraft, cloudRef: cloudRefDraft })}>Use cloud</Button></div>
            </div>
          </div>
        </Card>
      </div>

      <SettingsList>
        <SettingsRow
          title="Sandbox project boundary"
          sub="When a local/cloud workspace is active, Ordo sends a strict root-boundary policy with each turn: no parent traversal and no reads/writes outside the selected root."
          control={<ToggleSwitch checked={scope.sandboxEnabled} onChange={(checked) => setScope({ sandboxEnabled: checked })} />}
        />
        <SettingsRow
          title="Allow writes inside workspace"
          sub="Off by default. Keep disabled for research/review sessions; enable only when you want Ordo to modify files inside the selected project."
          control={<ToggleSwitch checked={scope.allowWrites} onChange={(checked) => setScope({ allowWrites: checked })} />}
        />
        <SettingsRow
          title="Retrieval source"
          sub={scope.kind === "ordo" ? "Ordo internal RAG remains enabled." : "Selected workspace is the retrieval source; internal Ordo RAG is disabled by default for assistant turns."}
          control={<Badge variant={scope.kind === "ordo" ? "neutral" : "warn"}>{scope.kind === "ordo" ? "internal rag" : "workspace first"}</Badge>}
        />
      </SettingsList>
    </SimpleSettingsSurface>
  );
};

const ArtifactsSurface = () => {
  const [q, setQ] = useState("");
  const artifacts = [
    ["Research Report", "Structured analysis generated by Ordo.", "Last edited today"],
    ["Runtime Notes", "Implementation details and verification notes.", "Last edited yesterday"],
    ["Device Plan", "P2P and direct connection architecture.", "Last edited Jun 2"],
  ].filter(([name, desc]) => `${name} ${desc}`.toLowerCase().includes(q.toLowerCase()));
  return (
    <SimpleSettingsSurface icon={<FileText size={22} />} title="Artifacts" sub="Documents and outputs created by Ordo.">
      <div className="flex justify-between gap-3"><TextInput value={q} onChange={setQ} placeholder="Search artifacts..." /><Button variant="primary" onClick={() => publishUxiDebugEvent("ordo.artifacts", "new_artifact_requested", "New artifact requested.")}>New artifact</Button></div>
      <div className="grid gap-4" style={{ gridTemplateColumns: "repeat(auto-fit, minmax(260px, 1fr))" }}>
        {artifacts.map(([name, desc, updated]) => (
          <Card key={name} padded={false} style={{ overflow: "hidden" }}>
            <div style={{ minHeight: 150, padding: 18, borderBottom: `1px solid ${UI.cardBorder}` }}><Serif size={14} color={UI.parchment}>{desc}</Serif></div>
            <div style={{ padding: 16 }}><Mono size={13} color={UI.parchment} weight={600}>{name}</Mono><div style={{ marginTop: 8 }}><Mono size={10} color={UI.textDim}>{updated}</Mono></div></div>
          </Card>
        ))}
      </div>
    </SimpleSettingsSurface>
  );
};

const ArchivedChatsSurface = () => {
  const [q, setQ] = useState("");
  const chats = [
    ["Set up automation", "2 Jun 2026, 14:20 - Ordo-main"],
    ["Polish VRAM Bridge", "27 May 2026, 9:04 - device-main"],
    ["Analyze project hybrids", "2 May 2026, 10:06 - Ordo-dev"],
  ].filter(([name, meta]) => `${name} ${meta}`.toLowerCase().includes(q.toLowerCase()));
  return (
    <SimpleSettingsSurface icon={<Archive size={22} />} title="Archived chats" sub="Archived Ordo conversations.">
      <TextInput value={q} onChange={setQ} placeholder="Search archived chats" />
      <SettingsList>
        {chats.map(([name, meta]) => (
          <SettingsRow key={`${name}-${meta}`} title={name} sub={meta} control={<div className="flex gap-2 justify-end"><Button size="sm" variant="ghost" onClick={() => publishUxiDebugEvent("ordo.archives", "archived_chat_delete_requested", "Archived chat delete requested.", { name, meta })}><Trash2 size={13} /></Button><Button size="sm" onClick={() => publishUxiDebugEvent("ordo.archives", "archived_chat_unarchive_requested", "Archived chat unarchive requested.", { name, meta })}>Unarchive</Button></div>} />
        ))}
      </SettingsList>
      <div className="flex justify-end"><Button variant="danger" onClick={() => publishUxiDebugEvent("ordo.archives", "delete_all_archived_chats_requested", "Delete all archived chats requested.", { visible_count: chats.length }, "WARN")}>Delete all</Button></div>
    </SimpleSettingsSurface>
  );
};

const DocsSurface = () => (
  <SimpleSettingsSurface
    icon={<BookMarked size={22} />}
    title="Docs"
    sub="Operator documentation for using Ordo day to day."
  >
    <div className="grid gap-4" style={{ gridTemplateColumns: "repeat(auto-fit, minmax(300px, 1fr))" }}>
      {[
        ["User guide", "Start with docs/user-guide.md for provider setup, modes, automation, dreaming, diagnostic mode, sessions, workspaces, and exports."],
        ["Getting started", "Choose a provider, pick a workspace, select a mode, then use Assistant as the control surface."],
        ["Workspaces", "Use Ordo internal for built-in memory/RAG, local project for a folder sandbox, or cloud project for GitHub and Hugging Face references."],
        ["Modes", "Modes are bounded workspaces. Optional RAG domains start disabled with 0 MB until explicitly enabled."],
        ["Skills, plugins, and MCP", "Skills teach behavior, plugins package capabilities, and MCP servers expose external tools. Keep plugin and MCP tabs separate."],
        ["Hooks", "Hooks are lifecycle guardrails. They can be global or per-mode and should log their actions to the event/debug trail."],
        ["Exports", "Use the chatbox download button to export visible conversation work as Markdown."],
      ].map(([title, body]) => (
        <Card key={title}>
          <Mono size={13} color={UI.parchment} weight={700}>{title}</Mono>
          <div style={{ marginTop: 10 }}><Mono size={11} color={UI.textMuted}>{body}</Mono></div>
        </Card>
      ))}
    </div>
    <SettingsList>
      <SettingsRow
        title="Canonical local docs"
        sub="docs/user-guide.md is the canonical operator guide. README.md, README-BETA.md, UXI_SOURCE_MAP.md, and LINUX_BUILD.md provide supporting context."
        control={<Badge variant="neutral">operator</Badge>}
      />
      <SettingsRow
        title="Where this tab belongs"
        sub="Docs stays at the bottom of the left rail so operators can always find usage guidance without entering Settings."
        control={<Badge variant="success">surfaced</Badge>}
      />
    </SettingsList>
  </SimpleSettingsSurface>
);

const DevDocsSurface = () => (
  <SimpleSettingsSurface
    icon={<FileText size={22} />}
    title="Dev Docs"
    sub="Developer notes for rebuilding, extending, and safely recovering Ordo."
  >
    <div className="grid gap-4" style={{ gridTemplateColumns: "repeat(auto-fit, minmax(300px, 1fr))" }}>
      {[
        ["Developer guide", "docs/developer-guide.md is the canonical developer entry point for architecture, checks, crate map, modes, automation, and release hygiene."],
        ["UXI recovery notes", "UXI_DEV_NOTES.md documents the current official UXI behavior, what worked, and what should not be rebuilt differently."],
        ["Project instructions", "AGENTS.md and CLAUDE.md carry project-wide rules, including no Rust patching and workspace ownership expectations."],
        ["Architecture map", "UXI_SOURCE_MAP.md points future sessions to the canonical Ordo Studio shell files."],
        ["Verification gates", "Run npm build and check:tauri after UXI work. Rust runtime changes need the relevant cargo checks/tests."],
        ["Workspace sandboxing", "The UXI sends workspace_scope metadata; runtime/tool adapters must enforce the root boundary before filesystem access."],
        ["Design rule", "New surfaces should match Ordo's compact dark/lamp visual language and avoid adding parallel design systems."],
      ].map(([title, body]) => (
        <Card key={title}>
          <Mono size={13} color={UI.parchment} weight={700}>{title}</Mono>
          <div style={{ marginTop: 10 }}><Mono size={11} color={UI.textMuted}>{body}</Mono></div>
        </Card>
      ))}
    </div>
    <SettingsList>
      <SettingsRow
        title="Primary developer docs"
        sub="docs/developer-guide.md is the first developer entry point; UXI_DEV_NOTES.md remains the recovery note for shell-specific behavior."
        control={<Badge variant="warn">required</Badge>}
      />
      <SettingsRow
        title="No patches rule"
        sub="Rust work must trace the design and rebuild correctly. Do not patch Rust modules as a shortcut."
        control={<Badge variant="danger">strict</Badge>}
      />
    </SettingsList>
  </SimpleSettingsSurface>
);

type HookEventId =
  | "PreToolUse"
  | "PostToolUse"
  | "PermissionRequest"
  | "SessionStart"
  | "Stop"
  | "UserPromptSubmit"
  | "PreCompact"
  | "PostCompact"
  | "SubagentStart"
  | "SubagentStop";

type HookDecision = "deny" | "allow" | "context";
type HookScope = "global" | "mode";

interface ManagedHook {
  id: string;
  name: string;
  scope: HookScope;
  modeId: string;
  event: HookEventId;
  matcher: string;
  decision: HookDecision;
  message: string;
  fileFilter: string;
  timeout: number;
  enabled: boolean;
}

interface HookDebugEvent {
  id: string;
  ts: string;
  level: "INFO" | "WARN" | "ERROR";
  action: string;
  message: string;
  hook_id?: string;
  hook_name?: string;
  event?: HookEventId;
  decision?: HookDecision;
  matcher?: string;
  file_filter?: string;
  scope?: HookScope;
  mode_id?: string;
  enabled?: boolean;
}

const HOOKS_STORAGE_KEY = "ordo:settings_hooks";
const HOOK_EVENTS_STORAGE_KEY = "ordo:settings_hook_events";
const HOOK_EVENT_LOG_CAP = 80;

const HOOK_EVENTS: Array<{ value: HookEventId; label: string; desc: string }> = [
  { value: "PreToolUse", label: "Pre Tool Use", desc: "Before a tool runs" },
  { value: "PostToolUse", label: "Post Tool Use", desc: "After a tool runs" },
  { value: "PermissionRequest", label: "Permission Request", desc: "When approval is needed" },
  { value: "SessionStart", label: "Session Start", desc: "When a session begins" },
  { value: "Stop", label: "Stop", desc: "When the model stops" },
  { value: "UserPromptSubmit", label: "User Prompt", desc: "When user sends a prompt" },
  { value: "PreCompact", label: "Pre Compact", desc: "Before conversation compaction" },
  { value: "PostCompact", label: "Post Compact", desc: "After conversation compaction" },
  { value: "SubagentStart", label: "Subagent Start", desc: "When a subagent starts" },
  { value: "SubagentStop", label: "Subagent Stop", desc: "When a subagent stops" },
];

const HOOK_MATCHER_PRESETS = [
  { value: "", label: "All" },
  { value: "Bash", label: "Bash" },
  { value: "apply_patch|Edit|Write", label: "File edits" },
  { value: "^apply_patch$", label: "apply_patch" },
  { value: "Edit|Write", label: "Edit / Write" },
  { value: "startup|resume", label: "Startup / resume" },
  { value: "manual|auto", label: "Manual / auto" },
];

const HOOK_DECISIONS: Array<{ value: HookDecision; label: string; sub: string }> = [
  { value: "deny", label: "Deny", sub: "Block the action" },
  { value: "allow", label: "Allow", sub: "Approve silently" },
  { value: "context", label: "Add context", sub: "Inject guidance" },
];

const SAMPLE_MANAGED_HOOKS: ManagedHook[] = [
  {
    id: "sample-rust-patches",
    name: "Block Rust Patches",
    scope: "global",
    modeId: "",
    event: "PreToolUse",
    matcher: "apply_patch|Edit|Write",
    decision: "deny",
    message: "Do not patch .rs files. Rebuild the affected Rust implementation natively.",
    fileFilter: ".rs",
    timeout: 10,
    enabled: true,
  },
  {
    id: "sample-cargo-context",
    name: "Warn on Cargo.toml edits",
    scope: "global",
    modeId: "",
    event: "PreToolUse",
    matcher: "apply_patch|Edit|Write",
    decision: "context",
    message: "Cargo.toml is being modified. Check dependency versions and feature flags before proceeding.",
    fileFilter: "Cargo.toml",
    timeout: 10,
    enabled: true,
  },
];

const blankManagedHook = (): ManagedHook => ({
  id:
    typeof crypto !== "undefined" && "randomUUID" in crypto
      ? crypto.randomUUID()
      : `${Date.now().toString(36)}${Math.random().toString(36).slice(2, 6)}`,
  name: "",
  scope: "global",
  modeId: "",
  event: "PreToolUse",
  matcher: "",
  decision: "deny",
  message: "",
  fileFilter: "",
  timeout: 10,
  enabled: true,
});

const normalizeManagedHook = (raw: Partial<ManagedHook>): ManagedHook => {
  const fallback = blankManagedHook();
  const scope = raw.scope === "mode" ? "mode" : "global";
  return {
    ...fallback,
    ...raw,
    scope,
    modeId: scope === "mode" ? raw.modeId ?? "" : "",
    timeout: typeof raw.timeout === "number" && Number.isFinite(raw.timeout) ? raw.timeout : fallback.timeout,
    enabled: typeof raw.enabled === "boolean" ? raw.enabled : true,
  };
};

const loadManagedHooks = (): ManagedHook[] => {
  if (typeof window === "undefined") return SAMPLE_MANAGED_HOOKS;
  try {
    const raw = window.localStorage.getItem(HOOKS_STORAGE_KEY);
    if (!raw) return SAMPLE_MANAGED_HOOKS;
    const parsed = JSON.parse(raw);
    return Array.isArray(parsed) ? parsed.map((item) => normalizeManagedHook(item)) : SAMPLE_MANAGED_HOOKS;
  } catch {
    return SAMPLE_MANAGED_HOOKS;
  }
};

const saveManagedHooks = (hooks: ManagedHook[]) => {
  if (typeof window === "undefined") return;
  window.localStorage.setItem(HOOKS_STORAGE_KEY, JSON.stringify(hooks));
};

const loadHookDebugEvents = (): HookDebugEvent[] => {
  if (typeof window === "undefined") return [];
  try {
    const raw = window.localStorage.getItem(HOOK_EVENTS_STORAGE_KEY);
    if (!raw) return [];
    const parsed = JSON.parse(raw);
    return Array.isArray(parsed) ? parsed as HookDebugEvent[] : [];
  } catch {
    return [];
  }
};

const saveHookDebugEvents = (events: HookDebugEvent[]) => {
  if (typeof window === "undefined") return;
  window.localStorage.setItem(HOOK_EVENTS_STORAGE_KEY, JSON.stringify(events.slice(0, HOOK_EVENT_LOG_CAP)));
};

const hookSlug = (name: string): string =>
  (name.trim().toLowerCase().replace(/[^a-z0-9]+/g, "_").replace(/^_+|_+$/g, "") || "hook");

type ExportedHookGroup = {
  matcher?: string;
  hooks: Array<{ type: string; command: string; timeout: number }>;
};

type ExportedHookBucket = Record<string, ExportedHookGroup[]>;

const addHookToExportBucket = (bucket: ExportedHookBucket, hook: ManagedHook) => {
    const command = `python3 "$(git rev-parse --show-toplevel 2>/dev/null || echo .)/.ordo/hooks/${hookSlug(hook.name)}.py"`;
    const group: ExportedHookGroup = {
      hooks: [{ type: "command", command, timeout: hook.timeout }],
    };
    if (hook.matcher.trim()) group.matcher = hook.matcher.trim();
    bucket[hook.event] = [...(bucket[hook.event] ?? []), group];
};

const generateHooksConfig = (hooks: ManagedHook[]): string => {
  const config: {
    global: { hooks: ExportedHookBucket };
    modes: Record<string, { hooks: ExportedHookBucket }>;
  } = { global: { hooks: {} }, modes: {} };
  for (const hook of hooks.filter((item) => item.enabled)) {
    if (hook.scope === "mode" && hook.modeId.trim()) {
      const modeBucket = config.modes[hook.modeId] ?? { hooks: {} };
      addHookToExportBucket(modeBucket.hooks, hook);
      config.modes[hook.modeId] = modeBucket;
    } else {
      addHookToExportBucket(config.global.hooks, hook);
    }
  }
  return JSON.stringify(config, null, 2);
};

const decisionColor = (decision: HookDecision): string =>
  decision === "deny" ? UI.red : decision === "allow" ? UI.jade : UI.amber;

const hookDebugEvent = (
  action: string,
  message: string,
  hook?: ManagedHook,
  level: HookDebugEvent["level"] = "INFO",
): HookDebugEvent => ({
  id:
    typeof crypto !== "undefined" && "randomUUID" in crypto
      ? crypto.randomUUID()
      : `${Date.now().toString(36)}${Math.random().toString(36).slice(2, 6)}`,
  ts: new Date().toISOString(),
  level,
  action,
  message,
  hook_id: hook?.id,
  hook_name: hook?.name,
  event: hook?.event,
  decision: hook?.decision,
  matcher: hook?.matcher,
  file_filter: hook?.fileFilter,
  scope: hook?.scope,
  mode_id: hook?.scope === "mode" ? hook.modeId : undefined,
  enabled: hook?.enabled,
});

const publishHookDebugEvent = (entry: HookDebugEvent) => {
  if (typeof window !== "undefined") {
    window.dispatchEvent(
      new CustomEvent("ordo:debug-event", {
        detail: {
          source: "ordo.hooks",
          topic: `ordo.hooks.${entry.action}`,
          ...entry,
        },
      }),
    );
  }
  console.debug("[ordo.hooks]", entry);
};

const HookManagerSurface = ({ modes }: { modes: AssistantMode[] }) => {
  const [hooks, setHooks] = useState<ManagedHook[]>(loadManagedHooks);
  const [editing, setEditing] = useState<ManagedHook | "new" | null>(null);
  const [draft, setDraft] = useState<ManagedHook>(() => blankManagedHook());
  const [filter, setFilter] = useState<HookEventId | "all">("all");
  const [scopeFilter, setScopeFilter] = useState<"all" | "global" | `mode:${string}`>("all");
  const [showExport, setShowExport] = useState(false);
  const [events, setEvents] = useState<HookDebugEvent[]>(loadHookDebugEvents);
  const modeOptions = modes.length > 0 ? modes : [{ id: "general", label: "General Assistant" } as AssistantMode];
  const modeLabel = (modeId: string) =>
    modeOptions.find((mode) => mode.id === modeId)?.label ?? modeId;

  const recordEvent = (
    action: string,
    message: string,
    hook?: ManagedHook,
    level: HookDebugEvent["level"] = "INFO",
  ) => {
    const entry = hookDebugEvent(action, message, hook, level);
    setEvents((prev) => {
      const next = [entry, ...prev].slice(0, HOOK_EVENT_LOG_CAP);
      saveHookDebugEvents(next);
      return next;
    });
    publishHookDebugEvent(entry);
  };

  const persist = (next: ManagedHook[]) => {
    setHooks(next);
    saveManagedHooks(next);
  };

  const openNew = () => {
    setDraft({
      ...blankManagedHook(),
      scope: scopeFilter.startsWith("mode:") ? "mode" : scopeFilter === "global" ? "global" : "global",
      modeId: scopeFilter.startsWith("mode:") ? scopeFilter.slice("mode:".length) : "",
    });
    setEditing("new");
    recordEvent("editor_opened", "Opened new hook editor.");
  };

  const openEdit = (hook: ManagedHook) => {
    setDraft({ ...hook });
    setEditing(hook);
    recordEvent("editor_opened", `Opened hook editor for "${hook.name}".`, hook);
  };

  const closeEditor = () => {
    setEditing(null);
    setDraft(blankManagedHook());
    recordEvent("editor_closed", "Closed hook editor.");
  };

  const saveDraft = () => {
    const normalized: ManagedHook = {
      ...draft,
      name: draft.name.trim(),
      scope: draft.scope,
      modeId: draft.scope === "mode" ? draft.modeId || modeOptions[0]?.id || "general" : "",
      matcher: draft.matcher.trim(),
      message: draft.message.trim(),
      fileFilter: draft.fileFilter.trim(),
      timeout: Math.max(1, Math.min(600, Number.isFinite(draft.timeout) ? draft.timeout : 10)),
    };
    if (!normalized.name || !normalized.message) return;
    const next =
      editing === "new"
        ? [...hooks, normalized]
        : hooks.map((hook) => (hook.id === normalized.id ? normalized : hook));
    persist(next);
    recordEvent(
      editing === "new" ? "hook_created" : "hook_updated",
      `${editing === "new" ? "Created" : "Updated"} hook "${normalized.name}".`,
      normalized,
    );
    closeEditor();
  };

  const toggleHook = (id: string) => {
    let changed: ManagedHook | undefined;
    persist(hooks.map((hook) => {
      if (hook.id !== id) return hook;
      changed = { ...hook, enabled: !hook.enabled };
      return changed;
    }));
    if (changed) {
      recordEvent(
        changed.enabled ? "hook_enabled" : "hook_disabled",
        `${changed.enabled ? "Enabled" : "Disabled"} hook "${changed.name}".`,
        changed,
        changed.enabled ? "INFO" : "WARN",
      );
    }
  };

  const deleteHook = (id: string) => {
    const target = hooks.find((hook) => hook.id === id);
    if (!window.confirm("Delete this hook?")) return;
    persist(hooks.filter((hook) => hook.id !== id));
    if (target) {
      recordEvent("hook_deleted", `Deleted hook "${target.name}".`, target, "WARN");
    }
  };

  const toggleExport = () => {
    setShowExport((value) => {
      const next = !value;
      recordEvent(
        next ? "export_opened" : "export_closed",
        next ? "Opened hooks.json export preview." : "Closed hooks.json export preview.",
      );
      return next;
    });
  };

  const clearEventLog = () => {
    setEvents([]);
    saveHookDebugEvents([]);
    publishHookDebugEvent(hookDebugEvent("event_log_cleared", "Cleared Hook Manager event log."));
  };

  const usedEvents = Array.from(new Set(hooks.map((hook) => hook.event)));
  const visibleHooks = hooks.filter((hook) => {
    const eventMatches = filter === "all" || hook.event === filter;
    const scopeMatches =
      scopeFilter === "all" ||
      (scopeFilter === "global" && hook.scope === "global") ||
      (scopeFilter.startsWith("mode:") && hook.scope === "mode" && hook.modeId === scopeFilter.slice("mode:".length));
    return eventMatches && scopeMatches;
  });
  const showMatcher = !["Stop", "UserPromptSubmit"].includes(draft.event);
  const canSave = draft.name.trim().length > 0 && draft.message.trim().length > 0;

  return (
    <div className="h-full flex flex-col gap-4 overflow-auto pb-4">
      <SurfaceTitle
        kicker="ordo - hooks"
        title="Hook Manager"
        sub="Global and per-mode lifecycle guardrails for tools, permissions, compaction, sessions, and subagents."
      />

      <div className="flex items-center justify-between gap-3 flex-wrap">
        <div className="flex items-center gap-1.5 flex-wrap">
          <Button onClick={() => setScopeFilter("all")} size="sm" variant={scopeFilter === "all" ? "primary" : "secondary"}>
            All scopes ({hooks.length})
          </Button>
          <Button onClick={() => setScopeFilter("global")} size="sm" variant={scopeFilter === "global" ? "primary" : "secondary"}>
            Global ({hooks.filter((hook) => hook.scope === "global").length})
          </Button>
          {modeOptions.map((mode) => (
            <Button
              key={mode.id}
              onClick={() => setScopeFilter(`mode:${mode.id}`)}
              size="sm"
              variant={scopeFilter === `mode:${mode.id}` ? "primary" : "secondary"}
            >
              {mode.label} ({hooks.filter((hook) => hook.scope === "mode" && hook.modeId === mode.id).length})
            </Button>
          ))}
        </div>
      </div>

      <div className="flex items-center justify-between gap-3 flex-wrap">
        <div className="flex items-center gap-1.5 flex-wrap">
          <Button onClick={() => setFilter("all")} size="sm" variant={filter === "all" ? "primary" : "secondary"}>
            All ({hooks.length})
          </Button>
          {usedEvents.map((event) => (
            <Button
              key={event}
              onClick={() => setFilter(event)}
              size="sm"
              variant={filter === event ? "primary" : "secondary"}
            >
              {event}
            </Button>
          ))}
        </div>
        <div className="flex items-center gap-2">
          <Button onClick={toggleExport} size="sm">
            {showExport ? "Hide export" : "Export config"}
          </Button>
          <Button onClick={openNew} variant="primary" size="md">
            <span style={{ display: "inline-flex", alignItems: "center", gap: 6 }}>
              <Plus size={13} /> Add Hook
            </span>
          </Button>
        </div>
      </div>

      {editing && (
        <Card>
          <div style={{ borderTop: `2px solid ${UI.jade}`, paddingTop: 14 }}>
            <Mono size={11} upper track="0.18em" color={UI.textMuted}>
              {editing === "new" ? "new hook" : "edit hook"}
            </Mono>
            <div className="grid gap-4" style={{ marginTop: 16 }}>
              <Field label="Hook name" required>
                <TextInput
                  value={draft.name}
                  onChange={(value) => setDraft((hook) => ({ ...hook, name: value }))}
                  placeholder="Block Rust Patches"
                />
              </Field>

              <Field label="Scope" required hint="Global hooks run everywhere. Mode hooks only apply when that Ordo mode is active.">
                <div className="grid gap-2" style={{ gridTemplateColumns: "repeat(auto-fit, minmax(180px, 1fr))" }}>
                  <button
                    type="button"
                    onClick={() => setDraft((hook) => ({ ...hook, scope: "global", modeId: "" }))}
                    style={{
                      textAlign: "left",
                      borderRadius: 6,
                      border: `1px solid ${draft.scope === "global" ? UI.primaryBorder : UI.cardBorder}`,
                      background: draft.scope === "global" ? UI.primarySoft : UI.cardBgRaised,
                      padding: "10px 12px",
                      cursor: "pointer",
                    }}
                  >
                    <Mono size={11} color={draft.scope === "global" ? UI.primary : UI.parchment} weight={600}>
                      Global
                    </Mono>
                    <div style={{ marginTop: 3 }}><Mono size={9} color={UI.textDim}>Applies to every mode.</Mono></div>
                  </button>
                  <button
                    type="button"
                    onClick={() => setDraft((hook) => ({ ...hook, scope: "mode", modeId: hook.modeId || modeOptions[0]?.id || "general" }))}
                    style={{
                      textAlign: "left",
                      borderRadius: 6,
                      border: `1px solid ${draft.scope === "mode" ? UI.primaryBorder : UI.cardBorder}`,
                      background: draft.scope === "mode" ? UI.primarySoft : UI.cardBgRaised,
                      padding: "10px 12px",
                      cursor: "pointer",
                    }}
                  >
                    <Mono size={11} color={draft.scope === "mode" ? UI.primary : UI.parchment} weight={600}>
                      Per mode
                    </Mono>
                    <div style={{ marginTop: 3 }}><Mono size={9} color={UI.textDim}>Applies to one selected mode.</Mono></div>
                  </button>
                </div>
              </Field>

              {draft.scope === "mode" && (
                <Field label="Mode" required>
                  <Select
                    value={draft.modeId || modeOptions[0]?.id || "general"}
                    onChange={(modeId) => setDraft((hook) => ({ ...hook, modeId }))}
                    options={modeOptions.map((mode) => ({ value: mode.id, label: mode.label }))}
                  />
                </Field>
              )}

              <Field label="Event" required>
                <div className="grid gap-2" style={{ gridTemplateColumns: "repeat(auto-fit, minmax(190px, 1fr))" }}>
                  {HOOK_EVENTS.map((event) => {
                    const active = draft.event === event.value;
                    return (
                      <button
                        key={event.value}
                        type="button"
                        onClick={() => setDraft((hook) => ({ ...hook, event: event.value }))}
                        style={{
                          textAlign: "left",
                          borderRadius: 6,
                          border: `1px solid ${active ? UI.primaryBorder : UI.cardBorder}`,
                          background: active ? UI.primarySoft : UI.cardBgRaised,
                          padding: "10px 12px",
                          cursor: "pointer",
                        }}
                      >
                        <Mono size={11} color={active ? UI.primary : UI.parchment} weight={600}>
                          {event.label}
                        </Mono>
                        <div style={{ marginTop: 3 }}>
                          <Mono size={9} color={UI.textDim}>
                            {event.desc}
                          </Mono>
                        </div>
                      </button>
                    );
                  })}
                </div>
              </Field>

              {showMatcher && (
                <Field label="Matcher pattern" hint="Regex-style filter for the tool or lifecycle matcher. Leave empty for all.">
                  <div className="flex flex-wrap gap-1.5" style={{ marginBottom: 8 }}>
                    {HOOK_MATCHER_PRESETS.map((preset) => (
                      <Button
                        key={preset.label}
                        size="sm"
                        variant={draft.matcher === preset.value ? "primary" : "secondary"}
                        onClick={() => setDraft((hook) => ({ ...hook, matcher: preset.value }))}
                      >
                        {preset.label}
                      </Button>
                    ))}
                  </div>
                  <TextInput
                    value={draft.matcher}
                    onChange={(value) => setDraft((hook) => ({ ...hook, matcher: value }))}
                    placeholder="mcp__filesystem__.*"
                  />
                </Field>
              )}

              <Field label="File filter" hint="Optional file extension or filename gate.">
                <TextInput
                  value={draft.fileFilter}
                  onChange={(value) => setDraft((hook) => ({ ...hook, fileFilter: value }))}
                  placeholder=".rs, Cargo.toml, .py"
                />
              </Field>

              <Field label="Action" required>
                <div className="grid gap-2" style={{ gridTemplateColumns: "repeat(auto-fit, minmax(150px, 1fr))" }}>
                  {HOOK_DECISIONS.map((decision) => {
                    const active = draft.decision === decision.value;
                    const color = decisionColor(decision.value);
                    return (
                      <button
                        key={decision.value}
                        type="button"
                        onClick={() => setDraft((hook) => ({ ...hook, decision: decision.value }))}
                        style={{
                          textAlign: "left",
                          borderRadius: 6,
                          border: `1px solid ${active ? `${color}88` : UI.cardBorder}`,
                          background: active ? `${color}18` : UI.cardBgRaised,
                          padding: "10px 12px",
                          cursor: "pointer",
                        }}
                      >
                        <Mono size={11} color={active ? color : UI.parchment} weight={600}>
                          {decision.label}
                        </Mono>
                        <div style={{ marginTop: 3 }}>
                          <Mono size={9} color={UI.textDim}>
                            {decision.sub}
                          </Mono>
                        </div>
                      </button>
                    );
                  })}
                </div>
              </Field>

              <Field
                label={draft.decision === "deny" ? "Denial reason" : draft.decision === "allow" ? "Approval note" : "Context to inject"}
                required
              >
                <Textarea
                  value={draft.message}
                  onChange={(value) => setDraft((hook) => ({ ...hook, message: value }))}
                  rows={3}
                  placeholder="Do not patch .rs files. Rebuild the affected implementation natively."
                />
              </Field>

              <Field label="Timeout" hint="Seconds, clamped between 1 and 600.">
                <NumberInput
                  value={draft.timeout}
                  onChange={(value) => setDraft((hook) => ({ ...hook, timeout: value }))}
                  min={1}
                  max={600}
                  step={1}
                />
              </Field>

              <Checkbox
                checked={draft.enabled}
                onChange={(enabled) => setDraft((hook) => ({ ...hook, enabled }))}
                label="Enabled"
              />
            </div>
            <div className="flex items-center gap-2" style={{ marginTop: 18 }}>
              <Button onClick={saveDraft} variant="primary" disabled={!canSave}>
                {editing === "new" ? "Add hook" : "Save changes"}
              </Button>
              <Button onClick={closeEditor}>Cancel</Button>
            </div>
          </div>
        </Card>
      )}

      <div className="grid gap-3" style={{ gridTemplateColumns: "repeat(auto-fit, minmax(340px, 1fr))" }}>
        {visibleHooks.map((hook) => {
          const color = decisionColor(hook.decision);
          const decision = HOOK_DECISIONS.find((item) => item.value === hook.decision);
          return (
            <Card key={hook.id} padded={false} style={{ opacity: hook.enabled ? 1 : 0.55, overflow: "hidden" }}>
              <div style={{ height: 3, background: color }} />
              <div style={{ padding: 16 }}>
                <div className="flex items-start justify-between gap-3">
                  <div style={{ minWidth: 0 }}>
                    <Mono size={13} color={UI.parchment} weight={600}>
                      {hook.name}
                    </Mono>
                    <div className="flex items-center gap-1.5 flex-wrap" style={{ marginTop: 8 }}>
                      <Badge variant={hook.scope === "global" ? "primary" : "success"}>
                        {hook.scope === "global" ? "Global" : `Mode: ${modeLabel(hook.modeId)}`}
                      </Badge>
                      <Badge variant="neutral">{hook.event}</Badge>
                      {hook.matcher && <Badge variant="info">/{hook.matcher}/</Badge>}
                      {hook.fileFilter && <Badge variant="warn">{hook.fileFilter}</Badge>}
                    </div>
                  </div>
                  <Button
                    onClick={() => toggleHook(hook.id)}
                    size="sm"
                    variant={hook.enabled ? "primary" : "secondary"}
                    title={hook.enabled ? "Disable hook" : "Enable hook"}
                  >
                    {hook.enabled ? <Pause size={13} /> : <Play size={13} />}
                  </Button>
                </div>
                <div className="flex items-center gap-2" style={{ marginTop: 12 }}>
                  <span
                    style={{
                      fontFamily: MONO,
                      fontSize: 10,
                      fontWeight: 700,
                      textTransform: "uppercase",
                      letterSpacing: "0.12em",
                      color,
                    }}
                  >
                    {decision?.label ?? hook.decision}
                  </span>
                  <Mono size={10} color={UI.textDim}>
                    timeout: {hook.timeout}s
                  </Mono>
                </div>
                <div style={{ marginTop: 10, minHeight: 38 }}>
                  <Mono size={11} color={UI.textMuted}>
                    "{hook.message}"
                  </Mono>
                </div>
                <div className="flex items-center gap-2" style={{ marginTop: 14 }}>
                  <Button onClick={() => openEdit(hook)} size="sm">
                    <span style={{ display: "inline-flex", alignItems: "center", gap: 4 }}>
                      <Wrench size={12} /> Edit
                    </span>
                  </Button>
                  <Button onClick={() => deleteHook(hook.id)} size="sm" variant="danger">
                    <span style={{ display: "inline-flex", alignItems: "center", gap: 4 }}>
                      <Trash2 size={12} /> Delete
                    </span>
                  </Button>
                </div>
              </div>
            </Card>
          );
        })}
      </div>

      {visibleHooks.length === 0 && (
        <Card>
          <div style={{ textAlign: "center", padding: "24px 0" }}>
            <Serif size={15} italic color={UI.textMuted}>
              No hooks configured for this filter.
            </Serif>
            <div style={{ marginTop: 10 }}>
              <Button onClick={openNew} variant="primary">Add your first hook</Button>
            </div>
          </div>
        </Card>
      )}

      {showExport && (
        <Card>
          <div className="flex items-center justify-between" style={{ marginBottom: 12 }}>
            <Mono size={11} upper track="0.18em" color={UI.textMuted}>
              hooks.json
            </Mono>
            <Badge variant="info">{hooks.filter((hook) => hook.enabled).length} enabled</Badge>
          </div>
          <pre
            style={{
              fontFamily: MONO,
              fontSize: 11,
              color: UI.jade,
              background: "rgba(0,0,0,0.35)",
              border: `1px solid ${UI.cardBorder}`,
              borderRadius: 6,
              padding: 12,
              maxHeight: 320,
              overflow: "auto",
              whiteSpace: "pre-wrap",
              wordBreak: "break-word",
            }}
          >
            {generateHooksConfig(hooks)}
          </pre>
          <div style={{ marginTop: 8 }}>
            <Mono size={10} color={UI.textDim}>
              Hook definitions are stored in Ordo settings. Export keeps global hooks and per-mode hooks in separate runtime buckets.
            </Mono>
          </div>
        </Card>
      )}

      <Card>
        <div className="flex items-center justify-between gap-3" style={{ marginBottom: 12 }}>
          <div>
            <Mono size={11} upper track="0.18em" color={UI.textMuted}>
              Debug / event logger
            </Mono>
            <div style={{ marginTop: 4 }}>
              <Mono size={10} color={UI.textDim}>
                Hook Manager emits structured ordo.hooks events for every editor, config, and lifecycle change.
              </Mono>
            </div>
          </div>
          <Button onClick={clearEventLog} size="sm" disabled={events.length === 0}>
            Clear
          </Button>
        </div>
        <div className="space-y-2" style={{ maxHeight: 340, overflow: "auto" }}>
          {events.length === 0 && (
            <div style={{ padding: 14, border: `1px solid ${UI.cardBorder}`, borderRadius: 6 }}>
              <Mono size={11} color={UI.textDim}>
                No hook events yet. Create, edit, toggle, delete, or export a hook to populate the log.
              </Mono>
            </div>
          )}
          {events.map((event) => {
            const levelColor = event.level === "ERROR" ? UI.red : event.level === "WARN" ? UI.amber : UI.jade;
            return (
              <div
                key={event.id}
                style={{
                  display: "grid",
                  gridTemplateColumns: "88px 120px minmax(0, 1fr)",
                  gap: 10,
                  alignItems: "start",
                  padding: "10px 12px",
                  border: `1px solid ${UI.cardBorder}`,
                  borderRadius: 6,
                  background: "rgba(0,0,0,0.18)",
                }}
              >
                <Mono size={10} color={UI.textDim}>
                  {new Date(event.ts).toLocaleTimeString([], { hour: "2-digit", minute: "2-digit", second: "2-digit" })}
                </Mono>
                <Mono size={10} color={levelColor} upper track="0.12em" weight={600}>
                  {event.action}
                </Mono>
                <div style={{ minWidth: 0 }}>
                  <Mono size={11} color={UI.parchment}>
                    {event.message}
                  </Mono>
                  <div className="flex items-center gap-1.5 flex-wrap" style={{ marginTop: 6 }}>
                    {event.hook_name && <Badge variant="neutral">{event.hook_name}</Badge>}
                    {event.scope && (
                      <Badge variant={event.scope === "global" ? "primary" : "success"}>
                        {event.scope === "global" ? "global" : `mode:${event.mode_id ?? "unknown"}`}
                      </Badge>
                    )}
                    {event.event && <Badge variant="info">{event.event}</Badge>}
                    {event.decision && <Badge variant={event.decision === "deny" ? "danger" : event.decision === "allow" ? "success" : "warn"}>{event.decision}</Badge>}
                    {typeof event.enabled === "boolean" && (
                      <Badge variant={event.enabled ? "success" : "neutral"}>{event.enabled ? "enabled" : "disabled"}</Badge>
                    )}
                  </div>
                </div>
              </div>
            );
          })}
        </div>
      </Card>
    </div>
  );
};

const SettingsSurface = ({
  activeTab,
  onOpen,
}: {
  activeTab: string;
  onOpen: (tab: string) => void;
}) => (
  <div className="h-full flex flex-col gap-4 overflow-auto pb-4">
    <SurfaceTitle
      kicker="ordo - settings"
      title="All surfaces"
      sub="The left rail shows the daily controls. Everything else stays reachable here."
    />
    {(["primary", "agent", "knowledge", "advanced", "docs"] as const).map((group) => {
      const groupTabs = TABS.filter((tab) => tab.group === group && tab.id !== "settings");
      if (groupTabs.length === 0) return null;
      return (
        <div key={group} className="space-y-3">
          <Mono size={11} upper track="0.18em" color={UI.textMuted}>{group}</Mono>
          <div className="grid gap-3" style={{ gridTemplateColumns: "repeat(auto-fit, minmax(240px, 1fr))" }}>
            {groupTabs.map((tab) => {
              const Icon = tab.glyph;
              const current = tab.id === activeTab;
              return (
                <button
                  key={tab.id}
                  type="button"
                  onClick={() => onOpen(tab.id)}
                  style={{
                    minHeight: 86,
                    textAlign: "left",
                    borderRadius: 10,
                    border: `1px solid ${current ? UI.primaryBorder : UI.cardBorder}`,
                    background: current ? UI.primarySoft : UI.cardBg,
                    padding: 16,
                    cursor: "pointer",
                  }}
                >
                  <div className="flex items-center gap-3">
                    <Icon size={18} color={current ? UI.primary : UI.textMuted} />
                    <div style={{ fontFamily: FRAUNCES, fontSize: 16, fontWeight: 650, color: UI.parchment }}>
                      {tab.label}
                    </div>
                  </div>
                  <div style={{ marginTop: 8 }}>
                    <Mono size={10} color={UI.textDim}>
                      {LEFT_RAIL_TAB_IDS.has(tab.id) ? "shown on left rail" : "settings-only surface"}
                    </Mono>
                  </div>
                </button>
              );
            })}
          </div>
        </div>
      );
    })}
  </div>
);

const SettingsNavigationBar = ({
  backLabel,
  onBack,
  onRefresh,
}: {
  backLabel: string;
  onBack: () => void;
  onRefresh: () => void;
}) => (
  <div className="flex items-center justify-between gap-3" style={{ flexShrink: 0 }}>
    <Button size="sm" onClick={onBack} title={`Return to ${backLabel}`}>
      <ChevronUp size={13} style={{ transform: "rotate(-90deg)" }} /> Back to {backLabel}
    </Button>
    <Button size="sm" onClick={onRefresh} title="Refresh this settings surface">
      <RefreshCcw size={13} /> Refresh
    </Button>
  </div>
);

export default function OrdoShell() {
  const [tab, setTab] = useState("assistant");
  const [lastNonSettingsTab, setLastNonSettingsTab] = useState("assistant");
  const [settingsRefreshNonce, setSettingsRefreshNonce] = useState(0);
  const [input, setInput] = useState("");
  const [theme, setTheme] = useState<OrdoTheme>(readStoredTheme);
  const [thinkingEffort, setThinkingEffortState] = useState<ThinkingEffort>(
    readStoredThinkingEffort,
  );
  const [ttsEnabled, setTtsEnabledState] = useState(() =>
    readStoredBoolean(TTS_ENABLED_KEY, false),
  );
  const [ttsModel, setTtsModelState] = useState(() =>
    readStoredString(TTS_MODEL_KEY, DEFAULT_TTS_MODEL),
  );
  const [ttsVoice, setTtsVoiceState] = useState(() =>
    readStoredString(TTS_VOICE_KEY, DEFAULT_TTS_VOICE),
  );
  const [ttsFormat, setTtsFormatState] = useState(() =>
    readStoredString(TTS_FORMAT_KEY, DEFAULT_TTS_FORMAT),
  );
  const [ttsBusy, setTtsBusy] = useState(false);
  const [ttsError, setTtsError] = useState<string | null>(null);
  const audioRef = useRef<HTMLAudioElement | null>(null);
  const audioUrlRef = useRef<string | null>(null);
  // The "advanced" group is a file-cabinet — collapsed by default,
  // auto-expanded when one of its tabs is the active surface so the
  // operator never sees an empty rail when they're inside it.
  const [advancedOpen, setAdvancedOpen] = useState(false);
  const [messages, setMessages] = useState<ChatMessage[]>([]);
  const assistantScrollRef = useRef<HTMLDivElement | null>(null);
  const assistantEndRef = useRef<HTMLDivElement | null>(null);
  const assistantPinnedRef = useRef(true);
  const scrollRafRef = useRef<number | null>(null);
  const scrollSettleTimerRef = useRef<number | null>(null);
  // Initial session id is read from localStorage so a reload picks
  // the chat back up. The hydration effect below fetches its turns
  // and replaces `messages` once they arrive.
  const [sessionId, setSessionId] = useState<string | undefined>(
    readStoredSessionId,
  );
  const [sessions, setSessions] = useState<AssistantSessionRecord[]>([]);
  const [sessionsBusy, setSessionsBusy] = useState(false);

  // ─── Mode-scoped workspaces (mode switcher) ───────────────────
  //
  // Two pieces of state:
  //   - `modes`: the registry's manifests, fetched once on mount.
  //     Empty until the fetch resolves; the picker degrades to a
  //     single "General" entry while loading.
  //   - `activeMode`: the operator's currently-selected mode id.
  //     localStorage-persisted. Drives the mode shown in the picker
  //     AND the mode passed when minting new sessions.
  //
  // Architectural rule: changing `activeMode` does NOT rewrite the
  // current session's mode — that field is fixed at creation. Mode
  // change = drop the session id (which makes the mount effect mint
  // a fresh session in the new mode). The previous chat stays in
  // the runtime's DB; it just isn't the studio's foreground anymore.
  const [modes, setModes] = useState<AssistantMode[]>([]);
  const [activeMode, setActiveMode] = useState<string>(readStoredActiveMode);
  // Mode inspector — expandable panel below the picker showing the
  // full manifest. Closed by default; operator opens via INSPECT
  // button. Not persisted — it's a per-session UI affordance, not
  // an operator preference.
  const [inspectorOpen, setInspectorOpen] = useState<boolean>(false);
  const [modeUiSettings, setModeUiSettings] = useState<Record<string, ModeUiSetting>>(
    loadModeUiSettings,
  );
  const [collaboratorRequest, setCollaboratorRequest] = useState("");
  const [workspaceScope, setWorkspaceScopeState] = useState<WorkspaceScope>(
    loadWorkspaceScope,
  );

  // Recent mode-related events captured off the WS stream. Bounded
  // buffer (newest first) — see MODE_EVENT_LOG_CAP and ModeEventLogEntry
  // at module scope.
  const [modeEventLog, setModeEventLog] = useState<ModeEventLogEntry[]>([]);
  const [newChatBusy, setNewChatBusy] = useState(false);
  const [midTaskDraft, setMidTaskDraft] = useState<string | null>(null);
  const [queuedTurns, setQueuedTurns] = useState<QueuedAssistantTurn[]>([]);
  const [interrupting, setInterrupting] = useState(false);
  const queuedTurnsRef = useRef<QueuedAssistantTurn[]>([]);
  const suppressCancelledTurnErrorRef = useRef(false);

  const isNearScrollBottom = (el: HTMLDivElement) =>
    el.scrollHeight - el.scrollTop - el.clientHeight < 48;

  const scrollElementToBottom = (
    el: HTMLDivElement | null,
    end: HTMLDivElement | null,
  ) => {
    if (!el) return;
    el.scrollTop = el.scrollHeight;
    end?.scrollIntoView({ block: "end" });
  };

  const scrollTranscriptsToBottom = (force = false) => {
    if (force || assistantPinnedRef.current) {
      scrollElementToBottom(assistantScrollRef.current, assistantEndRef.current);
    }
  };

  const scheduleTranscriptScroll = (force = false) => {
    if (scrollRafRef.current !== null) {
      window.cancelAnimationFrame(scrollRafRef.current);
    }
    if (scrollSettleTimerRef.current !== null) {
      window.clearTimeout(scrollSettleTimerRef.current);
    }
    scrollRafRef.current = window.requestAnimationFrame(() => {
      scrollRafRef.current = null;
      scrollTranscriptsToBottom(force);
      scrollSettleTimerRef.current = window.setTimeout(() => {
        scrollSettleTimerRef.current = null;
        scrollTranscriptsToBottom(force);
      }, 90);
    });
  };

  const markTranscriptPinnedState = () => {
    const el = assistantScrollRef.current;
    if (!el) return;
    assistantPinnedRef.current = isNearScrollBottom(el);
  };

  useLayoutEffect(() => {
    scheduleTranscriptScroll();
  }, [messages]);

  useEffect(() => {
    const targets = [assistantScrollRef.current].filter(
      (el): el is HTMLDivElement => Boolean(el),
    );
    if (targets.length === 0) return;
    const run = () => scheduleTranscriptScroll();
    const mutationObservers = targets.map((target) => {
      const observer = new MutationObserver(run);
      observer.observe(target, {
        childList: true,
        subtree: true,
        characterData: true,
      });
      return observer;
    });
    const resizeObserver =
      typeof ResizeObserver === "undefined"
        ? null
        : new ResizeObserver(run);
    if (resizeObserver) {
      targets.forEach((target) => {
        resizeObserver.observe(target);
        Array.from(target.children).forEach((child) => resizeObserver.observe(child));
      });
    }
    return () => {
      mutationObservers.forEach((observer) => observer.disconnect());
      resizeObserver?.disconnect();
    };
  }, []);

  useEffect(() => {
    return () => {
      if (scrollRafRef.current !== null) {
        window.cancelAnimationFrame(scrollRafRef.current);
      }
      if (scrollSettleTimerRef.current !== null) {
        window.clearTimeout(scrollSettleTimerRef.current);
      }
    };
  }, []);

  useEffect(() => {
    if (typeof document === "undefined") return;
    document.documentElement.classList.toggle("ordo-theme-bright", theme === "bright");
    document.documentElement.classList.toggle("ordo-theme-dark", theme === "dark");
  }, [theme]);

  const handleThemeChange = (next: OrdoTheme) => {
    setTheme(next);
    persistTheme(next);
    publishUxiDebugEvent("ordo.settings.appearance", "theme_changed", "Theme preference changed.", {
      theme: next,
    });
  };

  const setWorkspaceScope = (next: WorkspaceScope) => {
    const normalized = normalizeWorkspaceScope(next);
    setWorkspaceScopeState(normalized);
    saveWorkspaceScope(normalized);
    setSessionId(undefined);
    publishUxiDebugEvent("ordo.projects", "workspace_scope_changed", "Workspace scope changed.", {
      workspace: workspaceScopeToMetadata(normalized),
    });
  };

  const updateModeUiSettings = (
    modeId: string,
    patch: Partial<ModeUiSetting>,
  ) => {
    setModeUiSettings((prev) => {
      const existing = prev[modeId] ?? {
        enabled: modeDefaultsEnabled(modeId),
        ragLimitMb: modeId === "dreaming" ? 2048 : modeId === "diagnostic" ? 1024 : 512,
      };
      const next = { ...prev, [modeId]: { ...existing, ...patch } };
      saveModeUiSettings(next);
      return next;
    });
  };

  // Mode-list fetch, once on mount. A failure leaves `modes` empty,
  // and the picker falls back to a single "General" entry (the
  // FALLBACK_MODE_ID). The runtime's mode-aware paths still work in
  // that case — they resolve "general" as their default.
  useEffect(() => {
    let cancelled = false;
    void listAssistantModes()
      .then((res) => {
        if (cancelled) return;
        const nextModes = Array.isArray(res.modes) ? res.modes : [];
        setModes(nextModes);
        // If the persisted active mode was edited away (operator
        // deleted the manifest from disk between launches), fall
        // back to General quietly.
        if (
          nextModes.length > 0 &&
          !nextModes.some((m) => m.id === activeMode)
        ) {
          setActiveMode(FALLBACK_MODE_ID);
        }
        if (
          nextModes.length > 0 &&
          nextModes.some((m) => m.id === activeMode) &&
          !modeUiSetting(modeUiSettings, nextModes.find((m) => m.id === activeMode)!).enabled
        ) {
          setActiveMode(FALLBACK_MODE_ID);
        }
      })
      .catch((err) => {
        console.warn("[Modes] could not list:", err);
      });
    return () => {
      cancelled = true;
    };
    // mount-only; activeMode is read once via the snapshot above
    // and the registry-mismatch fix is part of that one-shot.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  // Persist activeMode to localStorage on every change.
  useEffect(() => {
    if (typeof window === "undefined") return;
    try {
      window.localStorage.setItem(ACTIVE_MODE_KEY, activeMode);
    } catch {
      // private/incognito mode can throw; non-fatal.
    }
  }, [activeMode]);

  const refreshSessions = async () => {
    setSessionsBusy(true);
    try {
      const res = await listAssistantSessions(80);
      setSessions(Array.isArray(res.sessions) ? res.sessions : []);
    } catch (err) {
      console.warn("[Chat] could not list sessions:", err);
    } finally {
      setSessionsBusy(false);
    }
  };

  useEffect(() => {
    void refreshSessions();
  }, []);

  // Operator changed mode in the picker. Architecture says: do NOT
  // rewrite the current session's mode. Instead, drop the session
  // id — the mount-time effect re-mints a fresh session in the
  // newly-active mode. Existing chat history stays in the DB
  // (recoverable via the future session-list UI).
  const handleModeChange = (next: string) => {
    if (next === activeMode) return;
    setActiveMode(next);
    setMessages([]);
    setSessionId(undefined);
    // Reset hydration guard so a future stored-session reload still
    // works as expected if the operator manually restores an id.
    sessionHydratedRef.current = true;
  };

  const handleNewChat = async () => {
    if (newChatBusy) return;
    setNewChatBusy(true);
    try {
      streamRef.current?.close();
      streamRef.current = null;
      const session = await newAssistantSession(undefined, activeMode);
      setSessionId(session.id);
      setSessions((prev) => [session, ...prev.filter((s) => s.id !== session.id)]);
      setSessionTaint(null);
      setModeEventLog([]);
      clearPending();
      setMessages([]);
      void refreshSessions();
    } catch (err: unknown) {
      setMessages([
        {
          role: "assistant",
          text: `new chat failed - ${err instanceof Error ? err.message : String(err)}`,
          ts: tsNow(),
          meta: ["error"],
        },
      ]);
    } finally {
      setNewChatBusy(false);
    }
  };

  // Persist sessionId to localStorage whenever it changes. Setting
  // to undefined wipes the stored id (used on cancel / new chat /
  // stale-session recovery).
  useEffect(() => {
    if (typeof window === "undefined") return;
    try {
      if (sessionId) {
        window.localStorage.setItem(SESSION_ID_KEY, sessionId);
      } else {
        window.localStorage.removeItem(SESSION_ID_KEY);
      }
    } catch {
      // localStorage can throw under private/incognito; non-fatal.
    }
  }, [sessionId]);

  // One-shot mount-time hydration of prior turns. We guard with a
  // ref so React strict-mode double-invokes don't double-hydrate.
  // On 404 / network error the session id is dropped and the chat
  // starts fresh — the runtime might have pruned the row, or the
  // operator may have moved between machines.
  const sessionHydratedRef = useRef(false);
  useEffect(() => {
    if (sessionHydratedRef.current) return;
    if (!sessionId) {
      sessionHydratedRef.current = true;
      return;
    }
    sessionHydratedRef.current = true;
    let cancelled = false;
    void (async () => {
      try {
        const detail = await fetchAssistantSession(sessionId);
        if (cancelled) return;
        const restored = turnsToChatMessages(detail.turns);
        setSessions((prev) => [
          detail.session,
          ...prev.filter((session) => session.id !== detail.session.id),
        ]);
        if (restored.length === 0) return;
        setMessages(restored);
      } catch (err) {
        // Stale or pruned session — drop the id and start fresh.
        // The setSessionId call also wipes localStorage via the
        // persistence effect above.
        if (!cancelled) {
          console.warn("[Chat] could not restore session:", err);
          setSessionId(undefined);
        }
      }
    })();
    return () => {
      cancelled = true;
    };
  }, [sessionId]);
  const [sending, setSending] = useState(false);

  // Conversation taint state (Phase B). Polled every 5s when a
  // session is alive — cheap call, only flips when an
  // <untrusted_web_content> block has entered the conversation. The
  // AssistantSurface reads this and shows the lamp-hot indicator
  // with the URLs in a tooltip when tainted.
  const [sessionTaint, setSessionTaint] = useState<SessionTaintState | null>(null);
  useEffect(() => {
    if (!sessionId) {
      setSessionTaint(null);
      return;
    }
    let cancelled = false;
    const tick = async () => {
      try {
        const taint = await fetchSessionTaint(sessionId);
        if (!cancelled) setSessionTaint(taint);
      } catch {
        // Silent — runtime restart between turns yields a 400 here;
        // the chat's auto-recovery on stale-session creates a fresh
        // session and the next tick picks it up.
      }
    };
    void tick();
    const interval = setInterval(tick, 5000);
    return () => {
      cancelled = true;
      clearInterval(interval);
    };
  }, [sessionId]);
  const clearTaintHandler = async () => {
    if (!sessionId) return;
    try {
      await clearSessionTaint(sessionId);
      // Optimistic local clear so the indicator hides immediately.
      setSessionTaint({
        session_id: sessionId,
        tainted: false,
        sources: [],
      });
    } catch (err) {
      console.error("[Taint] clear failed:", err);
    }
  };

  // Pending attachments for the next chat turn. Three kinds:
  //
  //   - pendingImages: read inline as base64 and shipped via
  //     TurnRequest.attachments → the LLM provider's vision channel
  //     (OpenAI image_url, Anthropic image block). Translator in
  //     ordo-cloud handles per-provider shape.
  //   - pendingFiles:  uploaded to user-files first (so the agent can
  //     filesystem.read_file them) and referenced in the user message
  //     prefix + TurnRequest.metadata.uploaded_files for audit.
  //
  // Folder uploads are just bulk file uploads with the relative path
  // preserved as part of original_name (e.g. "report/q3/notes.md").
  // The agent's environment map already says user-files/ is the
  // sandboxed read area, so the planner naturally routes there.
  interface PendingImage {
    id: string;        // local handle for remove
    name: string;
    size: number;
    data: string;      // base64 (no data: prefix)
    mediaType: string; // image/png, image/jpeg, …
  }
  interface PendingFile {
    id: string;        // local handle for remove
    name: string;      // original_name including any folder-relative path
    size: number;
    file_id?: string;  // server-assigned id once upload completes
    sha256?: string;
    uploading: boolean;
    error?: string;
  }
  const [pendingImages, setPendingImages] = useState<PendingImage[]>([]);
  const [pendingFiles, setPendingFiles] = useState<PendingFile[]>([]);
  const imageInputRef = useRef<HTMLInputElement | null>(null);
  const fileInputRef = useRef<HTMLInputElement | null>(null);
  const folderInputRef = useRef<HTMLInputElement | null>(null);

  const localId = (): string =>
    typeof crypto !== "undefined" && "randomUUID" in crypto
      ? crypto.randomUUID()
      : `${Date.now()}-${Math.random().toString(16).slice(2)}`;

  // Upload a single browser File via fileToBase64 → uploadFileBase64.
  // Returns the FileRow (or throws). The "name" sent up includes any
  // webkitRelativePath the browser provided so a folder upload keeps
  // its hierarchy in user-files/.
  const uploadOne = async (
    f: File,
  ): Promise<{ file_id: string; sha256: string }> => {
    const data_base64 = await fileToBase64(f);
    const relName =
      // webkitRelativePath is "folder/sub/file.md" when the input had
      // webkitdirectory; empty for single-file picks. Fall back to
      // the bare name in the latter case.
      (f as File & { webkitRelativePath?: string }).webkitRelativePath ||
      f.name;
    const row = await uploadFileBase64({
      original_name: relName,
      data_base64,
      content_type: f.type || "application/octet-stream",
      created_by: "operator",
    });
    return { file_id: row.id, sha256: row.sha256_hex };
  };

  // Image picker handler — read inline, no upload. Auto-expands the
  // chat so the operator sees the chip strip.
  const onPickImages = async (e: React.ChangeEvent<HTMLInputElement>) => {
    const list = Array.from(e.target.files ?? []);
    e.target.value = "";
    if (list.length === 0) return;
    for (const f of list) {
      try {
        const data = await fileToBase64(f);
        setPendingImages((prev) => [
          ...prev,
          {
            id: localId(),
            name: f.name,
            size: f.size,
            data,
            mediaType: f.type || "image/png",
          },
        ]);
      } catch (err) {
        console.error("[Chat] image read failed:", err);
      }
    }
  };

  // File picker handler — uploads each to user-files. Each row
  // appears in the chip strip immediately with `uploading: true`,
  // updated to `uploading: false` (with file_id) when done.
  const onPickFiles = async (e: React.ChangeEvent<HTMLInputElement>) => {
    const list = Array.from(e.target.files ?? []);
    e.target.value = "";
    if (list.length === 0) return;
    for (const f of list) {
      const id = localId();
      const relName =
        (f as File & { webkitRelativePath?: string }).webkitRelativePath ||
        f.name;
      setPendingFiles((prev) => [
        ...prev,
        { id, name: relName, size: f.size, uploading: true },
      ]);
      try {
        const { file_id, sha256 } = await uploadOne(f);
        setPendingFiles((prev) =>
          prev.map((p) =>
            p.id === id ? { ...p, uploading: false, file_id, sha256 } : p,
          ),
        );
      } catch (err) {
        const msg = err instanceof Error ? err.message : String(err);
        console.error("[Chat] file upload failed:", err);
        setPendingFiles((prev) =>
          prev.map((p) =>
            p.id === id ? { ...p, uploading: false, error: msg } : p,
          ),
        );
      }
    }
  };

  const removePendingImage = (id: string) =>
    setPendingImages((prev) => prev.filter((p) => p.id !== id));
  const removePendingFile = (id: string) =>
    setPendingFiles((prev) => prev.filter((p) => p.id !== id));
  const clearPending = () => {
    setPendingImages([]);
    setPendingFiles([]);
  };
  const anyUploadInFlight = pendingFiles.some((p) => p.uploading);
  // WS handle for the live token stream. Opened lazily once we have a
  // session id; closed on shell unmount. Each turn reuses it.
  const streamRef = useRef<TurnStreamHandle | null>(null);

  const handleSessionChange = async (nextSessionId: string) => {
    if (nextSessionId === "__new__") {
      await handleNewChat();
      return;
    }
    if (!nextSessionId || nextSessionId === sessionId || sending) return;
    setSessionsBusy(true);
    try {
      streamRef.current?.close();
      streamRef.current = null;
      const detail = await fetchAssistantSession(nextSessionId);
      const nextMode = detail.session.mode || FALLBACK_MODE_ID;
      setActiveMode(nextMode);
      setSessionId(detail.session.id);
      setSessionTaint(null);
      setModeEventLog([]);
      clearPending();
      setMessages(turnsToChatMessages(detail.turns));
      setSessions((prev) => [
        detail.session,
        ...prev.filter((session) => session.id !== detail.session.id),
      ]);
      sessionHydratedRef.current = true;
      publishUxiDebugEvent("ordo.assistant", "session_selected", "Assistant session selected.", {
        session_id: detail.session.id,
        mode: nextMode,
        turn_count: detail.session.turn_count ?? detail.turns.length,
      });
      scheduleTranscriptScroll(true);
      void refreshSessions();
    } catch (err: unknown) {
      const reason = err instanceof Error ? err.message : String(err);
      setMessages((prev) => [
        ...prev,
        {
          role: "assistant",
          text: `session load failed - ${reason}`,
          ts: tsNow(),
          meta: ["error"],
        },
      ]);
    } finally {
      setSessionsBusy(false);
    }
  };

  // ─── Voice-to-text (Web Speech API) ─────────────────────────
  //
  // Uses the browser-native SpeechRecognition API the WebView (Edge
  // WebView2 on Windows, WebKit on macOS) already has. No model
  // download, no extra deps. Feature-detects both prefixed and
  // unprefixed names. Click toggles recording; while recording, the
  // mic button pulses red and the recognition's results stream into
  // the existing chat input.
  //
  // Privacy note worth knowing: WebView2 routes recognition through
  // Microsoft's online speech service. That's fine for "I want it
  // for me right now" but if a fully-local pipeline is needed later,
  // swap in whisper.cpp via an MCP server (audio capture + transcribe
  // + return text on a tool call).
  //
  // We capture the input value at start-time as a "baseline" so the
  // operator's pre-typed text stays intact and gets concatenated
  // with whatever was spoken — they can type, dictate the rest,
  // edit, then send.
  const [isListening, setIsListening] = useState(false);
  const [voiceUnsupported, setVoiceUnsupported] = useState(false);
  const [voiceError, setVoiceError] = useState<string | null>(null);
  const recognitionRef = useRef<unknown | null>(null);
  const voiceBaselineRef = useRef<string>("");

  useEffect(() => {
    // Feature-detect once on mount. If the API isn't there, the mic
    // button stays visible but disabled with an explanatory tooltip.
    if (typeof window === "undefined") return;
    const SR =
      (window as unknown as { SpeechRecognition?: unknown }).SpeechRecognition ||
      (window as unknown as { webkitSpeechRecognition?: unknown })
        .webkitSpeechRecognition;
    if (!SR) setVoiceUnsupported(true);
  }, []);

  const stopListening = () => {
    const rec = recognitionRef.current as { stop?: () => void } | null;
    try {
      rec?.stop?.();
    } catch {
      // Some implementations throw if stop() runs before any audio
      // arrived. Safe to swallow — onend will fire either way.
    }
  };

  const startListening = () => {
    if (typeof window === "undefined") return;
    setVoiceError(null);
    const SR = (
      (window as unknown as { SpeechRecognition?: unknown }).SpeechRecognition ||
      (window as unknown as { webkitSpeechRecognition?: unknown })
        .webkitSpeechRecognition
    ) as
      | (new () => {
          continuous: boolean;
          interimResults: boolean;
          lang: string;
          start: () => void;
          stop: () => void;
          onresult: ((event: unknown) => void) | null;
          onerror: ((event: { error?: string }) => void) | null;
          onend: (() => void) | null;
        })
      | undefined;
    if (!SR) {
      setVoiceUnsupported(true);
      return;
    }
    const recognition = new SR();
    recognition.continuous = true;
    recognition.interimResults = true;
    recognition.lang =
      (typeof navigator !== "undefined" && navigator.language) || "en-US";
    voiceBaselineRef.current = input.length > 0 ? `${input} ` : "";
    recognition.onresult = (event: unknown) => {
      // SpeechRecognitionEvent shape: results is a list of
      // SpeechRecognitionResult, each a list of alternatives.
      // We walk from event.resultIndex (the first one new this fire)
      // and build a single transcript: final results commit, interim
      // results trail behind so the operator sees the words land in
      // real time.
      const e = event as {
        resultIndex: number;
        results: ArrayLike<{
          isFinal: boolean;
          0: { transcript: string };
        }> & { length: number };
      };
      let finalText = "";
      let interimText = "";
      for (let i = 0; i < e.results.length; i += 1) {
        const result = e.results[i];
        const transcript = result[0]?.transcript ?? "";
        if (result.isFinal) finalText += transcript;
        else interimText += transcript;
      }
      const composed = `${voiceBaselineRef.current}${finalText}${interimText}`.trimStart();
      setInput(composed);
    };
    recognition.onerror = (event: { error?: string }) => {
      const code = event?.error ?? "unknown";
      // Common ones: "not-allowed" (permission denied), "no-speech"
      // (silence timeout), "audio-capture" (no mic), "network"
      // (online recognition couldn't reach the speech service).
      if (code === "no-speech") {
        // Silence timeout is normal; just stop quietly.
        setIsListening(false);
        return;
      }
      const friendly =
        code === "not-allowed" || code === "service-not-allowed"
          ? "microphone permission denied — grant access in OS settings"
          : code === "audio-capture"
          ? "no microphone detected"
          : code === "network"
          ? "speech service unreachable (offline?)"
          : `voice input failed: ${code}`;
      setVoiceError(friendly);
      setIsListening(false);
    };
    recognition.onend = () => {
      setIsListening(false);
      recognitionRef.current = null;
    };
    try {
      recognition.start();
      recognitionRef.current = recognition;
      setIsListening(true);
    } catch (err) {
      setVoiceError(err instanceof Error ? err.message : String(err));
    }
  };

  const toggleListening = () => {
    if (isListening) stopListening();
    else startListening();
  };

  // Make sure recognition stops if the shell unmounts while
  // listening (rare but cheap to guard).
  useEffect(() => {
    return () => {
      stopListening();
    };
  }, []);
  // Pre-create a session on mount so the very first turn streams. If
  // we waited until send() to call assistant.new_session, the
  // WebSocket subscription would race the LLM's first TokenDelta and
  // the studio would only see the final reply.
  useEffect(() => {
    let cancelled = false;
    // Skip if we already have a session id from localStorage —
    // hydration is in flight (or done). Minting a new one here would
    // race with the restore and discard the prior conversation.
    if (sessionId) return;
    void newAssistantSession(undefined, activeMode)
      .then((s) => {
        if (cancelled) return;
        setSessionId(s.id);
        setSessions((prev) => [s, ...prev.filter((session) => session.id !== s.id)]);
        void refreshSessions();
      })
      .catch(() => {
        // Non-fatal. send() will retry creating one when the user types.
      });
    return () => {
      cancelled = true;
    };
    // We intentionally do not list sessionId in the dep array — this
    // is mount-only behavior. Subsequent setSessionId(undefined)
    // (e.g. on stale-session) is handled by the send() retry path,
    // which mints a new session inside attempt(true).
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  // Make sure we hold a live WS subscription whenever a session_id is
  // known. The runtime emits TokenDelta events while the LLM is
  // streaming so the chat dock can paint tokens as they arrive instead
  // of waiting for the full reply.
  useEffect(() => {
    if (!sessionId) return;
    const handle = openAssistantStream(sessionId, (e) => {
      if (e.event === "token_delta" && typeof (e as { delta?: unknown }).delta === "string") {
        const delta = (e as { delta: string }).delta;
        setMessages((prev) => {
          // Append to the last assistant message that's still streaming.
          if (prev.length === 0) return prev;
          const last = prev[prev.length - 1];
          if (last.role !== "assistant" || !last.streaming) return prev;
          return [...prev.slice(0, -1), { ...last, text: last.text + delta }];
        });
      } else if (e.event === "tool_call_started") {
        const cap = (e as { capability?: string }).capability ?? "";
        setMessages((prev) => {
          if (prev.length === 0) return prev;
          const last = prev[prev.length - 1];
          if (last.role !== "assistant" || !last.streaming) return prev;
          const tools = new Set(last.meta ?? []);
          tools.add(cap);
          return [...prev.slice(0, -1), { ...last, meta: Array.from(tools) }];
        });
      } else if (
        e.event === "mode_bound" ||
        e.event === "mode_memory_scope_applied" ||
        e.event === "mode_tool_filter_applied" ||
        e.event === "cross_mode_consult_requested" ||
        e.event === "cross_mode_consult_approved" ||
        e.event === "cross_mode_consult_denied" ||
        e.event === "cross_mode_consult_completed"
      ) {
        // Mode-related events flow into the inspector panel's
        // "recent activity" log. Bounded buffer (newest first).
        const summary = renderModeEventSummary(e);
        const entry: ModeEventLogEntry = {
          kind: e.event,
          ts: tsNow(),
          summary,
          raw: e,
        };
        setModeEventLog((prev) =>
          [entry, ...prev].slice(0, MODE_EVENT_LOG_CAP),
        );
      }
    });
    streamRef.current = handle;
    return () => {
      handle.close();
      streamRef.current = null;
    };
  }, [sessionId]);

  // Provider-neutral LLM signal: poll the credential list, find the row
  // whose service matches the operator's chosen default (localStorage),
  // and reflect it as a top-bar signal. No provider name baked in — if
  // there's no default, the signal goes off and labels itself "no LLM."
  const [llmSignal, setLlmSignal] = useState<SignalDef>({
    id: "llm",
    label: "llm",
    state: "off",
    detail: "no provider configured",
  });
  const [contextBudget, setContextBudget] = useState<ContextBudgetSignal>({
    tokens: DEFAULT_CONTEXT_WINDOW_TOKENS,
    providerLabel: "default",
    model: null,
    configured: false,
  });
  const [modelChoice, setModelChoice] = useState<ModelChoiceSignal>({
    service: null,
    providerLabel: "default",
    selected: "",
    options: [],
    extras: {},
    baseUrl: null,
    authStyle: null,
  });
  const [modelSaving, setModelSaving] = useState(false);
  useEffect(() => {
    let cancelled = false;
    const tick = async () => {
      try {
        const res = await listCloudCredentials();
        if (cancelled) return;
        const defaultId =
          (typeof window !== "undefined" &&
            window.localStorage.getItem("ordo:default_provider")) ||
          null;
        const cred =
          (defaultId && res.credentials.find((c) => c.service === defaultId)) ||
          res.credentials[0] ||
          null;
        if (!cred) {
          setLlmSignal({
            id: "llm",
            label: "llm",
            state: "off",
            detail: "no provider configured",
          });
          setContextBudget({
            tokens: DEFAULT_CONTEXT_WINDOW_TOKENS,
            providerLabel: "default",
            model: null,
            configured: false,
          });
          setModelChoice({
            service: null,
            providerLabel: "default",
            selected: "",
            options: [],
            extras: {},
            baseUrl: null,
            authStyle: null,
          });
        } else {
          const extras = (cred.extras ?? {}) as Record<string, string>;
          // Prefer the credential's `label` (always operator-visible)
          // over `extras.name` (which can be redacted on legacy
          // credentials saved before the redaction allowlist landed).
          const isRedacted = (s: string | null | undefined) =>
            !s || s === "***" || s.trim() === "";
          const labelRaw = !isRedacted(cred.label)
            ? cred.label!
            : !isRedacted(extras.name)
            ? extras.name
            : cred.service;
          const model = !isRedacted(extras.model) ? extras.model : null;
          const contextWindow =
            parseContextWindowTokens(extras.context_window) ??
            DEFAULT_CONTEXT_WINDOW_TOKENS;
          // Friendly short label: trim "(local)" suffix, lowercase for
          // the rail dot. Detail line is set after live model discovery
          // so stale saved cloud defaults do not win over discovered models.
          const friendly = labelRaw
            .replace(/\s*\(local\)\s*$/i, "")
            .toLowerCase();
          const template = findTemplate(cred.service);
          const discoveredProvider = localDiscoveryProvider(cred);
          const providerKind = (extras.provider_kind ?? "").toLowerCase();
          let discoveredModels: string[] = [];
          if (discoveredProvider) {
            try {
              const discovered = await detectLocalLlm(discoveredProvider);
              const rawDiscoveredModels = discovered.reachable ? discovered.models : [];
              discoveredModels =
                discoveredProvider === "ollama"
                  ? providerKind === "ollama_cloud_via_local_ollama"
                    ? cloudOllamaModels(rawDiscoveredModels)
                    : localOllamaModels(rawDiscoveredModels)
                  : rawDiscoveredModels;
            } catch {
              discoveredModels = [];
            }
            if (cancelled) return;
          }
          const savedModel =
            model ??
            liveOrFallback(cred.model, "") ??
            template?.default_model ??
            "";
          const discoveredCloudModels = providerKind === "ollama_cloud_via_local_ollama"
            ? cloudOllamaModels(discoveredModels)
            : [];
          const selectedModel = discoveredCloudModels.length > 0
            ? pickOllamaCloudModel(discoveredCloudModels, savedModel)
            : savedModel;
          const options = uniqueModels([
            selectedModel,
            ...splitModelOptions(extras.model_options),
            ...splitModelOptions(extras.available_models),
            ...(discoveredCloudModels.length > 0 ? discoveredCloudModels : discoveredModels),
            template?.default_model,
          ]);
          setLlmSignal({
            id: "llm",
            label: friendly,
            state: "ok",
            detail: selectedModel
              ? `${selectedModel} - ${cred.base_url ?? ""}`
              : cred.base_url ?? "configured (no model set)",
          });
          setContextBudget({
            tokens: contextWindow,
            providerLabel: labelRaw,
            model: selectedModel || null,
            configured: true,
          });
          if (
            providerKind === "ollama_cloud_via_local_ollama" &&
            selectedModel &&
            selectedModel !== (model ?? "")
          ) {
            void upsertCloudCredential({
              service: cred.service,
              extras: {
                ...extras,
                model: selectedModel,
              },
            });
          }
          setModelChoice({
            service: cred.service,
            providerLabel: labelRaw,
            selected: selectedModel,
            options,
            extras,
            baseUrl: cred.base_url ?? cred.endpoint ?? null,
            authStyle: cred.auth_style,
          });
        }
      } catch {
        if (cancelled) return;
        setLlmSignal({
          id: "llm",
          label: "llm",
          state: "warn",
          detail: "credential list unreachable",
        });
          setContextBudget({
            tokens: DEFAULT_CONTEXT_WINDOW_TOKENS,
            providerLabel: "default",
            model: null,
            configured: false,
          });
          setModelChoice({
            service: null,
            providerLabel: "default",
            selected: "",
            options: [],
            extras: {},
            baseUrl: null,
            authStyle: null,
          });
      }
    };
    void tick();
    const interval = setInterval(tick, 8000);
    return () => {
      cancelled = true;
      clearInterval(interval);
    };
  }, []);

  // Composed signal set rendered in the top bar. The LLM signal lives
  // at the trailing end of the rail because provider labels can be
  // long ("openrouter · openai/gpt-4o-mini" etc.) — putting it last
  // lets the detail line extend to the right edge without shoving the
  // fixed-width signals (gateway, bus, vault, mcp, embed, heal) around.
  const allSignals: SignalDef[] = useMemo(() => {
    return [...STATIC_SIGNAL_DEFS, llmSignal];
  }, [llmSignal]);

  // Rescue Mode trigger — when the gateway signal flips to err, the
  // whole shell tints amber. Reads from the live signal set.
  const gatewaySignal = useMemo(
    () => allSignals.find((s) => s.id === "gateway"),
    [allSignals],
  );
  const rescueActive = gatewaySignal?.state === "err";

  // Shell-level poll for pending review requests. Drives the pulse on
  // the Review tab — the operator should see the urgency from any
  // surface, not only when they're already on Review. The Review
  // surface itself does its own poll for the list payload; this one
  // only needs the count, so it's a thin call.
  const [pendingReviewCount, setPendingReviewCount] = useState(0);
  useEffect(() => {
    let cancelled = false;
    const tick = async () => {
      try {
        const res = await listReviewPending();
        if (!cancelled) setPendingReviewCount(res.count);
      } catch {
        // Silent — this is a background poll. A dropped poll is
        // self-healing on the next tick; surfacing every transient
        // glitch in the rail would be noisy.
      }
    };
    void tick();
    const interval = setInterval(tick, 5000);
    return () => {
      cancelled = true;
      clearInterval(interval);
    };
  }, []);

  // Pull a readable reason out of whatever the runtime returned. The
  // runtime's 4xx responses are JSON like `{"error":"session '…' not
  // found"}` — the bare ApiError.message ("POST … → 400") drops the
  // useful part. This helper surfaces the actual reason so the chat
  // bubble explains what's wrong.
  const reasonFromError = (err: unknown): string => {
    if (err instanceof ApiError) {
      const body = err.body;
      if (body && typeof body === "object" && "error" in body) {
        const inner = (body as { error?: unknown }).error;
        if (typeof inner === "string" && inner.length > 0) return inner;
      }
      if (typeof body === "string" && body.length > 0) return body;
      return err.message;
    }
    if (err instanceof Error) return err.message;
    return String(err);
  };

  // Detect "the session id we cached doesn't exist on the server"
  // (typical after a runtime restart). The runtime serializes this as
  // `session '<uuid>' not found` from map_assistant_error.
  const isSessionNotFound = (err: unknown): boolean => {
    if (!(err instanceof ApiError)) return false;
    const reason = reasonFromError(err);
    return /session '[^']+' not found/i.test(reason);
  };

  const nextQueuedTurnId = () =>
    typeof crypto !== "undefined" && typeof crypto.randomUUID === "function"
      ? crypto.randomUUID()
      : `${Date.now()}-${Math.random().toString(16).slice(2)}`;

  const setQueuedAssistantTurns = (next: QueuedAssistantTurn[]) => {
    queuedTurnsRef.current = next;
    setQueuedTurns(next);
  };

  const queueAssistantTurn = (
    turn: QueuedAssistantTurn,
    placement: "front" | "back" = "back",
  ) => {
    const next =
      placement === "front"
        ? [turn, ...queuedTurnsRef.current]
        : [...queuedTurnsRef.current, turn];
    setQueuedAssistantTurns(next);
  };

  const shiftQueuedAssistantTurn = (): QueuedAssistantTurn | null => {
    const [next, ...rest] = queuedTurnsRef.current;
    setQueuedAssistantTurns(rest);
    return next ?? null;
  };

  const send = async (override?: { text: string; meta?: Record<string, unknown> }) => {
    // The operator can send with text alone, attachments alone, or
    // both. Block sends while file uploads are still in flight —
    // otherwise the agent would see the file list in the prefix
    // before the bytes are actually queryable.
    const isOverride = override !== undefined;
    const txt = (override?.text ?? input).trim();
    const hasText = txt.length > 0;
    const hasAttachments = !isOverride && (pendingImages.length > 0 || pendingFiles.length > 0);
    if (sending && !isOverride) {
      if (hasText) {
        setMidTaskDraft(txt);
        publishUxiDebugEvent("ordo.assistant", "mid_task_message_detected", "Operator message entered while the agent is working.", {
          queued_count: queuedTurnsRef.current.length,
        });
      }
      return;
    }
    if ((!hasText && !hasAttachments) || anyUploadInFlight) return;
    // Snapshot attachments at send-time. The state could change
    // mid-send (operator picks more files); we want the turn to
    // ship exactly what was visible when they hit Send.
    const sendImages = isOverride ? [] : pendingImages;
    const sendFiles = isOverride
      ? []
      : pendingFiles.filter((f) => f.file_id && !f.uploading && !f.error);
    if (!isOverride) {
      setInput("");
      clearPending();
    }
    setSending(true);
    // Build the visible message text shown in the chat bubble. When
    // there are attachments, prefix a short "[attached: …]" list so
    // the operator (and the assistant) sees what came along. Empty
    // text + attachments still produces a valid message.
    const attachmentLines: string[] = [];
    if (sendImages.length > 0) {
      attachmentLines.push(
        `image: ${sendImages.map((i) => i.name).join(", ")}`,
      );
    }
    if (sendFiles.length > 0) {
      attachmentLines.push(
        `file: ${sendFiles.map((f) => f.name).join(", ")}`,
      );
    }
    const displayText =
      attachmentLines.length > 0
        ? `${attachmentLines.map((l) => `[${l}]`).join(" ")}${txt ? `\n\n${txt}` : ""}`
        : txt;
    // The user_message we ship to the runtime: prefix non-image
    // attachments with their user-files paths so the assistant can
    // route to filesystem.read_file naturally. Images don't need a
    // prefix — the LLM provider sees them directly via the
    // attachments channel.
    const filePrefix =
      sendFiles.length > 0
        ? `[I attached ${sendFiles.length} file${sendFiles.length === 1 ? "" : "s"} to user-files/: ${sendFiles
            .map((f) => f.name)
            .join(", ")}. Read them via filesystem.read_file when relevant.]\n\n`
        : "";
    const wireText = `${filePrefix}${txt}`.trim() || "(no text — attachments only)";
    // Append the user message + an empty placeholder assistant message
    // that token deltas will fill in. The placeholder's `streaming`
    // flag tells the WS effect to append rather than replace.
    assistantPinnedRef.current = true;
    setMessages((m) => [
      ...m,
      { role: "user", text: displayText, ts: tsNow() },
      { role: "assistant", text: "", ts: tsNow(), streaming: true },
    ]);
    scheduleTranscriptScroll(true);
    // Inner attempt — factored so we can retry once with a fresh
    // session id when the runtime says the cached one is gone (e.g.
    // after a runtime restart between turns). `forceNew` bypasses
    // the closure-captured sessionId on retry; the React state
    // update is async so we can't rely on the closure refreshing.
    const attempt = async (forceNew: boolean): Promise<void> => {
      let sid = forceNew ? undefined : sessionId;
      if (!sid) {
        const session = await newAssistantSession(undefined, activeMode);
        sid = session.id;
        setSessionId(sid);
        // Give React + the WS effect a tick to spin up the subscription
        // before the turn starts emitting TokenDeltas.
        await new Promise((r) => setTimeout(r, 50));
      }
      // Pull operator-side skill toggles out of localStorage on every
      // turn so changes from the Skills tab take effect immediately
      // without a state plumbing pass. The runtime currently echoes
      // metadata into audit and ignores unknown keys, so this is
      // forward-compatible: when skill-toggling lands on the runtime
      // side, the planner already has the data.
      const disabledSkills = Array.from(loadPausedSkills());
      const customSkills = loadCustomSkills();
      const turnMeta: Record<string, unknown> = {};
      // Always send the strictness preset — it controls the
      // untrusted-content rule the runtime appends to the bootstrap
      // prompt. Default ("medium") still ships explicitly so the
      // runtime sees a deliberate value rather than falling through
      // to its own default.
      turnMeta.untrusted_strictness = loadStrictnessPreset();
      turnMeta.thinking_effort = thinkingEffort;
      turnMeta.reasoning_effort =
        thinkingEffort === "off" ? "none" : thinkingEffort;
      if (modelChoice.selected) {
        turnMeta.requested_model = modelChoice.selected;
      }
      turnMeta.context_estimate = {
        used_tokens: contextUsedTokens,
        context_window_tokens: contextBudget.tokens,
        provider_configured: contextBudget.configured,
        compaction: "auto_mechanical",
      };
      turnMeta.workspace_scope = workspaceScopeToMetadata(workspaceScope);
      const activeModeManifest =
        modes.find((mode) => mode.id === activeMode) ?? {
          id: activeMode,
          label: activeMode,
          description: "",
          memory_scope: [],
          rag_domains: [],
          allowed_tool_lanes: [],
          blocked_tool_capabilities: [],
          policies: [],
          planner_bias: [],
          persona: [],
        };
      const activeModeUi = modeUiSetting(modeUiSettings, activeModeManifest);
      const activeOptionalRags = enabledOptionalRags(activeModeUi);
      const collaboration = modeCollaborationSetting(activeModeUi);
      turnMeta.rag_storage_budget_mb = activeModeUi.ragLimitMb;
      if (activeMode === "diagnostic") {
        turnMeta.diagnostic = {
          allow_cloud_models: activeModeUi.allowCloudModels === true,
          cloud_model_policy: activeModeUi.allowCloudModels === true ? "allow" : "deny",
        };
      }
      if (isTemporarySpecialistMode(activeMode)) {
        turnMeta.temporary_mode = {
          auto_disable_after_turn: true,
          permission_gated_os_access: true,
        };
      }
      if (activeOptionalRags.length > 0) {
        turnMeta.optional_rag_domains = activeOptionalRags.map((rag) => ({
          id: rag.id,
          label: rag.label,
          groups: rag.groups,
          storage_limit_mb: rag.storageLimitMb,
        }));
      }
      turnMeta.cross_mode_collaboration = {
        policy: collaboration.policy,
        mechanism: "consult_mode_agent",
        isolation: "no_cross_rag_or_memory_borrow",
        allowed_modes: collaboration.allowedModeIds,
        allow_subagents: collaboration.allowSubagents,
        max_collaborators: collaboration.maxCollaborators,
        user_requested_modes: collaboratorRequest ? [collaboratorRequest] : [],
        rule:
          "The active mode must not read another mode's RAG or memory directly. If collaboration is approved, consult the target mode agent and use only its bounded answer.",
      };
      if (disabledSkills.length > 0) turnMeta.disabled_skills = disabledSkills;
      if (customSkills.length > 0) {
        turnMeta.custom_skills = customSkills.map((c) => ({
          capability: c.capability,
          description: c.description,
          lane: c.lane,
        }));
      }
      if (override?.meta) {
        Object.assign(turnMeta, override.meta);
      }
      // Surface uploaded file metadata in audit so the runtime has a
      // record of what came alongside this turn. The `name` includes
      // any folder-relative path the picker preserved. Runtime
      // ignores unknown metadata keys today; future versions can
      // bind these to the planner's context without a wire change.
      if (sendFiles.length > 0) {
        turnMeta.uploaded_files = sendFiles.map((f) => ({
          file_id: f.file_id,
          name: f.name,
          size: f.size,
          sha256: f.sha256,
        }));
      }
      // Translate pending images into the runtime's UserAttachment
      // shape. Inline base64 (image_base64) — works with both OpenAI
      // and Anthropic; image_url is reserved for remote URLs the
      // operator might paste in a future revision.
      const attachmentsPayload: UserAttachmentPayload[] = sendImages.map(
        (img) => ({
          type: "image_base64",
          data: img.data,
          media_type: img.mediaType,
        }),
      );
      const res = await postAssistantTurn({
        user_message: wireText,
        session_id: sid,
        ...(modelChoice.service ? { credential: modelChoice.service } : {}),
        stream: true,
        use_rag: workspaceScope.kind === "ordo",
        ...(attachmentsPayload.length > 0
          ? { attachments: attachmentsPayload }
          : {}),
        ...(Object.keys(turnMeta).length > 0 ? { metadata: turnMeta } : {}),
      });
      const turn = res.turn;
      const meta: string[] = [];
      if (turn?.model) meta.push(`model: ${turn.model}`);
      if (turn?.credential_service) meta.push(`via ${turn.credential_service}`);
      const tools = turn?.context?.tool_calls ?? [];
      if (tools.length > 0) {
        meta.push(
          ...tools
            .map((t) => t.capability)
            .filter((c): c is string => typeof c === "string")
            .slice(0, 3),
        );
      }
      if (res.session_id) meta.push(`session ${res.session_id.slice(0, 8)}`);
      // Finalize: replace the streaming placeholder with the canonical
      // turn record (the WS may have raced ahead with token deltas;
      // the HTTP response is the source of truth).
      const finalAssistantText = turn?.assistant_response ?? "(empty reply)";
      setMessages((prev) => {
        if (prev.length === 0) return prev;
        const last = prev[prev.length - 1];
        if (last.role !== "assistant" || !last.streaming) return prev;
        return [
          ...prev.slice(0, -1),
          {
            role: "assistant",
            text: finalAssistantText || last.text || "(empty reply)",
            ts: tsNow(),
            meta: meta.length > 0 ? meta : undefined,
          },
        ];
      });
      if (ttsEnabled && finalAssistantText.trim()) {
        void speakText(finalAssistantText, "auto");
      }
      if (isTemporarySpecialistMode(activeMode)) {
        updateModeUiSettings(activeMode, { enabled: false });
        setActiveMode(FALLBACK_MODE_ID);
        setSessionId(undefined);
        publishUxiDebugEvent("ordo.modes", "temporary_mode_auto_disabled", "Temporary OS specialist mode returned to off after the turn.", {
          mode_id: activeMode,
        });
      }
      void refreshSessions();
    };
    try {
      try {
        await attempt(false);
        setCollaboratorRequest("");
      } catch (err: unknown) {
        if (isSessionNotFound(err)) {
          // Runtime has no record of our cached session (probably
          // restarted). Drop the stale id and try once more — the
          // attempt() helper will mint a fresh one. This is the
          // autonomous-friendly recovery path: the operator's turn
          // succeeds without them having to refresh.
          console.warn("[Assistant] session lost, recreating:", err);
          setSessionId(undefined);
          await attempt(true);
          setCollaboratorRequest("");
        } else {
          throw err;
        }
      }
    } catch (err: unknown) {
      if (suppressCancelledTurnErrorRef.current) {
        suppressCancelledTurnErrorRef.current = false;
        publishUxiDebugEvent("ordo.assistant", "interrupted_turn_cancelled", "Interrupted assistant turn cancelled.", {
          session_id: sessionId ?? null,
        });
        setMessages((prev) => {
          if (prev.length === 0) return prev;
          const last = prev[prev.length - 1];
          if (last.role !== "assistant" || !last.streaming) return prev;
          return [
            ...prev.slice(0, -1),
            {
              role: "assistant",
              text: last.text.trim()
                ? `${last.text.trim()}\n\n[interrupted by operator]`
                : "Interrupted by operator.",
              ts: tsNow(),
              meta: [...(last.meta ?? []), "interrupted"],
            },
          ];
        });
        return;
      }
      const reason = reasonFromError(err);
      console.error("[Assistant] turn failed:", err);
      // Provider hint is conditional — only nudge toward the Cloud tab
      // when the failure actually looks like a credential problem,
      // not for every error (session lost, model OOM, etc.).
      const looksLikeCredentialProblem =
        /no compatible credential|no credential|configure.*provider/i.test(
          reason,
        );
      const tail = looksLikeCredentialProblem
        ? " Configure a provider in Cloud."
        : "";
      const text = `assistant turn failed — ${reason}.${tail}`;
      setMessages((prev) => {
        if (prev.length === 0) return prev;
        const last = prev[prev.length - 1];
        if (last.role !== "assistant" || !last.streaming) {
          return [...prev, { role: "assistant", text, ts: tsNow(), meta: ["error"] }];
        }
        return [
          ...prev.slice(0, -1),
          {
            role: "assistant",
            text,
            ts: tsNow(),
            meta: ["error"],
          },
        ];
      });
    } finally {
      const queued = shiftQueuedAssistantTurn();
      setSending(false);
      setInterrupting(false);
      suppressCancelledTurnErrorRef.current = false;
      if (queued) {
        window.setTimeout(() => {
          void send({ text: queued.text, meta: queued.meta });
        }, 0);
      }
    }
  };

  const closeMidTaskPrompt = () => {
    setMidTaskDraft(null);
    publishUxiDebugEvent("ordo.assistant", "mid_task_message_dismissed", "Mid-task message prompt dismissed.", {
      queued_count: queuedTurnsRef.current.length,
    });
  };

  const handleMidTaskAction = async (action: MidTaskAction) => {
    const text = midTaskDraft?.trim() ?? "";
    if (!text) {
      setMidTaskDraft(null);
      return;
    }
    const turn: QueuedAssistantTurn = {
      id: nextQueuedTurnId(),
      text,
      meta: {
        mid_task_action: action,
        requested_during_active_turn: true,
        queued_at: new Date().toISOString(),
      },
    };
    if (action === "queue") {
      queueAssistantTurn(turn, "back");
      setInput("");
      setMidTaskDraft(null);
      publishUxiDebugEvent("ordo.assistant", "mid_task_message_queued", "Operator message queued for the next assistant turn.", {
        queued_count: queuedTurnsRef.current.length,
      });
      return;
    }
    if (action === "steer") {
      queueAssistantTurn(
        {
          ...turn,
          meta: {
            ...turn.meta,
            priority: "front",
            steering_guidance: true,
          },
        },
        "front",
      );
      setInput("");
      setMidTaskDraft(null);
      publishUxiDebugEvent("ordo.assistant", "mid_task_steer_queued", "Operator steering guidance prioritized for the active task.", {
        queued_count: queuedTurnsRef.current.length,
      });
      return;
    }
    setInterrupting(true);
    suppressCancelledTurnErrorRef.current = true;
    queueAssistantTurn(
      {
        ...turn,
        meta: {
          ...turn.meta,
          priority: "front",
          interrupted_previous_turn: true,
        },
      },
      "front",
    );
    setInput("");
    setMidTaskDraft(null);
    setMessages((prev) => {
      if (prev.length === 0) return prev;
      const last = prev[prev.length - 1];
      if (last.role !== "assistant" || !last.streaming) return prev;
      return [
        ...prev.slice(0, -1),
        {
          role: "assistant",
          text: last.text.trim()
            ? `${last.text.trim()}\n\n[interrupted by operator]`
            : "Interrupted by operator.",
          ts: tsNow(),
          meta: [...(last.meta ?? []), "interrupted"],
        },
      ];
    });
    publishUxiDebugEvent("ordo.assistant", "mid_task_interrupt_requested", "Operator requested assistant turn interruption.", {
      session_id: sessionId ?? null,
      queued_count: queuedTurnsRef.current.length,
    }, "WARN");
    try {
      if (sessionId) {
        await cancelAssistantTurn(sessionId);
      }
      streamRef.current?.close();
      streamRef.current = null;
    } catch (err: unknown) {
      publishUxiDebugEvent("ordo.assistant", "mid_task_interrupt_failed", "Assistant turn interruption failed.", {
        session_id: sessionId ?? null,
        error: err instanceof Error ? err.message : String(err),
      }, "ERROR");
      setInterrupting(false);
    }
  };

  const stopActiveAssistantTurn = async () => {
    if (!sending || interrupting) return;
    setInterrupting(true);
    suppressCancelledTurnErrorRef.current = true;
    setMessages((prev) => {
      if (prev.length === 0) return prev;
      const last = prev[prev.length - 1];
      if (last.role !== "assistant" || !last.streaming) return prev;
      return [
        ...prev.slice(0, -1),
        {
          role: "assistant",
          text: last.text.trim()
            ? `${last.text.trim()}\n\n[stopped by operator]`
            : "Stopped by operator.",
          ts: tsNow(),
          meta: [...(last.meta ?? []), "stopped"],
        },
      ];
    });
    publishUxiDebugEvent("ordo.assistant", "active_turn_stop_requested", "Operator requested active assistant turn stop.", {
      session_id: sessionId ?? null,
    }, "WARN");
    setSending(false);
    try {
      if (sessionId) {
        await cancelAssistantTurn(sessionId);
      }
      streamRef.current?.close();
      streamRef.current = null;
    } catch (err: unknown) {
      suppressCancelledTurnErrorRef.current = false;
      publishUxiDebugEvent("ordo.assistant", "active_turn_stop_failed", "Active assistant turn stop failed.", {
        session_id: sessionId ?? null,
        error: err instanceof Error ? err.message : String(err),
      }, "ERROR");
    } finally {
      setInterrupting(false);
    }
  };

  const exportCurrentConversation = () => {
    const activeModeLabel =
      modes.find((mode) => mode.id === activeMode)?.label ?? activeMode;
    const markdown = renderChatMarkdown({
      messages,
      sessionId,
      modeId: activeMode,
      modeLabel: activeModeLabel,
    });
    const stamp = new Date().toISOString().replace(/[:.]/g, "-");
    const filename = `${filenameSafe(`ordo-${activeModeLabel}-${stamp}`)}.md`;
    downloadTextFile(filename, markdown);
    publishUxiDebugEvent("ordo.assistant", "conversation_exported", "Conversation exported as Markdown.", {
      session_id: sessionId ?? null,
      mode: activeMode,
      message_count: messages.length,
      format: "markdown",
    });
  };

  const contextUsedTokens = useMemo(
    () => estimateChatTokens(messages, input),
    [messages, input],
  );
  const setThinkingEffort = (effort: ThinkingEffort) => {
    setThinkingEffortState(effort);
    if (typeof window !== "undefined") {
      window.localStorage.setItem("ordo:thinking_effort", effort);
    }
    publishUxiDebugEvent("ordo.assistant", "thinking_effort_changed", "Thinking effort changed.", {
      effort,
    });
  };
  const cleanupSpeechAudio = () => {
    audioRef.current?.pause();
    audioRef.current = null;
    if (audioUrlRef.current) {
      URL.revokeObjectURL(audioUrlRef.current);
      audioUrlRef.current = null;
    }
  };
  useEffect(() => cleanupSpeechAudio, []);
  const setTtsEnabled = (enabled: boolean) => {
    setTtsEnabledState(enabled);
    if (typeof window !== "undefined") {
      window.localStorage.setItem(TTS_ENABLED_KEY, String(enabled));
    }
    if (!enabled) cleanupSpeechAudio();
    publishUxiDebugEvent("ordo.voice", "tts_enabled_changed", "Voice output preference changed.", {
      enabled,
    });
  };
  const setTtsModel = (model: string) => {
    setTtsModelState(model);
    if (typeof window !== "undefined") window.localStorage.setItem(TTS_MODEL_KEY, model);
  };
  const setTtsVoice = (voice: string) => {
    setTtsVoiceState(voice);
    if (typeof window !== "undefined") window.localStorage.setItem(TTS_VOICE_KEY, voice);
  };
  const setTtsFormat = (format: string) => {
    setTtsFormatState(format);
    if (typeof window !== "undefined") window.localStorage.setItem(TTS_FORMAT_KEY, format);
  };
  const speakText = async (text: string, reason: "auto" | "manual") => {
    const clean = text.trim();
    if (!clean || ttsBusy) return;
    setTtsBusy(true);
    setTtsError(null);
    try {
      cleanupSpeechAudio();
      const speech = await postVoiceSpeech({
        input: clean.slice(0, 4096),
        service: modelChoice.service ?? undefined,
        model: ttsModel,
        voice: ttsVoice,
        format: ttsFormat,
      });
      const url = URL.createObjectURL(speech.blob);
      audioUrlRef.current = url;
      const audio = new Audio(url);
      audioRef.current = audio;
      audio.onended = cleanupSpeechAudio;
      await audio.play();
      publishUxiDebugEvent("ordo.voice", "tts_playback_started", "Assistant response speech playback started.", {
        reason,
        provider: speech.provider,
        model: speech.model,
        voice: speech.voice,
        format: speech.format,
      });
    } catch (err: unknown) {
      const reasonText = err instanceof Error ? err.message : String(err);
      setTtsError(reasonText);
      publishUxiDebugEvent("ordo.voice", "tts_playback_failed", "Assistant response speech playback failed.", {
        reason,
        error: reasonText,
      }, "ERROR");
    } finally {
      setTtsBusy(false);
    }
  };
  const speakLatestAssistantMessage = () => {
    const latest = [...messages].reverse().find((message) => message.role === "assistant" && message.text.trim());
    if (latest) void speakText(latest.text, "manual");
  };
  const setActiveModelChoice = async (model: string) => {
    const service = modelChoice.service;
    if (!service || !model.trim()) return;
    const nextModel = model.trim();
    const previous = modelChoice;
    const nextChoice: ModelChoiceSignal = {
      ...previous,
      selected: nextModel,
      extras: { ...previous.extras, model: nextModel },
      options: uniqueModels([nextModel, ...previous.options]),
    };
    setModelChoice(nextChoice);
    setContextBudget((budget) => ({ ...budget, model: nextModel }));
    setModelSaving(true);
    try {
      await upsertCloudCredential({
        service,
        ...(previous.authStyle ? { auth_style: previous.authStyle } : {}),
        ...(previous.baseUrl ? { base_url: previous.baseUrl } : {}),
        label: previous.providerLabel,
        extras: nextChoice.extras,
      });
      publishUxiDebugEvent("ordo.provider", "active_model_changed", "Active chat model changed.", {
        provider: service,
        model: nextModel,
      });
    } catch (err: unknown) {
      setModelChoice(previous);
      setContextBudget((budget) => ({ ...budget, model: previous.selected || null }));
      publishUxiDebugEvent("ordo.provider", "active_model_change_failed", "Active chat model change failed.", {
        provider: service,
        model: nextModel,
        error: err instanceof Error ? err.message : String(err),
      });
    } finally {
      setModelSaving(false);
    }
  };
  const showChatComposer = tab === "assistant";
  const navigateToTab = (nextTab: string) => {
    if (nextTab !== tab && !isSettingsManagedTab(tab)) {
      setLastNonSettingsTab(tab);
    }
    setTab(nextTab);
  };
  const goBackFromSettings = () => {
    publishUxiDebugEvent("ordo.settings", "settings_back_requested", "Settings back requested.", {
      from: tab,
      to: lastNonSettingsTab,
    });
    setTab(lastNonSettingsTab);
  };
  const refreshSettingsSurface = () => {
    setSettingsRefreshNonce((value) => value + 1);
    publishUxiDebugEvent("ordo.settings", "settings_refresh_requested", "Settings refresh requested.", {
      tab,
      refresh_key: settingsRefreshNonce + 1,
    });
  };

  const renderSurface = () => {
    switch (tab) {
      case "assistant":
        return (
          <AssistantSurface
            messages={messages}
            scrollRef={assistantScrollRef}
            endRef={assistantEndRef}
            onTranscriptScroll={markTranscriptPinnedState}
            taint={sessionTaint}
            onClearTaint={() => void clearTaintHandler()}
            newChatBusy={newChatBusy}
            sessions={sessions}
            activeSessionId={sessionId}
            sessionsBusy={sessionsBusy}
            onSessionChange={(next) => void handleSessionChange(next)}
            modes={modes.filter((mode) => modeUiSetting(modeUiSettings, mode).enabled)}
            activeMode={activeMode}
            onModeChange={handleModeChange}
            collaboratorRequest={collaboratorRequest}
            onCollaboratorRequestChange={setCollaboratorRequest}
            workspaceScope={workspaceScope}
            onOpenWorkspace={() => navigateToTab("projects")}
            inspectorOpen={inspectorOpen}
            onToggleInspector={() => setInspectorOpen((v) => !v)}
            modeEvents={modeEventLog}
          />
        );
      case "modes":
        return (
          <ModesSurface
            modes={modes}
            activeMode={activeMode}
            settings={modeUiSettings}
            onModeChange={handleModeChange}
            onSettingsChange={updateModeUiSettings}
          />
        );
      case "skills":
        return <SkillsSurface onOpenDirectoryTab={navigateToTab} />;
      case "persona":
        return <PersonaSurface />;
      case "agent-persona":
        return <AgentPersonaSurface />;
      case "agent-memory":
        return <AgentMemorySurface />;
      case "rag":
        return <RagSurface />;
      case "memory":
        return <MemorySurface />;
      case "capabilities":
        return <CapabilitiesSurface />;
      case "hooks":
        return <HookManagerSurface modes={modes} />;
      case "cloud":
        return <CloudSurface />;
      case "connectors":
        return <DirectoryConnectionsSurface onOpenDirectoryTab={navigateToTab} />;
      case "connections":
        return <DeviceConnectionsSurface />;
      case "plugins":
        return <EnhancedPluginsSurface onOpenDirectoryTab={navigateToTab} />;
      case "mcp":
        return <McpSurface />;
      case "extensions":
        return <ExtensionsSurface />;
      case "settings-mcp":
        return <CustomMcpSettingsSurface />;
      case "remote-communication":
        return <RemoteCommunicationSurface refreshKey={settingsRefreshNonce} />;
      case "apps":
        return <AppsSurface />;
      case "files":
        return <FilesSurface />;
      case "webhooks":
        return <WebhooksSurface />;
      case "automation":
      case "routines":
        return <RoutinesSurface modes={modes} />;
      case "builds":
        return <BuildsSurface />;
      case "dreaming":
        return (
          <DreamingSurface
            modes={modes}
            activeMode={activeMode}
            settings={modeUiSettings}
            onModeChange={handleModeChange}
            onSettingsChange={updateModeUiSettings}
          />
        );
      case "diagnostic":
        return (
          <DiagnosticSurface
            modes={modes}
            activeMode={activeMode}
            settings={modeUiSettings}
            onModeChange={handleModeChange}
            onSettingsChange={updateModeUiSettings}
          />
        );
      case "projects":
        return (
          <ProjectsSurface
            scope={workspaceScope}
            onScopeChange={setWorkspaceScope}
          />
        );
      case "artifacts":
        return <ArtifactsSurface />;
      case "docs":
        return <DocsSurface />;
      case "dev-docs":
        return <DevDocsSurface />;
      case "security":
        return <SecurityHealthSurface />;
      case "review":
        return <ReviewSurface />;
      case "runtime":
        return <RuntimeSurface />;
      case "settings-general":
        return <GeneralSettingsSurface />;
      case "settings-profile":
        return <PlaceholderSettingsSurface kind="Profile" icon={<User size={22} />} />;
      case "settings-appearance":
        return <AppearanceSettingsSurface theme={theme} onThemeChange={handleThemeChange} />;
      case "settings-configuration":
        return <PlaceholderSettingsSurface kind="Configuration" icon={<SlidersHorizontal size={22} />} />;
      case "settings-personalization":
        return <PlaceholderSettingsSurface kind="Personalization" icon={<Sparkles size={22} />} />;
      case "settings-keyboard":
        return <PlaceholderSettingsSurface kind="Keyboard shortcuts" icon={<Keyboard size={22} />} />;
      case "settings-browser":
        return <PlaceholderSettingsSurface kind="Browser" icon={<Globe size={22} />} />;
      case "settings-computer-use":
        return <PlaceholderSettingsSurface kind="Computer use" icon={<Monitor size={22} />} />;
      case "settings-git":
        return <PlaceholderSettingsSurface kind="Git" icon={<GitBranch size={22} />} />;
      case "settings-environments":
        return <PlaceholderSettingsSurface kind="Environments" icon={<Terminal size={22} />} />;
      case "settings-worktrees":
        return <PlaceholderSettingsSurface kind="Worktrees" icon={<FolderUp size={22} />} />;
      case "archived-chats":
        return <ArchivedChatsSurface />;
      case "settings":
        return <SettingsSurface activeTab={tab} onOpen={navigateToTab} />;
      default:
        return null;
    }
  };

  return (
    <>
      <style>{`
        @import url('https://fonts.googleapis.com/css2?family=Fraunces:opsz,wght@9..144,300;9..144,400;9..144,500;9..144,600&family=JetBrains+Mono:wght@400;500&display=swap');
        * { box-sizing: border-box; }
        textarea, input, button { font-family: inherit; }
        ::-webkit-scrollbar { width: 6px; height: 6px; }
        ::-webkit-scrollbar-track { background: transparent; }
        ::-webkit-scrollbar-thumb { background: rgba(255,255,255,0.08); border-radius: 3px; }
        ::-webkit-scrollbar-thumb:hover { background: rgba(255,255,255,0.15); }
        /* Pulse animation for the Review tab when reviews are pending.
           Uses the lamp-gold accent so it ties into the rest of the
           palette. The keyframes shift the left-border + background
           tint so the eye catches the row even at peripheral vision. */
        @keyframes ordoPulse {
          0%   { box-shadow: inset 2px 0 0 ${LAMP}, 0 0 0 0 ${LAMP}66; background: ${LAMP}1a; }
          50%  { box-shadow: inset 2px 0 0 ${LAMP_HOT}, 0 0 8px 1px ${LAMP_HOT}55; background: ${LAMP_HOT}26; }
          100% { box-shadow: inset 2px 0 0 ${LAMP}, 0 0 0 0 ${LAMP}66; background: ${LAMP}1a; }
        }
        .ordo-pulse { animation: ordoPulse 1.6s ease-in-out infinite; }
        /* Mic-recording pulse — red ring around the mic button when
           SpeechRecognition is actively listening. Faster cadence
           than the review pulse so it reads as "happening now"
           rather than "needs attention". */
        @keyframes ordoMicPulse {
          0%   { box-shadow: 0 0 0 0 ${RED}88; }
          70%  { box-shadow: 0 0 0 8px ${RED}00; }
          100% { box-shadow: 0 0 0 0 ${RED}00; }
        }
        .ordo-mic-pulse { animation: ordoMicPulse 1.2s ease-out infinite; }
      `}</style>
      <div
        className={`ordo-theme-${theme}`}
        style={{
          width: "100%",
          height: "100vh",
          background: `radial-gradient(ellipse at top, ${INK_2} 0%, ${INK} 62%, ${INK} 100%)`,
          fontFamily: FRAUNCES,
          color: PARCHMENT,
          overflow: "hidden",
          position: "relative",
          display: "flex",
          flexDirection: "column",
        }}
      >
        <div
          className="pointer-events-none absolute inset-0"
          style={{
            background:
              "radial-gradient(circle at 20% 0%, rgba(244,201,93,0.05), transparent 50%), radial-gradient(circle at 80% 100%, rgba(127,209,197,0.03), transparent 60%)",
          }}
        />

        {/* Rescue Mode amber flood — gates on gateway === "err". */}
        <AnimatePresence>
          {rescueActive && (
            <motion.div
              key="rescue"
              className="pointer-events-none absolute inset-0 z-20"
              initial={{ opacity: 0 }}
              animate={{ opacity: 1 }}
              exit={{ opacity: 0 }}
              transition={{ duration: 0.4 }}
              style={{
                background: `radial-gradient(ellipse at center, ${LAMP_HOT}2e 0%, ${LAMP_HOT}14 40%, transparent 75%)`,
                mixBlendMode: "screen",
              }}
            />
          )}
        </AnimatePresence>

        {/* TOP STATUS BAR */}
        <div
          className="relative z-10 flex items-center justify-between px-6 py-3"
          style={{
            borderBottom: "1px solid var(--ordo-shell-border)",
            background: "var(--ordo-shell-top-bg)",
            backdropFilter: "blur(20px)",
          }}
        >
          <Lamp />
          <div className="flex items-center gap-5">
            {allSignals.map((s) => (
              <Signal key={s.id} sig={s} />
            ))}
          </div>
          <div className="flex items-center gap-2">
            <Mono size={10} color="rgba(255,255,255,0.4)">
              standard · 4141
            </Mono>
          </div>
        </div>

        {/* MAIN */}
        <div className="relative z-10 flex-1 flex min-h-0">
          {/* LEFT TAB RAIL */}
          <div
            style={{
              width: 188,
              flexShrink: 0,
              borderRight: "1px solid var(--ordo-shell-border)",
              background: "var(--ordo-shell-rail-bg)",
              padding: "16px 8px",
              overflow: "auto",
            }}
          >
            {(["primary", "agent", "knowledge", "connectivity", "advanced", "docs"] as const).map((group) => {
              const groupTabs = TABS.filter((t) => t.group === group && LEFT_RAIL_TAB_IDS.has(t.id));
              // Skip empty groups so the rail doesn't show an
              // orphaned label + chevron when a group is in the
              // type but currently has no members. (connectivity is
              // empty after Plugins/MCP/Connections relocated.)
              if (groupTabs.length === 0) return null;
              const isCollapsible = group === "advanced";
              // Auto-expand the advanced cabinet when one of its tabs
              // is the active surface — otherwise it'd appear closed
              // even though we're inside it.
              const containsActive = groupTabs.some((t) => t.id === tab);
              const open = isCollapsible ? advancedOpen || containsActive : true;
              return (
              <div key={group} className="mb-4">
                {isCollapsible ? (
                  <button
                    onClick={() => setAdvancedOpen((v) => !v)}
                    className="w-full text-left flex items-center justify-between rounded-md transition-all"
                    style={{
                      padding: "6px 10px 6px 12px",
                      marginBottom: 4,
                      background: "transparent",
                      border: "none",
                      cursor: "pointer",
                    }}
                    title={open ? "collapse" : "expand"}
                  >
                    <Mono size={9} upper track="0.3em" color="rgba(255,255,255,0.5)">
                      {group}
                    </Mono>
                    {open ? (
                      <ChevronDown size={11} color="rgba(255,255,255,0.45)" />
                    ) : (
                      <ChevronUp
                        size={11}
                        color="rgba(255,255,255,0.45)"
                        style={{ transform: "rotate(180deg)" }}
                      />
                    )}
                  </button>
                ) : (
                  <Mono
                    size={9}
                    upper
                    track="0.3em"
                    color="rgba(255,255,255,0.3)"
                    style={{ paddingLeft: 12, marginBottom: 6, display: "block" }}
                  >
                    {group}
                  </Mono>
                )}
                {open && groupTabs.map((t) => {
                  const Icon = t.glyph;
                  const active = tab === t.id;
                  // The Review tab pulses + shows a count badge when
                  // there are pending requests AND we're not already
                  // looking at it. Once the operator clicks in, the
                  // pulse stops — being on the surface is acknowledgment
                  // even if they haven't approved/denied yet.
                  const showReviewPulse =
                    t.id === "review" && pendingReviewCount > 0 && !active;
                  return (
                    <button
                      key={t.id}
                      onClick={() => navigateToTab(t.id)}
                      className={`w-full text-left flex items-center gap-2.5 rounded-md transition-all${showReviewPulse ? " ordo-pulse" : ""}`}
                      style={{
                        padding: "7px 10px",
                        marginBottom: 1,
                        background: active
                          ? `linear-gradient(90deg, ${LAMP}1f, transparent)`
                          : "transparent",
                        borderLeft: active ? `2px solid ${LAMP}` : "2px solid transparent",
                      }}
                    >
                      <Icon size={14} color={active ? LAMP : "rgba(255,255,255,0.5)"} />
                      <span
                        style={{
                          fontFamily: FRAUNCES,
                          fontSize: 13,
                          color: active ? PARCHMENT : "rgba(255,255,255,0.65)",
                          fontWeight: active ? 500 : 400,
                          flex: 1,
                        }}
                      >
                        {t.label}
                      </span>
                      {t.id === "review" && pendingReviewCount > 0 && (
                        <span
                          style={{
                            fontFamily: MONO,
                            fontSize: 10,
                            fontWeight: 700,
                            padding: "2px 6px",
                            borderRadius: 999,
                            background: LAMP_HOT,
                            color: INK,
                            minWidth: 18,
                            textAlign: "center",
                            lineHeight: 1,
                          }}
                          title={`${pendingReviewCount} pending review${pendingReviewCount === 1 ? "" : "s"}`}
                        >
                          {pendingReviewCount > 99 ? "99+" : pendingReviewCount}
                        </span>
                      )}
                    </button>
                  );
                })}
              </div>
              );
            })}
          </div>

          {/* SURFACE */}
          <div className="flex-1 flex flex-col min-w-0">
            <div
              className={`flex-1 p-7 ${showChatComposer ? "overflow-hidden" : "overflow-auto"}`}
              style={{ minHeight: 0 }}
            >
              <AnimatePresence mode="wait">
                <motion.div
                  key={tab}
                  initial={{ opacity: 0, y: 6 }}
                  animate={{ opacity: 1, y: 0 }}
                  exit={{ opacity: 0, y: -6 }}
                  transition={{ duration: 0.25 }}
                  className="h-full flex flex-col gap-4"
                >
                  {isSettingsManagedTab(tab) && (
                    <SettingsNavigationBar
                      backLabel={tabLabel(lastNonSettingsTab)}
                      onBack={goBackFromSettings}
                      onRefresh={refreshSettingsSurface}
                    />
                  )}
                  <div
                    className={
                      showChatComposer || isSettingsManagedTab(tab)
                        ? "flex-1 min-h-0"
                        : "h-full"
                    }
                    style={showChatComposer ? { overflow: "hidden" } : undefined}
                  >
                    {renderSurface()}
                  </div>
                </motion.div>
              </AnimatePresence>
            </div>

            {/*
              Approval render slot — reserved for the artifact preview pane that
              surfaces inspectable assistant output above the chat input before
              the operator approves it. Layout placeholder only; the rendering
              decision is deferred (see top-of-file notes). When wired, this
              <div> hosts the artifact pane + its approve/reject buttons.
            */}
            {showChatComposer && <div data-slot="approval-render" />}

            {/* CHAT — anchored bottom */}
            <div
              style={{
                display: showChatComposer ? undefined : "none",
                borderTop: "1px solid rgba(255,255,255,0.06)",
                background: "rgba(10,12,16,0.7)",
                backdropFilter: "blur(20px)",
              }}
            >
              <div
                className="px-6 py-2.5 flex items-center gap-3"
                style={{ borderBottom: "1px solid rgba(255,255,255,0.04)" }}
              >
                <ContextUsageIndicator
                  usedTokens={contextUsedTokens}
                  budget={contextBudget}
                  modelChoice={modelChoice}
                  modelSaving={modelSaving}
                  onModelChange={setActiveModelChoice}
                  thinkingEffort={thinkingEffort}
                  onThinkingEffortChange={setThinkingEffort}
                />
                <label className="flex items-center gap-2" title="Speak assistant responses with cloud TTS">
                  <input
                    type="checkbox"
                    checked={ttsEnabled}
                    onChange={(event) => setTtsEnabled(event.target.checked)}
                  />
                  <Mono size={10} upper track="0.16em" color={ttsEnabled ? LAMP : SLATE}>
                    speak
                  </Mono>
                </label>
                <select
                  value={ttsModel}
                  onChange={(event) => setTtsModel(event.target.value)}
                  disabled={!ttsEnabled || ttsBusy}
                  title="Speech model"
                  style={{
                    fontFamily: MONO,
                    fontSize: 10,
                    background: "rgba(255,255,255,0.04)",
                    color: PARCHMENT,
                    border: "1px solid rgba(255,255,255,0.1)",
                    borderRadius: 999,
                    padding: "5px 10px",
                    opacity: ttsEnabled ? 1 : 0.45,
                  }}
                >
                  {TTS_MODEL_OPTIONS.map((model) => (
                    <option key={model} value={model}>{model}</option>
                  ))}
                </select>
                <select
                  value={ttsVoice}
                  onChange={(event) => setTtsVoice(event.target.value)}
                  disabled={!ttsEnabled || ttsBusy}
                  title="Speech voice"
                  style={{
                    fontFamily: MONO,
                    fontSize: 10,
                    background: "rgba(255,255,255,0.04)",
                    color: PARCHMENT,
                    border: "1px solid rgba(255,255,255,0.1)",
                    borderRadius: 999,
                    padding: "5px 10px",
                    opacity: ttsEnabled ? 1 : 0.45,
                  }}
                >
                  {TTS_VOICE_OPTIONS.map((voice) => (
                    <option key={voice} value={voice}>{voice}</option>
                  ))}
                </select>
                <button
                  type="button"
                  onClick={speakLatestAssistantMessage}
                  disabled={ttsBusy || messages.every((message) => message.role !== "assistant" || !message.text.trim())}
                  title="Speak the latest assistant message"
                  className="rounded-full px-2.5 py-1.5 transition-all"
                  style={{
                    background: ttsBusy ? `${LAMP}22` : "rgba(255,255,255,0.04)",
                    border: `1px solid ${ttsBusy ? `${LAMP}55` : "rgba(255,255,255,0.1)"}`,
                    color: ttsBusy ? LAMP : PARCHMENT,
                    opacity: ttsBusy ? 0.75 : 1,
                  }}
                >
                  <Volume2 size={13} />
                </button>
              </div>

              <Modal
                open={midTaskDraft !== null}
                onClose={closeMidTaskPrompt}
                title="Agent is working"
                sub="Choose how Ordo should handle this message."
                width={620}
                footer={
                  <>
                    <Button onClick={closeMidTaskPrompt}>Keep typing</Button>
                    <Button onClick={() => void handleMidTaskAction("queue")}>
                      Queue next
                    </Button>
                    <Button onClick={() => void handleMidTaskAction("steer")} variant="primary">
                      Steer
                    </Button>
                    <Button
                      onClick={() => void handleMidTaskAction("interrupt")}
                      variant="danger"
                      disabled={interrupting}
                    >
                      {interrupting ? "Interrupting..." : "Interrupt and send"}
                    </Button>
                  </>
                }
              >
                <div className="space-y-3">
                  <Alert variant="warn">
                    Steer prioritizes the message as guidance for the active task. Queue sends it after the current turn. Interrupt cancels the active turn and sends this message next.
                  </Alert>
                  <Textarea
                    value={midTaskDraft ?? ""}
                    onChange={setMidTaskDraft}
                    rows={5}
                    placeholder="Add steering guidance, a queued follow-up, or the replacement instruction..."
                  />
                  {queuedTurns.length > 0 && (
                    <Mono size={10} color={SLATE}>
                      {queuedTurns.length} queued turn{queuedTurns.length === 1 ? "" : "s"} waiting.
                    </Mono>
                  )}
                </div>
              </Modal>

              {/* Voice-input error notice. Auto-dismisses on next
                  successful start; also dismissable via the × so
                  the operator can clear a stale message. */}
              {voiceError && (
                <div
                  className="px-6 py-2 flex items-center gap-2"
                  style={{
                    background: `${RED}11`,
                    borderBottom: `1px solid ${RED}33`,
                  }}
                >
                  <Mic size={12} color={RED} />
                  <Mono size={10} color={RED}>
                    {voiceError}
                  </Mono>
                  <span style={{ flex: 1 }} />
                  <button
                    onClick={() => setVoiceError(null)}
                    style={{
                      background: "transparent",
                      border: "none",
                      color: RED,
                      cursor: "pointer",
                      padding: 0,
                      display: "inline-flex",
                      alignItems: "center",
                    }}
                    title="Dismiss"
                  >
                    <X size={12} />
                  </button>
                </div>
              )}

              {ttsError && (
                <div
                  className="px-6 py-2 flex items-center gap-2"
                  style={{
                    background: `${RED}11`,
                    borderBottom: `1px solid ${RED}33`,
                  }}
                >
                  <Volume2 size={12} color={RED} />
                  <Mono size={10} color={RED}>
                    speech output failed: {ttsError}
                  </Mono>
                  <span style={{ flex: 1 }} />
                  <button
                    onClick={() => setTtsError(null)}
                    style={{
                      background: "transparent",
                      border: "none",
                      color: RED,
                      cursor: "pointer",
                      padding: 0,
                      display: "inline-flex",
                      alignItems: "center",
                    }}
                    title="Dismiss"
                  >
                    <X size={12} />
                  </button>
                </div>
              )}

              {/* Pending attachments — chip strip above the input.
                  Operator sees what's about to ship, can remove
                  individual items, sees per-file upload progress.
                  Hidden when nothing is queued so the resting chat
                  input stays compact. */}
              {(pendingImages.length > 0 || pendingFiles.length > 0) && (
                <div
                  className="px-6 py-2 flex flex-wrap gap-2"
                  style={{ borderBottom: "1px solid rgba(255,255,255,0.04)" }}
                >
                  {pendingImages.map((img) => (
                    <div
                      key={img.id}
                      className="flex items-center gap-2 rounded-md px-2 py-1"
                      style={{
                        background: "rgba(255,255,255,0.05)",
                        border: "1px solid rgba(255,255,255,0.1)",
                      }}
                      title={`${img.name} · ${fmtBytes(img.size)} · ${img.mediaType}`}
                    >
                      <img
                        src={`data:${img.mediaType};base64,${img.data}`}
                        alt=""
                        style={{
                          width: 22,
                          height: 22,
                          objectFit: "cover",
                          borderRadius: 3,
                        }}
                      />
                      <Mono size={10} color={PARCHMENT}>
                        {img.name.length > 28
                          ? `${img.name.slice(0, 25)}…`
                          : img.name}
                      </Mono>
                      <button
                        onClick={() => removePendingImage(img.id)}
                        disabled={sending}
                        style={{
                          background: "transparent",
                          border: "none",
                          color: "rgba(255,255,255,0.5)",
                          cursor: "pointer",
                          padding: 0,
                          display: "inline-flex",
                          alignItems: "center",
                        }}
                        title="Remove"
                      >
                        <X size={12} />
                      </button>
                    </div>
                  ))}
                  {pendingFiles.map((f) => (
                    <div
                      key={f.id}
                      className="flex items-center gap-2 rounded-md px-2 py-1"
                      style={{
                        background: f.error
                          ? "rgba(232,93,93,0.15)"
                          : "rgba(255,255,255,0.05)",
                        border: `1px solid ${f.error ? "rgba(232,93,93,0.4)" : "rgba(255,255,255,0.1)"}`,
                        opacity: f.uploading ? 0.6 : 1,
                      }}
                      title={
                        f.error
                          ? `upload failed: ${f.error}`
                          : `${f.name} · ${fmtBytes(f.size)}${f.uploading ? " · uploading…" : ""}`
                      }
                    >
                      <FileText size={11} color={f.error ? RED : SLATE} />
                      <Mono size={10} color={PARCHMENT}>
                        {f.name.length > 32
                          ? `${f.name.slice(0, 29)}…`
                          : f.name}
                      </Mono>
                      {f.uploading && (
                        <Mono size={9} color={SLATE}>
                          ↑
                        </Mono>
                      )}
                      <button
                        onClick={() => removePendingFile(f.id)}
                        disabled={sending}
                        style={{
                          background: "transparent",
                          border: "none",
                          color: "rgba(255,255,255,0.5)",
                          cursor: "pointer",
                          padding: 0,
                          display: "inline-flex",
                          alignItems: "center",
                        }}
                        title="Remove"
                      >
                        <X size={12} />
                      </button>
                    </div>
                  ))}
                </div>
              )}

              <div className="px-4 py-3 flex items-end gap-2">
                {/* Three hidden file inputs — image / file / folder.
                    Each opens with appropriate filters. The folder
                    input uses webkitdirectory so the picker becomes
                    a folder selector and we receive every descendant
                    with its relative path. */}
                <input
                  ref={imageInputRef}
                  type="file"
                  accept="image/*"
                  multiple
                  style={{ display: "none" }}
                  onChange={(e) => void onPickImages(e)}
                />
                <input
                  ref={fileInputRef}
                  type="file"
                  multiple
                  style={{ display: "none" }}
                  onChange={(e) => void onPickFiles(e)}
                />
                <input
                  ref={folderInputRef}
                  type="file"
                  multiple
                  // @ts-expect-error — webkitdirectory is a valid HTML
                  // attribute that React's typings don't expose.
                  webkitdirectory=""
                  directory=""
                  style={{ display: "none" }}
                  onChange={(e) => void onPickFiles(e)}
                />
                {/* Voice input — Web Speech API. Toggles recording.
                    Pulses red while listening; greyed out + tooltip
                    explains when unsupported. Spoken text streams
                    into the input field as it's recognized. */}
                <button
                  onClick={undefined}
                  disabled={sending || voiceUnsupported}
                  className={`rounded-lg px-2 py-2 transition-all${
                    isListening ? " ordo-mic-pulse" : ""
                  }`}
                  style={{
                    display: "none",
                    background: isListening
                      ? `${RED}22`
                      : "rgba(255,255,255,0.04)",
                    border: `1px solid ${
                      isListening ? `${RED}88` : "rgba(255,255,255,0.08)"
                    }`,
                    color: isListening ? RED : PARCHMENT,
                    cursor:
                      sending || voiceUnsupported ? "not-allowed" : "pointer",
                    opacity: voiceUnsupported ? 0.4 : 1,
                  }}
                  title={
                    voiceUnsupported
                      ? "Voice input unavailable in this runtime (no SpeechRecognition API)"
                      : isListening
                      ? "Stop listening"
                      : "Start voice input — speech streams into the message field"
                  }
                >
                  {isListening ? <MicOff size={14} /> : <Mic size={14} />}
                </button>
                <div
                  className="flex-1 rounded-xl px-4 py-3"
                  style={{
                    background: "rgba(255,255,255,0.03)",
                    border: "1px solid rgba(255,255,255,0.08)",
                    minHeight: 148,
                    display: "flex",
                    flexDirection: "column",
                    alignItems: "stretch",
                    gap: 10,
                  }}
                >
                  <textarea
                    value={input}
                    onChange={(e) => setInput(e.target.value)}
                    onKeyDown={(e) => {
                      if (e.key === "Enter" && !e.shiftKey) {
                        e.preventDefault();
                        void send();
                      }
                    }}
                    placeholder={
                      sending
                        ? "Agent is working. Type a steer, queue, or interrupt..."
                        : anyUploadInFlight
                        ? "uploading…"
                        : pendingImages.length + pendingFiles.length > 0
                        ? "add a question or just send the attachments…"
                        : "Tell Ordo the brief…"
                    }
                    className="w-full bg-transparent outline-none"
                    rows={4}
                    style={{
                      fontFamily: FRAUNCES,
                      color: PARCHMENT,
                      fontSize: 15,
                      lineHeight: 1.45,
                      resize: "none",
                      minHeight: 86,
                      maxHeight: 160,
                      flex: 1,
                    }}
                  />
                  <div
                    className="flex items-center justify-between gap-3"
                    style={{
                      borderTop: "1px solid rgba(255,255,255,0.06)",
                      paddingTop: 8,
                    }}
                  >
                    <div className="flex items-center gap-2">
                      <button
                        onClick={() => imageInputRef.current?.click()}
                        disabled={sending}
                        className="rounded-lg px-2 py-2 transition-all"
                        style={{
                          background: "rgba(255,255,255,0.04)",
                          border: "1px solid rgba(255,255,255,0.08)",
                          color: PARCHMENT,
                          cursor: sending ? "not-allowed" : "pointer",
                        }}
                        title="Attach image (vision)"
                      >
                        <ImageIcon size={14} />
                      </button>
                      <button
                        onClick={() => fileInputRef.current?.click()}
                        disabled={sending}
                        className="rounded-lg px-2 py-2 transition-all"
                        style={{
                          background: "rgba(255,255,255,0.04)",
                          border: "1px solid rgba(255,255,255,0.08)",
                          color: PARCHMENT,
                          cursor: sending ? "not-allowed" : "pointer",
                        }}
                        title="Attach files (uploaded to user-files)"
                      >
                        <Paperclip size={14} />
                      </button>
                      <button
                        onClick={() => folderInputRef.current?.click()}
                        disabled={sending}
                        className="rounded-lg px-2 py-2 transition-all"
                        style={{
                          background: "rgba(255,255,255,0.04)",
                          border: "1px solid rgba(255,255,255,0.08)",
                          color: PARCHMENT,
                          cursor: sending ? "not-allowed" : "pointer",
                        }}
                        title="Attach folder (uploaded recursively)"
                      >
                        <FolderUp size={14} />
                      </button>
                      <button
                        onClick={exportCurrentConversation}
                        disabled={messages.length === 0}
                        className="rounded-lg px-2 py-2 transition-all"
                        style={{
                          background: "rgba(255,255,255,0.04)",
                          border: "1px solid rgba(255,255,255,0.08)",
                          color: PARCHMENT,
                          cursor: messages.length === 0 ? "not-allowed" : "pointer",
                          opacity: messages.length === 0 ? 0.45 : 1,
                        }}
                        title="Export this conversation as Markdown"
                      >
                        <Download size={14} />
                      </button>
                      <button
                        onClick={toggleListening}
                        disabled={sending || voiceUnsupported}
                        className={`rounded-lg px-2 py-2 transition-all${
                          isListening ? " ordo-mic-pulse" : ""
                        }`}
                        style={{
                          background: isListening
                            ? `${RED}22`
                            : "rgba(255,255,255,0.04)",
                          border: `1px solid ${
                            isListening ? `${RED}88` : "rgba(255,255,255,0.08)"
                          }`,
                          color: isListening ? RED : PARCHMENT,
                          cursor:
                            sending || voiceUnsupported ? "not-allowed" : "pointer",
                          opacity: voiceUnsupported ? 0.4 : 1,
                        }}
                        title={
                          voiceUnsupported
                            ? "Voice input unavailable in this runtime (no SpeechRecognition API)"
                            : isListening
                            ? "Stop listening"
                            : "Start voice input - speech streams into the message field"
                        }
                      >
                        {isListening ? <MicOff size={14} /> : <Mic size={14} />}
                      </button>
                    </div>
                    <div className="flex items-center gap-2">
                      <button
                        onClick={() => void handleNewChat()}
                        disabled={newChatBusy || sending}
                        className="rounded-xl px-3 py-2.5 transition-all"
                        style={{
                          fontFamily: MONO,
                          fontSize: 10,
                          background: `${LAMP}18`,
                          color: LAMP,
                          border: `1px solid ${LAMP}44`,
                          cursor: newChatBusy || sending ? "not-allowed" : "pointer",
                          opacity: newChatBusy || sending ? 0.55 : 1,
                        }}
                        title={sending ? "Stop or finish the current turn before starting a new chat" : "Start a new chat"}
                        aria-label="start new chat"
                      >
                        {newChatBusy ? "STARTING" : "+ CHAT"}
                      </button>
                      <button
                        onClick={() => (sending ? void stopActiveAssistantTurn() : void send())}
                        disabled={
                          sending
                            ? interrupting
                            : anyUploadInFlight ||
                              (!input.trim() &&
                                pendingImages.length === 0 &&
                                pendingFiles.length === 0)
                        }
                        className="rounded-xl px-4 py-2.5 transition-all"
                        style={{
                          background: sending
                            ? `linear-gradient(180deg, ${RED}, #b83f3f)`
                            : `linear-gradient(180deg, ${LAMP}, #c89a3d)`,
                          color: sending ? PARCHMENT : INK,
                          boxShadow: sending ? `0 6px 16px ${RED}30` : `0 6px 16px ${LAMP}40`,
                          opacity: anyUploadInFlight || interrupting ? 0.6 : 1,
                        }}
                        title={sending ? "Stop current assistant turn" : "Send message"}
                      >
                        {sending ? <Square size={14} strokeWidth={2.5} /> : <Send size={14} strokeWidth={2.5} />}
                      </button>
                    </div>
                  </div>
                </div>
              </div>
            </div>
          </div>
        </div>
      </div>
    </>
  );
}
