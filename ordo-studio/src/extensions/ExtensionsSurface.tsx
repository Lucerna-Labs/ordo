import { useState } from "react";

import { ExtensionHost } from "./ExtensionHost";
import { useUiExtensions, type UiExtension, type UiExtensionSurface } from "./useUiExtensions";

interface SurfaceEntry {
  extension: UiExtension;
  surface: UiExtensionSurface;
}

function surfaceKey(entry: SurfaceEntry): string {
  return `${entry.extension.name}::${entry.surface.id}`;
}

/**
 * The "Extensions" tab. Lists enabled UI extensions discovered by the
 * control API (`/api/ui-extensions`) and mounts the selected one through
 * `ExtensionHost` (a sandboxed iframe + postMessage bridge). The backend
 * for this has always been live; this surface is what makes it visible in
 * the studio.
 */
export function ExtensionsSurface() {
  const [state, reload] = useUiExtensions();
  const [selectedKey, setSelectedKey] = useState<string | null>(null);

  if (state.status === "loading") {
    return (
      <div className="flex min-h-[400px] items-center justify-center text-sm text-slate-400">
        Loading UI extensions…
      </div>
    );
  }

  if (state.status === "error") {
    return (
      <div className="flex min-h-[400px] flex-col items-center justify-center gap-3 text-center">
        <div className="text-sm text-rose-300">Couldn’t load UI extensions: {state.message}</div>
        <div className="text-xs text-slate-500">The control API at 127.0.0.1:4141 must be running.</div>
        <button
          onClick={reload}
          className="rounded-full border border-white/15 bg-white/5 px-4 py-1.5 text-xs text-slate-200 hover:bg-white/10"
        >
          Retry
        </button>
      </div>
    );
  }

  const entries: SurfaceEntry[] = state.extensions.flatMap((extension) =>
    extension.surfaces.map((surface) => ({ extension, surface })),
  );

  if (entries.length === 0) {
    return (
      <div className="flex min-h-[400px] flex-col items-center justify-center gap-3 text-center">
        <div className="text-sm font-semibold text-slate-200">No UI extensions installed</div>
        <div className="max-w-md text-xs text-slate-400">
          Drop an extension into{" "}
          <code className="text-slate-300">user-files/ui-extensions/&lt;name&gt;/</code> with a{" "}
          <code className="text-slate-300">ui.json</code> manifest, then reload.
        </div>
        {state.errors.length > 0 && (
          <ul className="mt-1 max-w-md text-left text-[11px] text-amber-300">
            {state.errors.map((err) => (
              <li key={err.manifest_path}>
                {err.manifest_path}: {err.error}
              </li>
            ))}
          </ul>
        )}
        <button
          onClick={reload}
          className="rounded-full border border-white/15 bg-white/5 px-4 py-1.5 text-xs text-slate-200 hover:bg-white/10"
        >
          Reload
        </button>
      </div>
    );
  }

  const active = entries.find((entry) => surfaceKey(entry) === selectedKey) ?? entries[0];

  return (
    <div className="flex flex-col gap-4">
      <div className="flex flex-wrap items-center justify-between gap-3">
        <div>
          <div className="text-[11px] font-semibold uppercase tracking-[0.36em] text-slate-400">
            UI Extensions
          </div>
          <div className="mt-0.5 text-xs text-slate-500">
            {entries.length} surface{entries.length === 1 ? "" : "s"} from {state.extensions.length}{" "}
            extension{state.extensions.length === 1 ? "" : "s"}
          </div>
        </div>
        <button
          onClick={reload}
          className="rounded-full border border-white/15 bg-white/5 px-3 py-1 text-[11px] uppercase tracking-[0.22em] text-slate-300 hover:bg-white/10"
        >
          Reload
        </button>
      </div>

      {entries.length > 1 && (
        <nav className="flex flex-wrap gap-2">
          {entries.map((entry) => {
            const key = surfaceKey(entry);
            const isActive = key === surfaceKey(active);
            return (
              <button
                key={key}
                onClick={() => setSelectedKey(key)}
                aria-current={isActive ? "page" : undefined}
                className={`rounded-full border px-3 py-1.5 text-xs transition ${
                  isActive
                    ? "border-teal-300/40 bg-teal-500/15 text-teal-100"
                    : "border-white/10 bg-white/5 text-slate-300 hover:bg-white/10"
                }`}
              >
                {entry.surface.label}
                <span className="ml-1.5 text-[10px] text-slate-500">{entry.extension.name}</span>
              </button>
            );
          })}
        </nav>
      )}

      <ExtensionHost
        key={surfaceKey(active)}
        extension={active.extension}
        surface={active.surface}
        origin={state.origin}
      />
    </div>
  );
}
