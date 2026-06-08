import React, { useState } from "react";
import { Log } from "../hooks/useSystemState";

export function DeveloperDrawer({ logs }: { logs: Log[] }) {
  const [isOpen, setIsOpen] = useState(false);
  return (
    <div
      className={`liquid-panel pointer-events-auto fixed bottom-4 left-[7.5rem] right-4 z-50 overflow-hidden rounded-[1.9rem] border transition-all duration-500 ${
        isOpen ? "h-[22rem]" : "h-14"
      }`}
    >
      <button
        onClick={() => setIsOpen((value) => !value)}
        className="flex h-14 w-full items-center justify-center gap-3 text-[11px] font-semibold uppercase tracking-[0.34em] text-slate-400 transition hover:text-teal-200"
      >
        <span className={`transition-transform ${isOpen ? "rotate-180" : ""}`}>^</span>
        Engine room console
      </button>
      <div className="h-[calc(100%-3.5rem)] overflow-y-auto px-6 pb-5 font-mono text-[12px]">
        {logs.length === 0 ? (
          <div className="rounded-2xl border border-white/10 bg-black/20 p-4 text-slate-500">
            Waiting for bus telemetry...
          </div>
        ) : (
          <div className="space-y-2">
            {logs.map((log) => (
              <div
                key={log.id}
                className="grid gap-2 rounded-2xl border border-white/8 bg-black/25 px-4 py-3 md:grid-cols-[auto_auto_1fr]"
              >
                <span className="text-slate-600">
                  [{new Date(log.timestamp).toLocaleTimeString()}]
                </span>
                <span
                  className={`font-semibold ${
                    log.level === "ERROR"
                      ? "text-red-300"
                      : log.level === "WARN"
                        ? "text-amber-200"
                        : "text-teal-200"
                  }`}
                >
                  [{log.source}]
                </span>
                <span className="text-slate-300">{log.message}</span>
              </div>
            ))}
          </div>
        )}
      </div>
    </div>
  );
}
