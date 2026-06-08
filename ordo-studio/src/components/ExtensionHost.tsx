import React, { useCallback, useEffect, useMemo, useRef, useState } from "react";

import type { UiExtension, UiExtensionSurface } from "../hooks/useUiExtensions";

interface ExtensionHostProps {
  extension: UiExtension;
  surface: UiExtensionSurface;
  origin: string;
  onClose?: () => void;
}

type PendingRequest = {
  id: number;
  type: "call";
  method: string;
  params: unknown;
};

type Toast = { text: string; tone: "info" | "success" | "warn" | "error" };

function globMatches(pattern: string, input: string): boolean {
  if (pattern.endsWith("*")) {
    return input.startsWith(pattern.slice(0, -1));
  }
  return pattern === input;
}

function permits(list: string[], candidate: string): boolean {
  return list.some((p) => globMatches(p, candidate));
}

const ALLOWED_EVENT_TOPICS = [
  "review.opened",
  "review.resolved",
  "review.queue_snapshot",
  "review.*",
];

export function ExtensionHost({ extension, surface, origin, onClose }: ExtensionHostProps) {
  const iframeRef = useRef<HTMLIFrameElement | null>(null);
  const subscriptionsRef = useRef<Map<string, WebSocket>>(new Map());
  const [toast, setToast] = useState<Toast | null>(null);
  const [ready, setReady] = useState(false);

  const entryUrl = useMemo(
    () => new URL(surface.entry_url, origin).toString(),
    [surface.entry_url, origin],
  );

  /**
   * Ship a message to the iframe. Target origin "*" is safe here
   * because the iframe has sandbox="allow-scripts" with no
   * allow-same-origin — its origin is opaque. We trust the target by
   * its window reference, not by origin.
   */
  const sendToChild = useCallback((message: unknown) => {
    iframeRef.current?.contentWindow?.postMessage(message, "*");
  }, []);

  /**
   * Open a review WebSocket bridge on the iframe's behalf. Exactly
   * one per topic per host instance.
   */
  const openReviewBridge = useCallback(
    (topic: string) => {
      if (subscriptionsRef.current.has(topic)) return;
      if (!permits(extension.permissions.subscribe_events, topic)) return;
      if (!ALLOWED_EVENT_TOPICS.some((allowed) => globMatches(allowed, topic))) return;
      const url = new URL("/ws/review", origin);
      url.protocol = url.protocol === "https:" ? "wss:" : "ws:";
      const socket = new WebSocket(url.toString());
      socket.onmessage = (event) => {
        let parsed: { event?: string } & Record<string, unknown>;
        try {
          parsed = JSON.parse(event.data);
        } catch {
          return;
        }
        const eventName =
          typeof parsed.event === "string" ? `review.${parsed.event}` : "review.unknown";
        if (!globMatches(topic, eventName)) return;
        sendToChild({
          type: "event",
          topic: eventName,
          payload: parsed,
        });
      };
      socket.onclose = () => {
        subscriptionsRef.current.delete(topic);
      };
      subscriptionsRef.current.set(topic, socket);
    },
    [extension.permissions.subscribe_events, origin, sendToChild],
  );

  const closeReviewBridge = useCallback((topic: string) => {
    const socket = subscriptionsRef.current.get(topic);
    if (socket) {
      socket.close();
      subscriptionsRef.current.delete(topic);
    }
  }, []);

  useEffect(() => {
    function onMessage(event: MessageEvent) {
      if (event.source !== iframeRef.current?.contentWindow) return;
      const message = event.data;
      if (!message || typeof message !== "object") return;

      // `hello` from the bridge -> reply with `ready` + manifest.
      if (message.type === "hello") {
        setReady(true);
        sendToChild({
          type: "ready",
          manifest: {
            name: extension.name,
            version: extension.version,
            surfaces: extension.surfaces,
            permissions: extension.permissions,
            surface_id: surface.id,
          },
        });
        return;
      }

      if (message.type === "ui.close") {
        onClose?.();
        return;
      }

      if (message.type === "ui.toast") {
        setToast({
          text: String(message.text ?? ""),
          tone: (message.tone ?? "info") as Toast["tone"],
        });
        return;
      }

      if (message.type === "subscribe" && typeof message.topic === "string") {
        openReviewBridge(message.topic);
        return;
      }

      if (message.type === "unsubscribe" && typeof message.topic === "string") {
        closeReviewBridge(message.topic);
        return;
      }

      // Request / response calls --------------------------------------
      if (message.type !== "call" || typeof message.id !== "number") return;

      const fail = (error: string) => {
        sendToChild({ id: message.id, type: "error", error });
      };
      const ok = (result: unknown) => {
        sendToChild({ id: message.id, type: "result", result });
      };

      const method = message.method;
      const params = message.params ?? {};

      if (method === "tools.call") {
        const capability = String((params as { capability?: unknown }).capability ?? "");
        const args = (params as { arguments?: unknown }).arguments ?? {};
        if (!capability) {
          fail("missing 'capability'");
          return;
        }
        if (!permits(extension.permissions.mcp_tools, capability)) {
          fail(
            `extension '${extension.name}' does not have permission to call '${capability}'`,
          );
          return;
        }
        void fetch(`${origin}/api/tools/${encodeURIComponent(capability)}`, {
          method: "POST",
          headers: { "content-type": "application/json" },
          body: JSON.stringify(args),
        })
          .then(async (response) => {
            const payload = await response.json().catch(() => ({}));
            if (!response.ok) {
              fail(
                typeof (payload as { error?: unknown }).error === "string"
                  ? (payload as { error: string }).error
                  : `control API returned ${response.status}`,
              );
              return;
            }
            ok(payload);
          })
          .catch((err) => {
            fail(err instanceof Error ? err.message : String(err));
          });
        return;
      }

      if (method === "tools.list") {
        void fetch(`${origin}/api/capabilities`)
          .then(async (response) => {
            const payload = await response.json().catch(() => ({}));
            if (!response.ok) {
              fail(`control API returned ${response.status}`);
              return;
            }
            const descriptors =
              ((payload as { descriptors?: Array<{ capability: string }> })
                .descriptors as Array<{ capability: string }>) || [];
            // Filter down to what this extension is permitted to call.
            const allowed = descriptors.filter((d) =>
              permits(extension.permissions.mcp_tools, d.capability),
            );
            ok({ count: allowed.length, capabilities: allowed });
          })
          .catch((err) => {
            fail(err instanceof Error ? err.message : String(err));
          });
        return;
      }

      fail(`unknown method '${method}'`);
    }

    window.addEventListener("message", onMessage);
    return () => {
      window.removeEventListener("message", onMessage);
      subscriptionsRef.current.forEach((socket) => socket.close());
      subscriptionsRef.current.clear();
    };
  }, [
    extension,
    surface.id,
    origin,
    onClose,
    sendToChild,
    openReviewBridge,
    closeReviewBridge,
  ]);

  useEffect(() => {
    if (!toast) return;
    const timer = setTimeout(() => setToast(null), 3000);
    return () => clearTimeout(timer);
  }, [toast]);

  return (
    <section className="liquid-panel relative flex min-h-[600px] flex-col overflow-hidden rounded-[2.75rem] border p-4">
      <div className="flex flex-wrap items-center justify-between gap-3 border-b border-white/10 px-4 pb-3">
        <div>
          <div className="text-[11px] font-semibold uppercase tracking-[0.36em] text-slate-400">
            extension · {extension.name} v{extension.version}
          </div>
          <div className="mt-0.5 text-sm font-semibold text-slate-100">{surface.label}</div>
          {surface.description && (
            <div className="mt-1 text-xs text-slate-400">{surface.description}</div>
          )}
        </div>
        <div className="flex items-center gap-2 text-[10px] uppercase tracking-[0.22em]">
          <span
            className={`rounded-full border px-2 py-0.5 ${
              ready
                ? "border-teal-300/40 bg-teal-500/15 text-teal-200"
                : "border-amber-300/40 bg-amber-500/10 text-amber-200"
            }`}
          >
            {ready ? "connected" : "loading"}
          </span>
          <span className="rounded-full border border-white/10 bg-white/5 px-2 py-0.5 text-slate-400">
            {extension.permissions.mcp_tools.length} tool grants
          </span>
        </div>
      </div>

      <div className="relative mt-3 flex-1">
        {toast && (
          <div
            className={`pointer-events-none absolute right-4 top-4 z-10 rounded-2xl border px-4 py-2 text-xs ${
              toast.tone === "error"
                ? "border-rose-400/40 bg-rose-500/15 text-rose-200"
                : toast.tone === "warn"
                  ? "border-amber-300/40 bg-amber-500/15 text-amber-200"
                  : toast.tone === "success"
                    ? "border-teal-300/40 bg-teal-500/15 text-teal-200"
                    : "border-white/15 bg-white/10 text-slate-200"
            }`}
          >
            {toast.text}
          </div>
        )}
        <iframe
          ref={iframeRef}
          src={entryUrl}
          sandbox="allow-scripts"
          title={`${extension.name}/${surface.id}`}
          className="h-full min-h-[600px] w-full rounded-2xl border border-white/10 bg-white"
        />
      </div>
    </section>
  );
}
