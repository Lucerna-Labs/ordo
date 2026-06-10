#!/usr/bin/env python3
"""Ordo FULL test harness — one self-launched runtime, every subsystem, one verdict.

This is the comprehensive companion to the three focused harnesses. It launches
ONE runtime on a test port (default 127.0.0.1:4142) with an isolated temp DB,
the avatar driver enabled, and the orchestrator enabled, stands up a single mock
provider that speaks the OpenAI `/audio/speech`, `/audio/transcriptions`,
`/chat/completions` and MiniMax `/t2a_v2` contracts, then exercises:

  liveness/static, the read-GET + parameterized-GET sweep, assistant
  sessions+turn (driven END TO END through the mock LLM), facts/recall, the
  MODES contract (the protected `avatar` mode, user-mode CRUD, the avatar-brain
  bind round-trip that must NOT lose manifest fields), the avatar assets/SSE/
  speak/visemes, the provider-agnostic voice dispatch (TTS + STT, happy +
  adversarial), cloud credentials (CRUD + redaction + the empty-secret-preserve
  regression), apps lifecycle + deployments, files (text + binary), webhooks
  (real end-to-end fire), connections, MCP (real WASM install→invoke→quarantine
  →uninstall), self-heal/memory/settings/review/plugins/automations/builds/
  ui-extensions, RAG, the capability sweep, the orchestrator, and a final
  adversarial / no-500 / concurrency storm — then a panic/clean-exit canary.

It REUSES the proven scaffolding in scripts/ordo_avatar_test.py (mock provider,
HTTP client, SSE reader, credential lifecycle, launch/teardown, the :4141 refusal
guard) by importing it as a library, and folds in the webhook receiver + echo
WASM from scripts/ordo_functional_test.py. Nothing here ever touches :4141.

Usage:
  python scripts/ordo_full_test.py [--base-url http://127.0.0.1:4142]
                                   [--no-launch] [--keep] [--bin PATH]
                                   [--include-network]
"""

import argparse
import json
import os
import sys
import threading
import time
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer

# Reuse the proven avatar/voice harness as a library. Its module-level code is
# pure definitions (constants, helpers, the mock handler, lifecycle) — main() is
# guarded — so importing it has no side effects beyond making those available.
sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import ordo_avatar_test as av  # noqa: E402

try:
    sys.stdout.reconfigure(encoding="utf-8")
    sys.stderr.reconfigure(encoding="utf-8")
except Exception:
    pass

BOGUS = "00000000-0000-0000-0000-000000000000"
# Deterministic reply the mock LLM returns — asserted byte-for-byte in the turn.
MOCK_CHAT_REPLY = "Mock model reply for run " + av.RUNID

# A 142-byte WASM "echo" module (copied from ordo_functional_test.py): memory +
# alloc + a `hello` entry returning its input unchanged. Proves a REAL MCP
# install + sandboxed invoke with no external build step.
ECHO_WASM_B64 = (
    "AGFzbQEAAAABDAJgAX8Bf2ACf38BfgMDAgABBQMBAAEGBwF/AUGACAsHGgMGbWVtb3J5AgAFYWxs"
    "b2MAAAVoZWxsbwABCiACEQEBfyMAIQEjACAAaiQAIAELDAAgAK1CIIYgAa2ECwAlBG5hbWUCFQIA"
    "AgABbgEBcAECAANpbnABA2xlbgcHAQAEYnVtcA=="
)

# Keys that must never carry a *real* secret in any GET/LIST response.
SECRET_KEYS = {
    "secret", "password", "api_key", "secret_key", "private_key",
    "client_secret", "access_token", "refresh_token",
}
# Values that are explicit redaction placeholders — NOT a leak. Different
# subsystems redact differently (cloud creds drop the key / set null; webhooks
# substitute the literal "<redacted>"); both are safe.
REDACTION_MARKERS = {"<redacted>", "redacted", "[redacted]", "***", "*****", "********"}


def is_redaction_marker(v):
    s = v.strip().lower()
    return s in REDACTION_MARKERS or (len(s) >= 3 and set(s) == {"*"})


# ─────────────────────────────────────────────────────────────────────
# Thin helpers layered over the imported primitives (all results funnel
# into av's PASS/WARN/FAIL counters → one unified summary).
# ─────────────────────────────────────────────────────────────────────

def okj(status):
    return status is not None and 200 <= status < 300


def no500(status):
    return status is not None and not (500 <= status < 600)


def jreq(method, path, body=None):
    """(status, parsed_json_or_text) — discards headers/bytes for terse CRUD."""
    s, _h, raw, p = av.req(method, path, body)
    if p is None and raw:
        try:
            p = raw.decode("utf-8", "replace")
        except Exception:
            p = None
    return s, p


def jget(path):
    return jreq("GET", path)


def jpost(path, body):
    return jreq("POST", path, body)


def idof(p):
    if not isinstance(p, dict):
        return None
    if isinstance(p.get("id"), str):
        return p["id"]
    for k in ("app", "file", "subscription", "connection", "deployment",
              "build", "mode", "fact", "session", "turn", "credential",
              "automation"):
        v = p.get(k)
        if isinstance(v, dict) and isinstance(v.get("id"), str):
            return v["id"]
    return None


def classify(name, status, body, warn_5xx_substrings=()):
    """2xx→PASS, 4xx→PASS (reachable+validated), 429→WARN, 5xx→FAIL
    (unless body carries an allowed substring → WARN), None→FAIL."""
    b = body
    if isinstance(b, (bytes, bytearray)):
        b = b.decode("utf-8", "replace")
    b = str(b).lower()
    if status is None:
        return av.rec("FAIL", name, "transport error / no response")
    if 200 <= status < 300:
        return av.rec("PASS", name, f"HTTP {status}")
    if status == 429:
        return av.rec("WARN", name, "rate-limited")
    if 400 <= status < 500:
        return av.rec("PASS", name, f"HTTP {status} (reachable+validated)")
    if any(sig in b for sig in warn_5xx_substrings):
        return av.rec("WARN", name, f"HTTP {status}: {b[:70]}")
    return av.rec("FAIL", name, f"HTTP {status}: {b[:70]}")


def assert_no_secret_leak(name, parsed):
    leaks = []

    def walk(o, path=""):
        if isinstance(o, dict):
            for k, v in o.items():
                kp = f"{path}.{k}"
                if (k.lower() in SECRET_KEYS and isinstance(v, str) and v.strip()
                        and not is_redaction_marker(v)):
                    leaks.append(kp.lstrip("."))
                walk(v, kp)
        elif isinstance(o, list):
            for i, v in enumerate(o):
                walk(v, f"{path}[{i}]")

    walk(parsed)
    av.check(f"no secret leak: {name}", not leaks,
             ("LEAKED " + ",".join(leaks)) if leaks else "redacted")


# ─────────────────────────────────────────────────────────────────────
# Mock provider — the avatar harness's MockHandler + a NEW /chat/completions
# arm. We only read the body in the chat branch; everything else delegates to
# the proven handler verbatim (records into the same av.MOCK).
# ─────────────────────────────────────────────────────────────────────

class FullMockHandler(av.MockHandler):
    def do_POST(self):
        path = self.path.split("?", 1)[0]
        if path.endswith("/chat/completions"):
            parts = [p for p in path.split("/") if p]
            mode = parts[0] if parts else "ok"
            length = int(self.headers.get("Content-Length", "0") or "0")
            raw = self.rfile.read(length) if length else b""
            try:
                payload = json.loads(raw.decode())
            except Exception:
                payload = None
            av.MOCK.record({"kind": "chat", "mode": mode,
                            "auth": self.headers.get("Authorization", ""),
                            "payload": payload})
            if mode == "err500":
                return self._send(500, b'{"error":"mock 500"}', "application/json")
            if mode == "err401":
                return self._send(401, b'{"error":{"message":"Invalid authentication"}}',
                                  "application/json")
            if mode == "badjson":
                return self._send(200, b"not-json-at-all", "application/json")
            model = (payload or {}).get("model") or "mock-model"
            body = json.dumps({
                "id": "chatcmpl-mock", "object": "chat.completion", "model": model,
                "choices": [{"index": 0, "finish_reason": "stop",
                             "message": {"role": "assistant", "content": MOCK_CHAT_REPLY}}],
                "usage": {"prompt_tokens": 1, "completion_tokens": 1, "total_tokens": 2},
            }).encode()
            return self._send(200, body, "application/json")
        return super().do_POST()


def start_full_mock():
    srv = ThreadingHTTPServer(("127.0.0.1", 0), FullMockHandler)
    threading.Thread(target=srv.serve_forever, daemon=True).start()
    return srv, srv.server_address[1]


# ─────────────────────────────────────────────────────────────────────
# Webhook receiver (captures headers too, for the signature assertion)
# ─────────────────────────────────────────────────────────────────────

_RECV = []  # list of (headers_lower_dict, body_str)


class _WHandler(BaseHTTPRequestHandler):
    def do_POST(self):
        length = int(self.headers.get("Content-Length", 0) or 0)
        body = self.rfile.read(length).decode("utf-8", "replace") if length else ""
        _RECV.append(({k.lower(): v for k, v in self.headers.items()}, body))
        self.send_response(200)
        self.end_headers()
        self.wfile.write(b"ok")

    def log_message(self, *a):
        pass


def start_receiver():
    srv = ThreadingHTTPServer(("127.0.0.1", 0), _WHandler)
    threading.Thread(target=srv.serve_forever, daemon=True).start()
    return srv, srv.server_address[1]


# ─────────────────────────────────────────────────────────────────────
# GROUP 1 — liveness & static surfaces
# ─────────────────────────────────────────────────────────────────────

def g1_liveness():
    print("\n## Group 1 — liveness & static surfaces")
    s, _h, raw, p = av.req("GET", "/health")
    av.check("health 200", s == 200, f"HTTP {s}")
    s, h, raw, _ = av.req("GET", "/metrics")
    txt = (raw or b"").decode("utf-8", "replace")
    av.check("metrics 200 + prometheus tokens", s == 200 and ("# HELP" in txt or "# TYPE" in txt),
             f"HTTP {s}")
    s, h, raw, _ = av.req("GET", "/")
    av.check("dashboard / 200 html", s == 200 and "text/html" in h.get("content-type", ""),
             f"HTTP {s}")
    s, _h, _raw, p = av.req("GET", "/api/capabilities")
    descriptors = []
    if isinstance(p, dict):
        descriptors = p.get("descriptors") or []
        av.check("capabilities: lanes+descriptors lists, descriptors>0",
                 isinstance(p.get("lanes"), list) and isinstance(descriptors, list) and len(descriptors) > 0,
                 f"HTTP {s} descriptors={len(descriptors)}")
    else:
        av.check("capabilities 200 json", okj(s), f"HTTP {s}")
    return descriptors


# ─────────────────────────────────────────────────────────────────────
# GROUP 2 / 3 — read-GET sweep + parameterized GETs (must never 5xx)
# ─────────────────────────────────────────────────────────────────────

READ_GETS = [
    "/api/rag/collections", "/api/runtime/profile", "/api/runtime/storage",
    "/api/runtime/settings", "/api/self-heal/cases", "/api/memory/pinned",
    "/api/memory/working", "/api/cloud/credentials", "/api/builds",
    "/api/automations", "/api/plugins", "/api/security/audit",
    "/api/security/rules", "/api/review/pending", "/api/review/recent",
    "/api/ui-extensions", "/api/assistant/sessions", "/api/assistant/facts",
    "/api/assistant/modes", "/api/files", "/api/apps", "/api/webhooks",
    "/api/mcp/servers", "/api/connections/types", "/api/connections",
    "/api/rag/preview?query=test",
]

PARAM_GETS = [
    f"/api/builds/{BOGUS}", f"/api/automations/{BOGUS}", f"/api/review/{BOGUS}",
    f"/api/assistant/sessions/{BOGUS}", f"/api/assistant/sessions/{BOGUS}/taint",
    "/api/assistant/modes/general", f"/api/apps/{BOGUS}", f"/api/files/{BOGUS}",
    f"/api/webhooks/{BOGUS}", f"/api/connections/{BOGUS}",
]


def g23_get_sweep():
    print("\n## Group 2 — read-GET sweep (classify; 5xx = FAIL)")
    for path in READ_GETS:
        s, _h, raw, _ = av.req("GET", path)
        classify(f"GET {path}", s, raw)
    print("\n## Group 3 — parameterized GETs (bogus id must not 5xx)")
    for path in PARAM_GETS:
        s, _h, raw, _ = av.req("GET", path)
        classify(f"GET {path}", s, raw)
    # find_binary: happy + path-traversal reject
    s, _h, raw, p = av.req("GET", "/api/system/find_binary?name=ordo-mcp.exe")
    av.check("find_binary(name) no-500", no500(s), f"HTTP {s}")
    s, _h, raw, _ = av.req("GET", "/api/system/find_binary?name=../../etc/passwd")
    body = (raw or b"").decode("utf-8", "replace").lower()
    av.check("find_binary path-traversal -> 400", s == 400 and "separator" in body,
             f"HTTP {s} {body[:60]}")


# ─────────────────────────────────────────────────────────────────────
# GROUP 4 — assistant sessions + turn (driven through the mock LLM)
# ─────────────────────────────────────────────────────────────────────

def g4_turn(mock_port, launched):
    print("\n## Group 4 — assistant sessions + turn (mock chat)")
    s, p = jpost("/api/assistant/sessions", {"title": f"full-test {av.RUNID}"})
    sid = idof(p)
    av.check("sessions.create 200 + id", okj(s) and bool(sid), f"HTTP {s} id={sid}")
    s, p = jget("/api/assistant/sessions")
    sessions = (p or {}).get("sessions", []) if isinstance(p, dict) else []
    av.check("sessions.list shows created", okj(s) and any(idof(x) == sid or x.get("id") == sid
             for x in sessions if isinstance(x, dict)), f"HTTP {s}")
    if sid:
        s, _ = jget(f"/api/assistant/sessions/{sid}")
        av.check("sessions.get round-trips", okj(s), f"HTTP {s}")
        s, _ = jget(f"/api/assistant/sessions/{sid}/taint")
        av.check("sessions.taint no-500", no500(s), f"HTTP {s}")

    # The turn drives the mock LLM. Isolate credentials so candidate-fallback
    # can't mask via a leftover real provider; under --no-launch with foreign
    # creds, skip (can't guarantee the mock is the one hit).
    av.purge_creds()
    can_turn = sid and (launched or not av.foreign_creds_present())
    if not can_turn:
        av.rec("WARN", "turn(mock LLM)", "skipped — foreign creds present under --no-launch")
    else:
        svc = f"mockchat{av.RUNID}"
        av.register_cred(svc, f"http://127.0.0.1:{mock_port}/ok/v1", {"model": "mock-model"})
        before = av.MOCK.count("chat")
        s, _h, raw, p = av.req("POST", "/api/assistant/turn", {
            "session_id": sid, "user_message": "Say hello to the test.",
            "credential": svc, "use_tools": False, "use_rag": False, "use_memory": False,
        }, timeout=40)
        reply = ""
        if isinstance(p, dict):
            reply = ((p.get("turn") or {}).get("assistant_response") or "")
        av.check("turn no-500", no500(s), f"HTTP {s}")
        hit = av.MOCK.count("chat") > before
        if okj(s) and hit and MOCK_CHAT_REPLY in reply:
            av.rec("PASS", "turn drives mock LLM (reply byte-exact)", f"reply={reply[:48]!r}")
        elif okj(s) and hit:
            av.rec("WARN", "turn hit mock LLM but reply text differs", f"reply={reply[:70]!r}")
        elif okj(s):
            av.rec("WARN", "turn 200 but mock /chat not hit", "assistant LLM path may differ")
        else:
            av.rec("FAIL", "turn against mock LLM", f"HTTP {s} body={str(p)[:140]}")
        last = av.MOCK.last("chat")
        if last and hit:
            av.check("turn sent exact bearer secret on the wire",
                     last.get("auth") == f"Bearer {av.secret_for(svc)}", last.get("auth"))
        av.delete_cred(svc)

    # lifecycle + adversarial (no-500)
    if sid:
        s, _ = jpost(f"/api/assistant/sessions/{sid}/cancel", {})
        av.check("sessions.cancel no-500", no500(s), f"HTTP {s}")
        s, _ = jpost(f"/api/assistant/sessions/{sid}/taint/clear", {})
        av.check("sessions.taint/clear no-500", no500(s), f"HTTP {s}")
        events = av.read_sse(f"/api/assistant/sessions/{sid}/stream", 2.0, stop_after_events=1)
        got = [e for e in events if e[0] != "_error"]
        av.check("sessions.stream emits an event", len(got) >= 1,
                 f"{len(got)} events", warn_only=True)
    for bad in ({}, {"session_id": "not-a-uuid", "user_message": ""}):
        s, _h, raw, _ = av.req("POST", "/api/assistant/turn", bad)
        av.check(f"turn adversarial {bad} -> 4xx no-500",
                 s is not None and 400 <= s < 500, f"HTTP {s}")


# ─────────────────────────────────────────────────────────────────────
# GROUP 5 — facts + recall
# ─────────────────────────────────────────────────────────────────────

def g5_facts():
    print("\n## Group 5 — assistant facts + recall")
    s, p = jpost("/api/assistant/facts", {"subject": f"test-{av.RUNID}",
                                          "content": "the sky is blue"})
    fid = idof(p)
    av.check("facts.add no-500", no500(s), f"HTTP {s}")
    s, _ = jget("/api/assistant/facts")
    av.check("facts.list 200", okj(s), f"HTTP {s}")
    # recall: a no-embedder runtime surfaces an embedding 500 by design → WARN
    s, _h, raw, _ = av.req("POST", "/api/assistant/recall", {"query": "sky", "top_k": 5})
    classify("recall", s, raw, warn_5xx_substrings=("embed", "embedding"))
    for tk in (0, -1):
        s, _h, _raw, _ = av.req("POST", "/api/assistant/recall", {"query": "x", "top_k": tk})
        av.check(f"recall top_k={tk} no-500", no500(s), f"HTTP {s}")
    s, _h, _raw, _ = av.req("DELETE", f"/api/assistant/facts/{fid or BOGUS}")
    av.check("facts.delete no-500", no500(s), f"HTTP {s}")


# ─────────────────────────────────────────────────────────────────────
# GROUP 6 — modes (avatar contract + CRUD + brain-bind round-trip)
# The avatar force-delete is DEAD LAST — irreversible for the process lifetime.
# ─────────────────────────────────────────────────────────────────────

def g6_modes(mock_port, launched):
    print("\n## Group 6 — modes (avatar contract + CRUD + brain-bind)")
    s, p = jget("/api/assistant/modes")
    modes = (p or {}).get("modes", []) if isinstance(p, dict) else []
    count = (p or {}).get("count", len(modes)) if isinstance(p, dict) else 0
    av.check("modes.list 200 + count>=8", okj(s) and count >= 8, f"HTTP {s} count={count}")
    avatar = next((m for m in modes if isinstance(m, dict) and m.get("id") == "avatar"), None)
    av.check("avatar mode present + protected + label",
             bool(avatar) and avatar.get("protected") is True and avatar.get("label") == "Avatar",
             str(avatar.get("label") if avatar else None))

    # 6.2 full manifest — stash for the brain-bind PATCH (re-sent VERBATIM)
    s, p = jget("/api/assistant/modes/avatar")
    man = p if isinstance(p, dict) else {}
    pb = man.get("planner_bias", [])
    av.check("avatar planner_bias: 3 entries incl. 'spoken aloud'",
             len(pb) == 3 and any("spoken aloud" in str(x) for x in pb), f"len={len(pb)}")
    av.check("avatar persona includes spoken_companion",
             "spoken_companion" in man.get("persona", []), str(man.get("persona")))
    av.check("avatar memory_scope = global + mode:avatar",
             {"global", "mode:avatar"}.issubset(set(man.get("memory_scope", []))),
             str(man.get("memory_scope")))
    av.check("avatar tool lanes include web.",
             "web." in man.get("allowed_tool_lanes", []), str(man.get("allowed_tool_lanes")))
    stash = dict(man)

    # 6.3/6.4/6.5 user mode CRUD
    s, p = jpost("/api/assistant/modes", {"name": f"Full Test Mode {av.RUNID}"})
    um = p if isinstance(p, dict) else {}
    uid = um.get("id")
    av.check("modes.create user mode (unprotected, has lanes)",
             okj(s) and bool(uid) and um.get("protected") is False
             and isinstance(um.get("allowed_tool_lanes"), list), f"HTTP {s} id={uid}")
    if uid:
        s, p = jget(f"/api/assistant/modes/{uid}")
        av.check("modes.get(user) round-trips", okj(s) and (p or {}).get("id") == uid, f"HTTP {s}")
        body = dict(p) if isinstance(p, dict) else {}
        body["label"] = f"Renamed {av.RUNID}"
        s, up = jreq("PATCH", f"/api/assistant/modes/{uid}", body)
        av.check("modes.patch(user) label updated (full-manifest contract)",
                 okj(s) and (up or {}).get("label") == f"Renamed {av.RUNID}",
                 f"HTTP {s} {str(up)[:80]}")

    # 6.6 avatar-brain bind round-trip (REGRESSION GUARD: no manifest field loss)
    brain = "avatar-brain"
    skip_brain = False
    if not launched:
        s, p = jget("/api/cloud/credentials")
        existing = {c.get("service") for c in (p or {}).get("credentials", [])} if isinstance(p, dict) else set()
        if brain in existing:
            skip_brain = True
            av.rec("WARN", "avatar-brain bind", "real avatar-brain cred exists under --no-launch — skipped")
    if not skip_brain:
        s, _h, _raw, _ = av.req("POST", "/api/cloud/credentials", {
            "service": brain, "label": "Avatar brain", "auth_style": "bearer",
            "secret": "local", "base_url": f"http://127.0.0.1:{mock_port}/ok/v1",
            "extras": {"model": "llama2", "avatar_brain": "true", "kind": "local"},
        })
        av.check("avatar-brain cred upsert", okj(s), f"HTTP {s}")
        s, p = jget("/api/cloud/credentials")
        creds = (p or {}).get("credentials", []) if isinstance(p, dict) else []
        bc = next((c for c in creds if c.get("service") == brain), None)
        av.check("avatar-brain listed", bool(bc), "")
        if bc:
            av.check("avatar-brain: redacted + has_secret + extras.model",
                     (bc.get("secret") in (None, "")) and bc.get("has_secret") is True
                     and (bc.get("extras") or {}).get("model") == "llama2",
                     str(bc.get("extras")))
        # PATCH the avatar mode's default_credential — FULL manifest re-sent verbatim
        patch = dict(stash)
        patch["default_credential"] = brain
        s, up = jreq("PATCH", "/api/assistant/modes/avatar", patch)
        av.check("avatar mode PATCH default_credential 200 (no unknown-field 400)",
                 okj(s), f"HTTP {s} {str(up)[:100]}")
        if okj(s) and isinstance(up, dict):
            av.check("PATCH response keeps id+protected+default_credential",
                     up.get("id") == "avatar" and up.get("protected") is True
                     and up.get("default_credential") == brain, "")
        # re-GET proves every defaultable field survived (the D2 regression)
        s, p = jget("/api/assistant/modes/avatar")
        g = p if isinstance(p, dict) else {}
        av.check("avatar brain-bind PRESERVES manifest (planner_bias/persona/lanes)",
                 g.get("default_credential") == brain and len(g.get("planner_bias", [])) == 3
                 and "spoken_companion" in g.get("persona", [])
                 and "web." in g.get("allowed_tool_lanes", []),
                 f"dc={g.get('default_credential')} pb={len(g.get('planner_bias', []))}")
        av.delete_cred(brain)  # named without RUNID → not caught by purge_creds

    # 6.7 delete user mode
    if uid:
        s, p = jreq("DELETE", f"/api/assistant/modes/{uid}")
        av.check("modes.delete(user)", okj(s) and (p or {}).get("deleted") == uid, f"HTTP {s}")
    # 6.8 protected delete refused
    s, _ = jreq("DELETE", "/api/assistant/modes/avatar")
    av.check("protected delete avatar -> 403", s == 403, f"HTTP {s}")
    # 6.9 DEAD LAST — force delete (irreversible for the process lifetime)
    s, p = jreq("DELETE", "/api/assistant/modes/avatar?force=true")
    av.check("force delete avatar -> 200 {deleted:avatar}",
             okj(s) and (p or {}).get("deleted") == "avatar", f"HTTP {s}")


# ─────────────────────────────────────────────────────────────────────
# GROUP 9b — credentials secret-leak sweep (D1 + uniform redaction guard).
# av.test_credentials_lifecycle / _preserve_secret cover CRUD + the empty-secret
# regression; here we add the recursive no-leak assertion over the list.
# ─────────────────────────────────────────────────────────────────────

def g9_secret_leak(mock_port):
    print("\n## Group 9b — credential redaction (recursive no-secret-leak)")
    svc = f"leakprobe{av.RUNID}"
    av.register_cred(svc, f"http://127.0.0.1:{mock_port}/ok/v1", {"model": "x"})
    s, p = jget("/api/cloud/credentials")
    av.check("credentials.list 200", okj(s), f"HTTP {s}")
    assert_no_secret_leak("/api/cloud/credentials", p)
    av.delete_cred(svc)


# ─────────────────────────────────────────────────────────────────────
# GROUP 10/11 — apps lifecycle + deployments
# ─────────────────────────────────────────────────────────────────────

def g10_apps():
    print("\n## Group 10 — apps lifecycle")
    s, p = jpost("/api/apps", {"name": f"Full Test App {av.RUNID}",
                               "description": "ordo_full_test", "workspace_id": "test-ws"})
    app_id = idof(p)
    av.check("apps.create 200 + id", okj(s) and bool(app_id), f"HTTP {s} id={app_id}")
    if not app_id:
        return None
    s, _ = jget(f"/api/apps/{app_id}")
    av.check("apps.get round-trips", okj(s), f"HTTP {s}")
    s, p = jget("/api/apps?workspace_id=test-ws&limit=50")
    apps = (p or {}).get("apps", p if isinstance(p, list) else []) if p else []
    av.check("apps.list shows created", okj(s) and any(
        (a.get("id") == app_id) for a in apps if isinstance(a, dict)), f"HTTP {s}")
    s, p = jget(f"/api/apps/{app_id}/events")
    av.check("apps.events 200 + >=1 event", okj(s) and len(
        (p or {}).get("events", []) if isinstance(p, dict) else []) >= 1, f"HTTP {s}")
    s, _ = jget(f"/api/apps/{app_id}/state-at/1")
    av.check("apps.state-at no-500", no500(s), f"HTTP {s}")
    for verb, body, warn in (("publish", {"actor": "full-test"}, True),
                             ("unpublish", {"actor": "full-test"}, True),
                             ("archive", {}, False),
                             ("unarchive", {"actor": "full-test"}, True)):
        s, _ = jpost(f"/api/apps/{app_id}/{verb}", body)
        av.check(f"apps.{verb} no-500", no500(s), f"HTTP {s}", warn_only=warn and not okj(s))
    s, _ = jreq("PATCH", f"/api/apps/{app_id}", {"name": f"Updated {av.RUNID}"})
    av.check("apps.patch no-500", no500(s), f"HTTP {s}")
    return app_id


def g11_deployments(app_id):
    print("\n## Group 11 — deployments")
    if not app_id:
        # re-create a draft app to deploy
        _s, p = jpost("/api/apps", {"name": f"Deploy App {av.RUNID}"})
        app_id = idof(p)
    if not app_id:
        av.rec("WARN", "deployments", "no app to deploy")
        return
    s, p = jget(f"/api/apps/{app_id}/deployments")
    av.check("deployments.list no-500", no500(s), f"HTTP {s}")
    s, p = jpost(f"/api/apps/{app_id}/deployments", {"note": "deploy v1", "spec": {}})
    dep_id = idof(p)
    classify("deployments.create", s, json.dumps(p) if isinstance(p, (dict, list)) else p)
    if dep_id:
        s, _h, raw, _ = av.req("POST", f"/api/apps/{app_id}/deployments/{dep_id}/promote", {})
        classify("deployments.promote", s, raw)
        s, _h, raw, _ = av.req("POST", f"/api/apps/{app_id}/deployments/{dep_id}/fail",
                               {"reason": "test failure"})
        classify("deployments.fail", s, raw)


# ─────────────────────────────────────────────────────────────────────
# GROUP 12 — files (text + binary, byte-exact round-trip)
# ─────────────────────────────────────────────────────────────────────

def g12_files():
    import base64
    print("\n## Group 12 — files (text + binary round-trip)")
    # text
    s, p = jpost("/api/files", {"original_name": "full-test.txt",
                                "data_base64": "aGVsbG8gd29ybGQ=", "content_type": "text/plain"})
    fid = idof(p)
    av.check("files.upload(text) 200 + size 11", okj(s) and bool(fid)
             and (p or {}).get("size_bytes", (p or {}).get("file", {}).get("size_bytes")) in (11, None),
             f"HTTP {s} id={fid}")
    s, p = jget("/api/files")
    av.check("files.list 200", okj(s), f"HTTP {s}")
    if fid:
        s, h, raw, _ = av.req("GET", f"/api/files/{fid}/content")
        av.check("files.download(text) byte-exact",
                 s == 200 and raw == b"hello world", f"HTTP {s} {len(raw or b'')}B")
        s, _ = jreq("DELETE", f"/api/files/{fid}")
        av.check("files.delete(text)", okj(s), f"HTTP {s}")
    # binary (1x1 PNG) — catches base64/encoding bugs text round-trips can't
    png = bytes([0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A, 0x00, 0xFF, 0x10, 0x7E, 0xCC, 0x33])
    s, p = jpost("/api/files", {"original_name": "blob.bin",
                                "data_base64": base64.b64encode(png).decode(),
                                "content_type": "application/octet-stream"})
    fid = idof(p)
    av.check("files.upload(binary) 200", okj(s) and bool(fid), f"HTTP {s}")
    if fid:
        s, h, raw, _ = av.req("GET", f"/api/files/{fid}/content")
        av.check("files.download(binary) byte-exact", s == 200 and raw == png,
                 f"HTTP {s} match={raw == png}")
        jreq("DELETE", f"/api/files/{fid}")


# ─────────────────────────────────────────────────────────────────────
# GROUP 13 — webhooks (CRUD + redaction + REAL end-to-end fire + signature)
# ─────────────────────────────────────────────────────────────────────

def g13_webhooks():
    print("\n## Group 13 — webhooks (real end-to-end fire)")
    srv, port = start_receiver()
    try:
        s, p = jpost("/api/webhooks", {"target_url": f"http://127.0.0.1:{port}/hook",
                                       "topics": [], "description": "full test",
                                       "secret": f"webhook-secret-{av.RUNID}"})
        wid = idof(p)
        av.check("webhooks.register 200 + id", okj(s) and bool(wid), f"HTTP {s} id={wid}")
        if not wid:
            return
        s, p = jget(f"/api/webhooks/{wid}")
        av.check("webhooks.get 200", okj(s), f"HTTP {s}")
        assert_no_secret_leak(f"/api/webhooks/{wid}", p)
        s, p = jget("/api/webhooks")
        assert_no_secret_leak("/api/webhooks (list)", p)
        s, _ = jreq("PATCH", f"/api/webhooks/{wid}",
                    {"target_url": f"http://127.0.0.1:{port}/hook2",
                     "description": "Updated", "active": True})
        av.check("webhooks.patch no-500", no500(s), f"HTTP {s}")
        # REAL FIRE: an app create emits a bus event; topics=[] matches all.
        before = len(_RECV)
        jpost("/api/apps", {"name": f"Webhook Fire {av.RUNID}"})
        delivered = False
        for _ in range(24):
            if len(_RECV) > before:
                delivered = True
                break
            time.sleep(0.25)
        av.check("webhooks.deliver(end-to-end)", delivered,
                 "received a POST" if delivered else "no delivery in 6s (topic may differ; CRUD ok)",
                 warn_only=not delivered)
        if delivered:
            hdrs = _RECV[-1][0]
            sig = any("sign" in k or "hmac" in k or k.endswith("-signature") for k in hdrs)
            av.check("webhook delivery carries a signature header", sig,
                     "no signature header" if not sig else "signed", warn_only=not sig)
        s, p = jreq("DELETE", f"/api/webhooks/{wid}")
        av.check("webhooks.delete", okj(s), f"HTTP {s}")
    finally:
        srv.shutdown()


# ─────────────────────────────────────────────────────────────────────
# GROUP 14 — connections
# ─────────────────────────────────────────────────────────────────────

def g14_connections():
    print("\n## Group 14 — connections")
    s, p = jget("/api/connections/types")
    tlist = (p or {}).get("types", p if isinstance(p, list) else []) if p else []
    av.check("connections.types 200 + non-empty", okj(s) and len(tlist) >= 1, f"HTTP {s}")
    type_id = next((t.get("id") or t.get("type_id") for t in tlist
                    if isinstance(t, dict) and (t.get("id") or t.get("type_id"))), None)
    if not type_id:
        av.rec("WARN", "connections.create", "no connection type available")
        return
    s, p = jpost("/api/connections", {"type_id": type_id,
                                      "friendly_name": f"Full Test Conn {av.RUNID}",
                                      "fields": {}, "secret": f"dummy-{av.RUNID}"})
    cid = idof(p)
    if not okj(s):
        av.check("connections.create (structured 4xx ok)", s is not None and 400 <= s < 500,
                 f"HTTP {s}", warn_only=True)
        return
    av.check("connections.create 200 + redacted", bool(cid) and (
        isinstance(p, dict) and (p.get("connection") or p).get("secret") in (None, "")),
        f"HTTP {s} id={cid}")
    s, p = jget(f"/api/connections/{cid}")
    av.check("connections.get 200", okj(s), f"HTTP {s}")
    assert_no_secret_leak(f"/api/connections/{cid}", p)
    s, _ = jreq("PATCH", f"/api/connections/{cid}", {"friendly_name": f"Updated {av.RUNID}"})
    av.check("connections.patch no-500", no500(s), f"HTTP {s}")
    s, _h, raw, _ = av.req("POST", f"/api/connections/{cid}/test", {})
    av.check("connections.test no-500 (dummy secret may cleanly fail)", no500(s), f"HTTP {s}")
    s, p = jreq("DELETE", f"/api/connections/{cid}")
    av.check("connections.delete", okj(s), f"HTTP {s}")


# ─────────────────────────────────────────────────────────────────────
# GROUP 15 — MCP (real WASM install → invoke → quarantine → uninstall)
# ─────────────────────────────────────────────────────────────────────

def g15_mcp():
    import base64
    import hashlib
    print("\n## Group 15 — MCP (real WASM lifecycle)")
    s, _ = jget("/api/mcp/servers")
    av.check("mcp.servers.list 200", okj(s), f"HTTP {s}")
    s, _ = jpost("/api/mcp/servers/install", {"server_id": "x"})
    av.check("mcp.install(incomplete) rejected", s is not None and 400 <= s < 500,
             f"HTTP {s}", warn_only=not (s and 400 <= s < 500))
    sid = f"echo-{av.RUNID}"
    wasm = base64.b64decode(ECHO_WASM_B64)
    body = {
        "server_id": sid, "module_b64": ECHO_WASM_B64,
        "identity": {"name": "Echo Test", "version": "0.1.0", "publisher": "full-test",
                     "sigstore_cert": [1, 2, 3, 4],
                     "identity_hash": list(hashlib.sha256(wasm).digest())},
        "declaration": {"host_functions": [], "domains": [], "filesystem_paths": [],
                        "bus_topics": [], "secret_classes": []},
        "tool_catalog": [{"name": "hello", "description": "echoes its JSON input",
                          "input_schema": {}, "output_schema": {}, "risk_level": "read_only"}],
    }
    s, p = jpost("/api/mcp/servers/install", body)
    if not okj(s):
        av.rec("FAIL", "mcp.install(end-to-end)", f"HTTP {s}: {str(p)[:120]}")
        return
    av.rec("PASS", "mcp.install(end-to-end)", "WASM installed; lockfile signed")
    s, p = jget(f"/api/mcp/servers/{sid}/lockfile")
    # response shape: {"lockfile": {...}, "trust_state": "<label>"} — trust_state
    # is a SIBLING of lockfile, not nested inside it.
    trust_before = (p or {}).get("trust_state") if isinstance(p, dict) else None
    av.check("mcp.lockfile 200 + lockfile obj + trust_state",
             okj(s) and trust_before is not None and isinstance((p or {}).get("lockfile"), dict),
             f"HTTP {s} trust={trust_before}")
    s, p = jpost(f"/api/mcp/servers/{sid}/invoke/hello", {"arguments": {"x": 1, "msg": "hi ordo"}})
    echoed = isinstance(p, dict) and p.get("raw_response") == {"x": 1, "msg": "hi ordo"}
    av.check("mcp.invoke(end-to-end) echoes input", okj(s) and echoed, f"HTTP {s} echo={echoed}")
    # quarantine → re-GET lockfile, assert the trust_state actually moved
    s, _h, raw, _ = av.req("POST", f"/api/mcp/servers/{sid}/quarantine", {"reason": "test"})
    av.check("mcp.quarantine no-500", no500(s), f"HTTP {s}")
    if okj(s):
        s, p = jget(f"/api/mcp/servers/{sid}/lockfile")
        trust_after = (p or {}).get("trust_state") if isinstance(p, dict) else None
        av.check("mcp.quarantine changed trust_state", trust_after != trust_before,
                 f"{trust_before} -> {trust_after}", warn_only=(trust_after == trust_before))
    s, _h, raw, _ = av.req("POST", f"/api/mcp/servers/{sid}/re-authorize",
                           {"declaration": body["declaration"], "tool_catalog": body["tool_catalog"]})
    classify("mcp.re-authorize", s, raw)
    s, p = jreq("DELETE", f"/api/mcp/servers/{sid}")
    av.check("mcp.uninstall", okj(s), f"HTTP {s}")


# ─────────────────────────────────────────────────────────────────────
# GROUP 16 — self-heal / memory / settings / review / plugins /
#            automations / builds / ui-extensions (must-not-500 round-trips)
# ─────────────────────────────────────────────────────────────────────

def g16_misc(app_id):
    print("\n## Group 16 — self-heal / memory / settings / review / plugins / automations / builds / ui-ext")

    def probe(method, path, body=None, name=None):
        s, _h, raw, _ = av.req(method, path, body)
        classify(name or f"{method} {path}", s, raw)

    # self-heal
    probe("GET", "/api/self-heal/cases")
    probe("POST", "/api/self-heal/cases/pin", {"case_id": f"sh-{av.RUNID}"})
    probe("DELETE", "/api/self-heal/cases", {"case_id": f"sh-{av.RUNID}"})
    # memory
    probe("GET", "/api/memory/pinned")
    probe("POST", "/api/memory/pinned", {"note": f"pin-{av.RUNID}"})
    probe("GET", "/api/memory/working")
    probe("POST", "/api/memory/working", {"note": f"work-{av.RUNID}"})
    # settings — valid update -> 200; empty update -> clean 4xx (regression: was 500)
    probe("GET", "/api/runtime/settings")
    s, _ = jpost("/api/runtime/settings", {"profile": "standard"})
    av.check("runtime.settings update(valid) -> 200", okj(s), f"HTTP {s}")
    s, _h, raw, _ = av.req("POST", "/api/runtime/settings", {})
    av.check("runtime.settings empty update -> 4xx not 5xx", s is not None and 400 <= s < 500,
             f"HTTP {s}")
    # review (bogus id → clean rejection, never 5xx)
    probe("GET", "/api/review/pending")
    probe("GET", "/api/review/recent?limit=10")
    probe("POST", f"/api/review/{BOGUS}/approve", {"note": "ok"})
    probe("POST", f"/api/review/{BOGUS}/deny", {"note": "no"})
    # plugins
    probe("GET", "/api/plugins")
    probe("POST", f"/api/plugins/test-plugin-{av.RUNID}/enabled", {})
    probe("DELETE", f"/api/plugins/test-plugin-{av.RUNID}/enabled")
    # automations
    probe("GET", "/api/automations")
    s, p = jpost("/api/automations", {"id": f"auto-{av.RUNID}", "enabled": False,
                                      "approved": False, "tasks": []})
    classify("automations.create", s, json.dumps(p) if isinstance(p, (dict, list)) else p)
    probe("POST", "/api/automations/tick", {})
    probe("POST", f"/api/automations/auto-{av.RUNID}/approve", {})
    probe("DELETE", f"/api/automations/auto-{av.RUNID}")
    # builds
    probe("GET", "/api/builds")
    s, p = jpost("/api/builds", {"app_id": app_id or BOGUS})
    classify("builds.create", s, json.dumps(p) if isinstance(p, (dict, list)) else p)
    bid = idof(p)
    probe("GET", f"/api/builds/{bid or BOGUS}")
    probe("POST", f"/api/builds/{bid or BOGUS}/gate", {"result": "pass"})
    # ui-extensions
    probe("GET", "/api/ui-extensions")
    s, h, raw, _ = av.req("GET", "/api/ui-extensions/_bridge.js")
    av.check("ui-extensions bridge.js served", s == 200 and "javascript" in h.get("content-type", ""),
             f"HTTP {s} {h.get('content-type','')}")


# ─────────────────────────────────────────────────────────────────────
# GROUP 17 — RAG
# ─────────────────────────────────────────────────────────────────────

def g17_rag():
    print("\n## Group 17 — RAG")
    s, p = jget("/api/rag/collections")
    av.check("rag.collections 200", okj(s), f"HTTP {s}")
    s, p = jget("/api/rag/preview?query=test&top_k=5")
    av.check("rag.preview 200 + hits list", okj(s) and isinstance(
        (p or {}).get("hits"), list) if isinstance(p, dict) else okj(s), f"HTTP {s}")


# ─────────────────────────────────────────────────────────────────────
# GROUP 18 — capability sweep (classify; skip destructive/network)
# ─────────────────────────────────────────────────────────────────────

DESTRUCTIVE = ("delete", "remove", "write", "run", "exec", "native", "install",
               "deploy", "publish", "archive", "kill", "reset", "purge", "send",
               "fire", "uninstall", "quarantine", "promote", "forget")
NETWORK = ("web.", "http", "fetch", "url", "download", "crawl", "scrape")


def cap_name(d):
    if isinstance(d, str):
        return d
    if isinstance(d, dict):
        for k in ("capability", "name", "id", "cap"):
            v = d.get(k)
            if isinstance(v, str):
                return v
    return None


def g18_capabilities(descriptors, include_network):
    print("\n## Group 18 — capability sweep")
    # mandatory known-good non-destructive probe — guards against a vacuous pass
    s, _h, raw, _ = av.req("POST", "/api/tools/memory.list_pinned", {"limit": 10})
    av.check("capability memory.list_pinned -> 2xx", okj(s), f"HTTP {s}")
    if not descriptors:
        av.rec("WARN", "capability sweep", "no descriptors from /api/capabilities")
        return
    skipped = run = 0
    for d in descriptors:
        name = cap_name(d)
        if not name:
            continue
        low = name.lower()
        if any(w in low for w in DESTRUCTIVE):
            skipped += 1
            continue
        if (not include_network) and any(w in low for w in NETWORK):
            skipped += 1
            continue
        run += 1
        s, _h, raw, _ = av.req("POST", f"/api/tools/{name}", {})
        classify(f"cap {name}", s, raw)
    av.rec("PASS", "capability sweep coverage", f"{run} run, {skipped} skipped (destructive/network)")


# ─────────────────────────────────────────────────────────────────────
# GROUP 19 — orchestrator (wired via ORDO_ENABLE_ORCHESTRATOR=1)
# ─────────────────────────────────────────────────────────────────────

def g19_orchestrator():
    print("\n## Group 19 — orchestrator")
    s, _h, raw, _ = av.req("POST", "/api/orchestrate", {"goal": ""})
    body = (raw or b"").decode("utf-8", "replace")
    # empty goal → 4xx; the key signal is "wired, not feature-off" (never 503)
    av.check("orchestrate(empty goal) wired (4xx, not 503)",
             s is not None and 400 <= s < 500 and s != 503, f"HTTP {s} {body[:60]}")


# ─────────────────────────────────────────────────────────────────────
# GROUP 20 — adversarial / no-500 across mutating routes
# (avatar storms + big payloads are covered by av.test_concurrency /
#  av.test_speak_adversarial, run just before this.)
# ─────────────────────────────────────────────────────────────────────

def g20_adversarial():
    print("\n## Group 20 — adversarial / malformed / wrong-method (no-500)")
    for path in ("/api/avatar/speak", "/api/assistant/turn", "/api/apps", "/api/cloud/credentials"):
        s, _h, _raw, _ = av.req("POST", path, raw_body=b"{bad json",
                                headers={"Content-Type": "application/json"})
        av.check(f"malformed JSON {path} -> 4xx no-500", s is not None and 400 <= s < 500, f"HTTP {s}")
    # wrong method on GET-only / POST-only routes
    s, _h, _raw, _ = av.req("POST", "/health")
    av.check("POST /health -> 4xx/405 no-500", no500(s) and not okj(s), f"HTTP {s}")
    s, _h, _raw, _ = av.req("GET", "/api/avatar/speak")
    av.check("GET /api/avatar/speak -> 405 no-500", no500(s) and not okj(s), f"HTTP {s}")


# ─────────────────────────────────────────────────────────────────────
# Section C — explicitly NOT HTTP-testable (informational; never PASS)
# ─────────────────────────────────────────────────────────────────────

def section_c():
    print("\n## NOT HTTP-TESTABLE (browser/desktop UI only — covered by manual/visual QA)")
    for note in (
        "VAD mic capture / auto-stop — ordo-studio UI, no HTTP surface (manual mic test).",
        "Pop-out avatar window open/close (Tauri WebviewWindow / window.open) — desktop UI only.",
        "Lip-sync visual rendering (canvas sprite-atlas) — SSE frame DATA is asserted (Group 7), pixels are not.",
        "Mode-picker hiding the avatar mode from the chat picker — cosmetic; the API DOES return it (Group 6).",
        "WebSocket /ws/assistant/:session and /ws/review — no stdlib WS client; the SSE mirror is tested instead.",
        "avatar.html JS event wiring — Group 7 asserts the markup bytes are present, not handler behavior.",
    ):
        print(f"  [note] {note}")


# ─────────────────────────────────────────────────────────────────────
# Main
# ─────────────────────────────────────────────────────────────────────

def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--base-url", default="http://127.0.0.1:4142")
    ap.add_argument("--no-launch", action="store_true",
                    help="use an already-running runtime instead of launching one")
    ap.add_argument("--keep", action="store_true", help="leave the runtime running on exit")
    ap.add_argument("--bin", default=None, help="path to the ordo binary")
    ap.add_argument("--include-network", action="store_true",
                    help="also probe network-classed capabilities in the sweep")
    args = ap.parse_args()

    av.BASE = args.base_url.rstrip("/")
    port = av.port_of(av.BASE)
    if port == 4141:
        print("REFUSING to run against :4141 (protected main/frozen runtime). Use :4142.",
              file=sys.stderr)
        return 2

    launched = not args.no_launch
    proc = tmpdir = logf = None
    if launched:
        # Enable the orchestrator for Group 19 (avatar is enabled by launch_runtime).
        os.environ["ORDO_ENABLE_ORCHESTRATOR"] = "1"
        bin_path = av.find_binary(args.bin)
        if not bin_path:
            print("ordo binary not found — build it first: cargo build -p ordo-cli", file=sys.stderr)
            return 2
        print(f"# launching runtime: {bin_path} on :{port} (avatar+orchestrator, temp DB)")
        av.kill_port_listener(port)
        proc, tmpdir, logf = av.launch_runtime(bin_path, port)

    early_exit = None
    try:
        if not av.wait_health(60):
            print(f"runtime not healthy at {av.BASE}/health", file=sys.stderr)
            if tmpdir:
                try:
                    with open(os.path.join(tmpdir, "runtime.log")) as f:
                        print(f.read()[-2000:], file=sys.stderr)
                except Exception:
                    pass
            return 2

        srv, mock_port = start_full_mock()
        print(f"# Ordo FULL test -> {av.BASE}  (mock provider :{mock_port}, run {av.RUNID})")

        # ── Phase 1: cheap reads (clean baseline) ──
        descriptors = g1_liveness()
        g23_get_sweep()

        # ── Phase 2: avatar driver (SSE idle/cadence BEFORE any heavy speak) ──
        av.test_assets()
        av.test_sse_idle()
        av.test_return_to_idle()
        av.test_speak_and_visemes()
        av.test_sse_survives_bad_speak()
        av.test_sse_concurrent()

        # ── Phase 3: voice dispatch (credential-isolated; no avatar pollution) ──
        av.test_voice_no_provider()
        av.test_voice_openai(mock_port)
        av.test_voice_openai_options(mock_port)
        av.test_voice_minimax(mock_port)
        av.test_voice_minimax_format(mock_port)
        av.test_voice_groupid(mock_port)
        av.test_voice_resolution(mock_port)
        av.test_voice_candidate(mock_port)
        av.test_voice_broken_providers(mock_port)
        av.test_voice_transcribe(mock_port)
        av.test_credentials_lifecycle(mock_port)
        av.test_credentials_preserve_secret(mock_port)
        g9_secret_leak(mock_port)

        # ── Phase 4: assistant + new subsystems ──
        g4_turn(mock_port, launched)
        g5_facts()
        app_id = g10_apps()
        g11_deployments(app_id)
        g12_files()
        g13_webhooks()
        g14_connections()
        g15_mcp()
        g16_misc(app_id)
        g17_rag()
        g18_capabilities(descriptors, args.include_network)
        g19_orchestrator()

        # ── Phase 5: storms + adversarial LAST (long background schedules) ──
        av.test_concurrency()
        av.test_speak_adversarial()
        g20_adversarial()

        # ── Phase 6: modes LAST among functional groups (avatar force-delete
        #    is irreversible for the process lifetime; nothing after reads it) ──
        g6_modes(mock_port, launched)

        section_c()
        av.purge_creds()

        # crash canary: did the runtime exit on its own mid-run?
        if proc is not None:
            early_exit = proc.poll()
            av.check("runtime did NOT exit on its own during the run",
                     early_exit is None, f"exited early code={early_exit}")
    finally:
        if proc and not args.keep:
            print("# stopping test runtime")
            try:
                proc.terminate()
                proc.wait(timeout=10)
            except Exception:
                pass
            # Belt-and-suspenders on Windows: terminate() can return before the
            # binary's file lock is released (or leave an orphan), which blocks
            # the next `cargo build`. Always sweep the :4142 listener too.
            av.kill_port_listener(port)
            if logf:
                logf.close()
        elif args.keep:
            print(f"# leaving runtime running on :{port} (--keep)")

    # panic canary: a clean HTTP 200 doesn't prove a background task didn't panic
    if tmpdir:
        try:
            with open(os.path.join(tmpdir, "runtime.log"), encoding="utf-8", errors="replace") as f:
                log = f.read()
            panics = [ln for ln in log.splitlines()
                      if "panicked" in ln.lower() or "PANIC" in ln]
            av.check("no panic in runtime log", not panics,
                     (panics[0][:120] if panics else "clean"))
        except Exception as e:
            av.rec("WARN", "panic canary", f"could not read runtime log: {e}")

    print(f"\n# summary: {av.PASS} pass, {av.WARN} warn, {av.FAIL} fail")
    if av.FAIL:
        print("# FAILURES:")
        for level, name, detail in av.RESULTS:
            if level == "FAIL":
                print(f"  - {name}: {detail}")
    return 1 if av.FAIL else 0


if __name__ == "__main__":
    sys.exit(main())
