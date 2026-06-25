import { useState, useEffect, useCallback } from "react";

interface UpdateInfo {
  current: string;
  latest: string | null;
  latest_tag: string | null;
  update_available: boolean;
  release_url: string | null;
  release_notes: string | null;
  platform_asset: { name: string; url: string; size: number } | null;
  error?: string;
}

interface ApplyResult {
  updated: boolean;
  old_version?: string;
  new_version?: string;
  message: string;
}

/**
 * Update banner — checks GitHub for a newer Ordo release on mount.
 * If one exists, shows a non-intrusive banner with a one-click "Update Now" button.
 * The update runs on the server side (git pull + rebuild, or .deb download + install).
 */
export default function UpdateBanner() {
  const [info, setInfo] = useState<UpdateInfo | null>(null);
  const [loading, setLoading] = useState(false);
  const [applying, setApplying] = useState(false);
  const [result, setResult] = useState<ApplyResult | null>(null);
  const [dismissed, setDismissed] = useState(false);

  const check = useCallback(async () => {
    setLoading(true);
    try {
      const res = await fetch("/api/update/check");
      const data = await res.json();
      setInfo(data);
    } catch {
      setInfo(null);
    } finally {
      setLoading(false);
    }
  }, []);

  useEffect(() => {
    // Check on mount, but with a small delay so the Studio loads first.
    const timer = setTimeout(check, 2000);
    return () => clearTimeout(timer);
  }, [check]);

  const applyUpdate = async () => {
    setApplying(true);
    setResult(null);
    try {
      const res = await fetch("/api/update/apply", { method: "POST" });
      const data = await res.json();
      setResult(data);
      if (data.updated) {
        setInfo(null);
      }
    } catch (e) {
      setResult({
        updated: false,
        message: `Update failed: ${e instanceof Error ? e.message : "network error"}`,
      });
    } finally {
      setApplying(false);
    }
  };

  if (loading || !info || dismissed) return null;

  // Show result message (success or failure) after applying
  if (result) {
    return (
      <div
        style={{
          position: "fixed",
          bottom: 16,
          right: 16,
          maxWidth: 380,
          padding: "14px 18px",
          borderRadius: 10,
          background: result.updated ? "#1a3a1a" : "#3a1a1a",
          border: `1px solid ${result.updated ? "#2a6a2a" : "#6a2a2a"}`,
          color: "#e0e0e0",
          fontSize: 13,
          zIndex: 10000,
          boxShadow: "0 4px 12px rgba(0,0,0,0.4)",
        }}
      >
        <div style={{ fontWeight: 600, marginBottom: 4 }}>
          {result.updated ? "✓ Update Applied" : "✗ Update Failed"}
        </div>
        <div style={{ opacity: 0.85, lineHeight: 1.4 }}>{result.message}</div>
        {result.updated && (
          <div style={{ marginTop: 8, display: "flex", gap: 8 }}>
            <button
              onClick={() => window.location.reload()}
              style={{
                padding: "4px 14px",
                borderRadius: 6,
                border: "none",
                background: "#2a6a2a",
                color: "#fff",
                cursor: "pointer",
                fontSize: 12,
                fontWeight: 600,
              }}
            >
              Restart Now
            </button>
            <button
              onClick={() => {
                setResult(null);
                setDismissed(true);
              }}
              style={{
                padding: "4px 14px",
                borderRadius: 6,
                border: "1px solid #555",
                background: "transparent",
                color: "#aaa",
                cursor: "pointer",
                fontSize: 12,
              }}
            >
              Later
            </button>
          </div>
        )}
        {!result.updated && (
          <button
            onClick={() => setResult(null)}
            style={{
              marginTop: 8,
              padding: "4px 14px",
              borderRadius: 6,
              border: "1px solid #555",
              background: "transparent",
              color: "#aaa",
              cursor: "pointer",
              fontSize: 12,
            }}
          >
            Close
          </button>
        )}
      </div>
    );
  }

  // No update available
  if (!info.update_available) return null;

  // Update available — show banner
  return (
    <div
      style={{
        position: "fixed",
        bottom: 16,
        right: 16,
        maxWidth: 380,
        padding: "14px 18px",
        borderRadius: 10,
        background: "#1a2a3a",
        border: "1px solid #2a5a8a",
        color: "#e0e0e0",
        fontSize: 13,
        zIndex: 10000,
        boxShadow: "0 4px 12px rgba(0,0,0,0.4)",
      }}
    >
      <div style={{ fontWeight: 600, marginBottom: 6 }}>
        ✦ Update Available — v{info.latest}
      </div>
      <div style={{ opacity: 0.7, marginBottom: 10 }}>
        You're on v{info.current}.{" "}
        {info.platform_asset
          ? `Will download and install ${info.platform_asset.name}.`
          : "Will run git pull + rebuild."}
      </div>
      {info.release_notes && (
        <div
          style={{
            opacity: 0.6,
            marginBottom: 10,
            maxHeight: 80,
            overflow: "hidden",
            lineHeight: 1.35,
            whiteSpace: "pre-wrap",
          }}
        >
          {info.release_notes.slice(0, 200)}
          {info.release_notes.length > 200 ? "…" : ""}
        </div>
      )}
      <div style={{ display: "flex", gap: 8 }}>
        <button
          onClick={applyUpdate}
          disabled={applying}
          style={{
            padding: "6px 16px",
            borderRadius: 6,
            border: "none",
            background: applying ? "#333" : "#2a6a9a",
            color: applying ? "#888" : "#fff",
            cursor: applying ? "wait" : "pointer",
            fontSize: 12,
            fontWeight: 600,
          }}
        >
          {applying ? "Updating…" : "Update Now"}
        </button>
        {info.release_url && (
          <a
            href={info.release_url}
            target="_blank"
            rel="noopener noreferrer"
            style={{
              padding: "6px 16px",
              borderRadius: 6,
              border: "1px solid #446",
              color: "#8ac",
              fontSize: 12,
              textDecoration: "none",
              display: "inline-flex",
              alignItems: "center",
            }}
          >
            Release Notes
          </a>
        )}
        <button
          onClick={() => setDismissed(true)}
          style={{
            padding: "6px 12px",
            borderRadius: 6,
            border: "1px solid #444",
            background: "transparent",
            color: "#888",
            cursor: "pointer",
            fontSize: 12,
          }}
        >
          Dismiss
        </button>
      </div>
    </div>
  );
}
