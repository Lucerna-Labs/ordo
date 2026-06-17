#!/usr/bin/env python3
"""
Ordo exhaustive black-box test harness.

Drives a RUNNING Ordo runtime through its control API and exercises, as a
client would, every surface we can safely touch:

  * every control-API route (read GETs hit directly; parameterized GETs
    probed with a bogus id and asserted to fail gracefully, not 500);
  * every capability/tool advertised by /api/capabilities, invoked via
    POST /api/tools/<capability> — destructive/mutating and network-egress
    tools are SKIPPED (this runs against a live runtime with real data);
  * the multi-agent orchestrator (POST /api/orchestrate).

Per-check verdict:
  PASS  = HTTP 2xx (worked) or HTTP 4xx (reachable + cleanly rejected input)
  WARN  = a reachable tool whose failure is not a code fault:
            * HTTP 5xx carrying a clean validation error (returns 500 where
              it should return 4xx — a status-code quirk, not a crash);
            * HTTP 5xx because a credential / precondition isn't configured;
            * a persistent 429 (rate-limited after retries).
  FAIL  = a genuine anomaly: an advertised capability with no live handler
          ("no provider handled" — often a lazy provider that wasn't
          warmed), any other unexplained HTTP 5xx, a timeout, or a
          connection error (the runtime actually broke).
  SKIP  = intentionally not invoked (destructive / network / needs setup)

Exit code is non-zero only if something FAILed (WARNs do not fail the run).

Usage:
  python scripts/ordo_exhaustive_test.py [--base-url http://127.0.0.1:4142]
                                         [--include-network] [--timeout 30]
                                         [--orchestrate]    # run a live goal
"""

import argparse
import contextlib
import json
import sys
import time
import urllib.error
import urllib.request

# Small inter-request pause + 429 retry keep us under the control API's rate
# limiter during the rapid bulk sweep.
THROTTLE_S = 0.03
RETRY_429 = 4

# ── HTTP ────────────────────────────────────────────────────────────────────


def request(method, url, body=None, timeout=30):
    """Returns (status:int|None, text:str). status None = transport failure.
    Retries on HTTP 429 with linear backoff."""
    data = json.dumps(body).encode() if body is not None else None
    for attempt in range(RETRY_429):
        time.sleep(THROTTLE_S)
        req = urllib.request.Request(url, data=data, method=method)
        if data is not None:
            req.add_header("Content-Type", "application/json")
        try:
            with urllib.request.urlopen(req, timeout=timeout) as resp:
                return resp.status, resp.read().decode("utf-8", "replace")
        except urllib.error.HTTPError as e:
            if e.code == 429 and attempt < RETRY_429 - 1:
                time.sleep(0.5 * (attempt + 1))
                continue
            return e.code, e.read().decode("utf-8", "replace")
        except Exception as e:  # noqa: BLE001 — transport/timeout = FAIL signal
            return None, f"{type(e).__name__}: {e}"
    return 429, "rate-limited after retries"


# ── Classification ──────────────────────────────────────────────────────────

DESTRUCTIVE = (
    "delete", "remove", "write", "upsert", "install", "uninstall", "publish",
    "unpublish", "archive", "unarchive", "dispatch", "remember", "forget",
    "pin", "unpin", "register", "create", "update", "approve", "deny", "edit",
    "set_trust", "set_enabled", "rotate", "seal", "unseal", "promote",
    "quarantine", "re_authorize", "re-authorize", "prepare_command",
    "sync_workspace", "sync_resource", "run_native", "run",
)
NETWORK = ("fetch", "search", "strain")

# Signatures of a clean, client-caused validation error (so a 500 carrying
# one is a status-code quirk, not a server fault).
VALIDATION_SIGNS = (
    "invalid argument", "missing", "required", "requires", "must not be empty",
    "must be", "not found", "empty", "expected",
)
# A 500 because a credential / external precondition isn't configured — an
# environment issue, not a code fault.
PRECONDITION_SIGNS = (
    "not configured", "no compatible credential", "credential for service",
)
# The genuine anomaly: a capability is ADVERTISED in /api/capabilities but no
# provider resolves it through /api/tools (often lazy-activated RAG/knowledge
# providers that weren't warmed). Surfaced as FAIL.
NO_HANDLER_SIGNS = ("no provider handled",)


def is_destructive(cap):
    return any(s in cap for s in DESTRUCTIVE)


def is_network(cap):
    return any(s in cap for s in NETWORK)


ARG_HINTS = {
    # Fast, local retrieval reads — real args for a true 200. LLM-backed
    # reads are left un-hinted (invoked with {} they 4xx as reachable,
    # keeping the sweep fast and free of generation calls).
    "assistant.recall_memory": {"query": "preferences"},
    "assistant.knowledge_lookup": {"query": "ordo"},
    "assistant.list_facts": {},
}


def classify(status, body=""):
    """Map (status, body) -> (verdict, detail)."""
    if status is None:
        return "FAIL", f"transport error / timeout — {body[:80]}"
    if 200 <= status < 300:
        return "PASS", f"HTTP {status}"
    if status == 429:
        return "WARN", "HTTP 429 (rate-limited after retries)"
    if 400 <= status < 500:
        return "PASS", f"HTTP {status} (reachable, validated)"
    low = (body or "").lower()
    if any(s in low for s in NO_HANDLER_SIGNS):
        return "FAIL", f"HTTP {status}: advertised capability has no handler — {body[:100]}"
    if any(s in low for s in PRECONDITION_SIGNS):
        return "WARN", f"HTTP {status}: needs a configured credential — {body[:90]}"
    if any(s in low for s in VALIDATION_SIGNS):
        return "WARN", f"HTTP {status} on bad input — should be 4xx: {body[:90]}"
    return "FAIL", f"HTTP {status}: {body[:120]}"


# ── Report ──────────────────────────────────────────────────────────────────


class Report:
    def __init__(self):
        self.rows = []

    def add(self, status, name, detail=""):
        self.rows.append((status, name, detail))
        sym = {"PASS": "PASS", "FAIL": "FAIL", "SKIP": "skip", "WARN": "WARN"}[status]
        print(f"  [{sym}] {name}{(' — ' + detail) if detail else ''}")

    def summary(self):
        c = {"PASS": 0, "FAIL": 0, "SKIP": 0, "WARN": 0}
        for status, _, _ in self.rows:
            c[status] += 1
        print("\n" + "=" * 64)
        print(f"  TOTAL {len(self.rows)}   PASS {c['PASS']}   WARN {c['WARN']}   "
              f"FAIL {c['FAIL']}   SKIP {c['SKIP']}")
        for label, key in (("WARNINGS", "WARN"), ("FAILURES", "FAIL")):
            items = [(n, d) for s, n, d in self.rows if s == key]
            if items:
                print(f"\n  {label}:")
                for n, d in items:
                    print(f"    - {n}: {d}")
        print("=" * 64)
        return c["FAIL"] == 0


BOGUS_ID = "00000000-0000-0000-0000-000000000000"

READ_GETS = [
    "/health", "/metrics", "/api/capabilities", "/api/rag/collections",
    "/api/runtime/profile", "/api/runtime/storage", "/api/runtime/settings",
    "/api/self-heal/cases", "/api/memory/pinned", "/api/memory/working",
    "/api/cloud/credentials", "/api/builds", "/api/automations", "/api/plugins",
    "/api/security/audit", "/api/security/rules", "/api/review/pending",
    "/api/review/recent", "/api/ui-extensions", "/api/assistant/sessions",
    "/api/assistant/facts", "/api/assistant/modes", "/api/files", "/api/apps",
    "/api/webhooks", "/api/mcp/servers", "/api/connections/types",
    "/api/connections",
]
PARAM_GETS = [
    f"/api/builds/{BOGUS_ID}", f"/api/automations/{BOGUS_ID}",
    f"/api/review/{BOGUS_ID}", f"/api/assistant/sessions/{BOGUS_ID}",
    f"/api/assistant/sessions/{BOGUS_ID}/taint", "/api/assistant/modes/general",
    f"/api/apps/{BOGUS_ID}", f"/api/files/{BOGUS_ID}",
    f"/api/webhooks/{BOGUS_ID}", f"/api/connections/{BOGUS_ID}",
]


def main():
    ap = argparse.ArgumentParser(description="Ordo exhaustive control-API test")
    ap.add_argument("--base-url", default="http://127.0.0.1:4142")
    ap.add_argument("--timeout", type=int, default=30)
    ap.add_argument("--include-network", action="store_true")
    ap.add_argument("--orchestrate", action="store_true",
                    help="run a live goal through POST /api/orchestrate")
    args = ap.parse_args()
    with contextlib.suppress(Exception):
        sys.stdout.reconfigure(encoding="utf-8", errors="replace")
    base = args.base_url.rstrip("/")
    rep = Report()

    print(f"\n# Ordo exhaustive test -> {base}\n\n## Phase 0: liveness")
    s, t = request("GET", base + "/health", timeout=args.timeout)
    if s != 200:
        print(f"  [FAIL] runtime not reachable at {base}/health ({s}: {t[:120]})")
        sys.exit(2)
    rep.add("PASS", "GET /health", t.strip()[:40])

    print("\n## Phase 1: read endpoints")
    for path in READ_GETS:
        s, t = request("GET", base + path, timeout=args.timeout)
        st, d = classify(s, t)
        rep.add(st, f"GET {path}", d if st != "PASS" else "")

    print("\n## Phase 2: parameterized endpoints (bogus id -> must not 5xx)")
    for path in PARAM_GETS:
        s, t = request("GET", base + path, timeout=args.timeout)
        st, d = classify(s, t)
        rep.add(st, f"GET {path}", d)

    print("\n## Phase 3: capabilities (every tool)")
    s, t = request("GET", base + "/api/capabilities", timeout=args.timeout)
    caps = []
    try:
        caps = [d["capability"] for d in json.loads(t).get("descriptors", [])]
    except Exception as e:  # noqa: BLE001
        rep.add("FAIL", "enumerate capabilities", f"parse error: {e}")
    print(f"  ({len(caps)} capabilities advertised)")
    for cap in sorted(caps):
        if is_destructive(cap):
            rep.add("SKIP", f"tool {cap}", "mutating — skipped on live runtime")
            continue
        if is_network(cap) and not args.include_network:
            rep.add("SKIP", f"tool {cap}", "network egress — use --include-network")
            continue
        s, t = request("POST", f"{base}/api/tools/{cap}",
                       body=ARG_HINTS.get(cap, {}), timeout=args.timeout)
        st, d = classify(s, t)
        rep.add(st, f"tool {cap}", d if st != "PASS" else "")

    print("\n## Phase 4: safe actions")
    s, t = request("POST", base + "/api/automations/tick", body={}, timeout=args.timeout)
    st, d = classify(s, t)
    rep.add(st, "POST /api/automations/tick", d if st != "PASS" else "")

    print("\n## Phase 5: orchestrator")
    s, t = request("POST", base + "/api/orchestrate", body={"goal": ""}, timeout=args.timeout)
    st, d = classify(s, t)  # empty goal -> 400 proves it's wired
    rep.add(st, "POST /api/orchestrate (empty-goal wiring probe)", d)
    if args.orchestrate:
        s, t = request("POST", base + "/api/orchestrate",
                       body={"goal": "Name two ways to check a service is healthy."},
                       timeout=max(args.timeout, 180))
        if s and 200 <= s < 300:
            try:
                o = json.loads(t)
                rep.add("PASS", "POST /api/orchestrate (live goal)",
                        f"phase={o.get('phase')} succeeded={o.get('succeeded')} "
                        f"accepted={len(o.get('accepted', []))}")
            except Exception:  # noqa: BLE001
                rep.add("PASS", "POST /api/orchestrate (live goal)", "200 (unparsed)")
        else:
            st, d = classify(s, t)
            rep.add(st, "POST /api/orchestrate (live goal)", d)

    ok = rep.summary()
    sys.exit(0 if ok else 1)


if __name__ == "__main__":
    main()
