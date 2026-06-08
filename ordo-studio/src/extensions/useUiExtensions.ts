import { useEffect, useState } from "react";

export interface UiExtensionSurface {
  kind: "tab";
  id: string;
  label: string;
  icon?: string | null;
  description?: string | null;
  entry_url: string;
}

export interface UiExtensionPermissions {
  mcp_tools: string[];
  subscribe_events: string[];
}

export interface UiExtension {
  name: string;
  version: string;
  description: string;
  author: string;
  enabled: boolean;
  surfaces: UiExtensionSurface[];
  permissions: UiExtensionPermissions;
  manifest_path: string;
}

interface UiExtensionsResponse {
  extensions_dir: string | null;
  extensions: UiExtension[];
  errors: { manifest_path: string; error: string }[];
}

export type UiExtensionsState =
  | { status: "loading" }
  | { status: "ready"; extensions: UiExtension[]; errors: UiExtensionsResponse["errors"]; origin: string }
  | { status: "error"; message: string };

const CONTROL_API_ORIGIN =
  (typeof window !== "undefined" &&
    (window as Window & { __ORDO_CONTROL_ORIGIN__?: string }).__ORDO_CONTROL_ORIGIN__) ||
  "http://127.0.0.1:4141";

export function useUiExtensions(): [UiExtensionsState, () => void] {
  const [state, setState] = useState<UiExtensionsState>({ status: "loading" });
  const [reloadTick, setReloadTick] = useState(0);

  useEffect(() => {
    let cancelled = false;
    async function load() {
      try {
        const response = await fetch(`${CONTROL_API_ORIGIN}/api/ui-extensions`);
        if (!response.ok) throw new Error(`control API returned ${response.status}`);
        const payload = (await response.json()) as UiExtensionsResponse;
        if (cancelled) return;
        setState({
          status: "ready",
          extensions: payload.extensions.filter((e) => e.enabled),
          errors: payload.errors,
          origin: CONTROL_API_ORIGIN,
        });
      } catch (error) {
        if (!cancelled) {
          setState({
            status: "error",
            message: error instanceof Error ? error.message : String(error),
          });
        }
      }
    }
    void load();
    return () => {
      cancelled = true;
    };
  }, [reloadTick]);

  return [state, () => setReloadTick((n) => n + 1)];
}
