import { startTransition, useEffect, useState } from "react";
import { listen } from "@tauri-apps/api/event";

export type SystemState = "HEALTHY" | "PROCESSING" | "RESCUE" | "CRITICAL";
export type LogLevel = "INFO" | "WARN" | "ERROR";

export interface Log {
  id: string;
  source: string;
  message: string;
  level: LogLevel;
  timestamp: string;
}

export interface AuraTone {
  shellBackground: string;
  primaryGlow: string;
  secondaryGlow: string;
  tertiaryGlow: string;
  border: string;
  badge: string;
  panelTint: string;
}

const AURA_BY_STATE: Record<SystemState, AuraTone> = {
  HEALTHY: {
    shellBackground:
      "linear-gradient(160deg, rgba(2, 6, 23, 0.98), rgba(8, 15, 42, 0.94) 46%, rgba(2, 132, 199, 0.08) 100%)",
    primaryGlow: "radial-gradient(circle, rgba(13, 148, 136, 0.42), transparent 68%)",
    secondaryGlow: "radial-gradient(circle, rgba(59, 130, 246, 0.26), transparent 70%)",
    tertiaryGlow: "radial-gradient(circle, rgba(45, 212, 191, 0.16), transparent 72%)",
    border: "rgba(94, 234, 212, 0.28)",
    badge: "#5eead4",
    panelTint: "rgba(6, 14, 28, 0.72)",
  },
  PROCESSING: {
    shellBackground:
      "linear-gradient(160deg, rgba(2, 6, 23, 0.98), rgba(7, 17, 47, 0.96) 46%, rgba(30, 64, 175, 0.12) 100%)",
    primaryGlow: "radial-gradient(circle, rgba(59, 130, 246, 0.4), transparent 68%)",
    secondaryGlow: "radial-gradient(circle, rgba(13, 148, 136, 0.24), transparent 72%)",
    tertiaryGlow: "radial-gradient(circle, rgba(147, 197, 253, 0.16), transparent 72%)",
    border: "rgba(125, 211, 252, 0.28)",
    badge: "#93c5fd",
    panelTint: "rgba(7, 15, 33, 0.76)",
  },
  RESCUE: {
    shellBackground:
      "linear-gradient(160deg, rgba(15, 9, 3, 0.98), rgba(19, 12, 4, 0.96) 46%, rgba(120, 53, 15, 0.18) 100%)",
    primaryGlow: "radial-gradient(circle, rgba(245, 158, 11, 0.34), transparent 68%)",
    secondaryGlow: "radial-gradient(circle, rgba(217, 119, 6, 0.18), transparent 74%)",
    tertiaryGlow: "radial-gradient(circle, rgba(59, 130, 246, 0.12), transparent 74%)",
    border: "rgba(251, 191, 36, 0.22)",
    badge: "#fbbf24",
    panelTint: "rgba(20, 12, 4, 0.78)",
  },
  CRITICAL: {
    shellBackground:
      "linear-gradient(160deg, rgba(10, 2, 8, 0.98), rgba(18, 3, 10, 0.98) 48%, rgba(153, 27, 27, 0.22) 100%)",
    primaryGlow: "radial-gradient(circle, rgba(239, 68, 68, 0.36), transparent 68%)",
    secondaryGlow: "radial-gradient(circle, rgba(153, 27, 27, 0.28), transparent 72%)",
    tertiaryGlow: "radial-gradient(circle, rgba(244, 63, 94, 0.16), transparent 74%)",
    border: "rgba(248, 113, 113, 0.24)",
    badge: "#f87171",
    panelTint: "rgba(18, 6, 10, 0.8)",
  },
};

function isTauriRuntime(): boolean {
  return (
    typeof window !== "undefined" &&
    typeof (window as Window & { __TAURI_INTERNALS__?: unknown }).__TAURI_INTERNALS__ !==
      "undefined"
  );
}

function makeLogId() {
  return `${Date.now()}-${Math.random().toString(36).slice(2, 8)}`;
}

function normalizeLog(partial: Partial<Log> & Pick<Log, "source" | "message" | "level">): Log {
  return {
    id: partial.id ?? makeLogId(),
    source: partial.source,
    message: partial.message,
    level: partial.level,
    timestamp: partial.timestamp ?? new Date().toISOString(),
  };
}

function deriveState(current: SystemState, entry: Log): SystemState {
  const lowered = entry.message.toLowerCase();
  if (entry.level === "ERROR") {
    if (lowered.includes("critical") || lowered.includes("containment")) {
      return "CRITICAL";
    }
    return "RESCUE";
  }
  if (entry.level === "WARN" && current === "HEALTHY") {
    return "PROCESSING";
  }
  if (
    lowered.includes("stabilized") ||
    lowered.includes("restored") ||
    lowered.includes("fallback retired")
  ) {
    return "HEALTHY";
  }
  if (
    lowered.includes("scan initiated") ||
    lowered.includes("investigating") ||
    lowered.includes("analyzing")
  ) {
    return current === "CRITICAL" ? "CRITICAL" : "PROCESSING";
  }
  return current;
}

export function systemStateLabel(state: SystemState) {
  switch (state) {
    case "HEALTHY":
      return "Mesh stable";
    case "PROCESSING":
      return "System tuning";
    case "RESCUE":
      return "Rescue protocol";
    case "CRITICAL":
      return "Containment";
  }
}

export function useSystemState(initialState: SystemState = "HEALTHY") {
  const [status, setStatus] = useState<SystemState>(initialState);
  const [logs, setLogs] = useState<Log[]>([]);

  function appendLog(entry: Partial<Log> & Pick<Log, "source" | "message" | "level">) {
    const normalized = normalizeLog(entry);
    startTransition(() => {
      setLogs((previous) => [normalized, ...previous].slice(0, 120));
      setStatus((current) => deriveState(current, normalized));
    });
  }

  useEffect(() => {
    if (!isTauriRuntime()) {
      return;
    }

    let disposed = false;
    let off: (() => void) | undefined;

    listen<Log>("bus-event", (event) => {
      appendLog(event.payload);
    })
      .then((unlisten) => {
        if (disposed) {
          unlisten();
          return;
        }
        off = unlisten;
      })
      .catch(() => {
        appendLog({
          source: "SHELL",
          message: "Event bridge unavailable. Running with local UXI fallbacks.",
          level: "WARN",
        });
      });

    return () => {
      disposed = true;
      off?.();
    };
  }, []);

  return {
    status,
    setStatus,
    logs,
    appendLog,
    aura: AURA_BY_STATE[status],
    statusLabel: systemStateLabel(status),
  };
}
