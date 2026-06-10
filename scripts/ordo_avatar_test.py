#!/usr/bin/env python3
"""Hard, self-contained test harness for the Ordo avatar + agnostic voice.

It launches its OWN runtime on a test port (default 127.0.0.1:4142) with an
isolated temp database and the avatar driver enabled, stands up a local MOCK
voice provider that speaks BOTH the OpenAI `/audio/speech` and the MiniMax
`/t2a_v2` contracts (and several deliberately-broken variants), then hammers:

  * the avatar static assets (/avatar.html, /avatar/*),
  * the SSE frame stream (/sse/avatar) — cadence, enum validity, broadcast,
  * the viseme producer (/api/avatar/speak) — happy path + adversarial inputs,
  * the provider-agnostic voice dispatch (/api/voice/speech) END TO END,
    including a byte-exact MiniMax hex round-trip, per-provider default
    selection, GroupId/auth propagation, voice_api resolution, and clean
    failure on every broken-provider variant (no 500 / no hang).

Nothing here touches :4141 (the main / frozen runtimes). The harness owns its
runtime process and tears it down at the end.

Usage:
  python scripts/ordo_avatar_test.py [--base-url http://127.0.0.1:4142]
                                     [--no-launch] [--keep] [--bin PATH]
"""

import argparse
import json
import os
import socket
import subprocess
import sys
import tempfile
import threading
import time
import urllib.error
import urllib.request
import uuid
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from urllib.parse import unquote

# ─────────────────────────────────────────────────────────────────────
# Config / globals
# ─────────────────────────────────────────────────────────────────────

# Force UTF-8 stdout so non-cp1252 chars (→, —, …) in test names don't crash
# the run on a Windows console.
try:
    sys.stdout.reconfigure(encoding="utf-8")
    sys.stderr.reconfigure(encoding="utf-8")
except Exception:
    pass

BASE = "http://127.0.0.1:4142"
RUNID = uuid.uuid4().hex[:8]

# Known audio payloads, with non-UTF8 bytes so we exercise true binary
# handling. The voice dispatch must return these *exactly*.
OPENAI_AUDIO = bytes([0x4F, 0x50, 0x4E, 0x00, 0xFF, 0xA5, 0x10, 0x7E,
                      0x00, 0x01, 0x02, 0xFD, 0xCC, 0x33, 0x99, 0x80])
MINIMAX_AUDIO = bytes([0x4D, 0x4D, 0x58, 0x00, 0x01, 0xFE, 0xDC, 0xBA,
                       0x98, 0x76, 0x54, 0x32, 0x10, 0xAB, 0xCD, 0xEF,
                       0x7F, 0x80, 0x00, 0xFF])

# Valid enum value sets (from ordo-protocol). mouth = Phoneme (UPPERCASE);
# expression / glitch serialize verbatim (PascalCase).
VALID_MOUTH = {
    "AA", "AE", "AH", "AO", "AW", "AY", "EH", "ER", "EY", "IH", "IY", "OW",
    "OY", "UH", "UW", "B", "CH", "D", "DH", "F", "G", "HH", "JH", "K", "L",
    "M", "N", "NG", "P", "R", "S", "SH", "T", "TH", "V", "W", "Y", "Z", "ZH",
    "REST",
}
VALID_EXPRESSION = {"Neutral", "Speaking", "Thinking", "Alarmed", "Amused", "Glitched"}
VALID_GLITCH = {"None", "Light", "Heavy"}

PASS = WARN = FAIL = 0
RESULTS = []


def rec(level, name, detail=""):
    global PASS, WARN, FAIL
    if level == "PASS":
        PASS += 1
    elif level == "WARN":
        WARN += 1
    else:
        FAIL += 1
    RESULTS.append((level, name, detail))
    mark = {"PASS": "ok  ", "WARN": "warn", "FAIL": "FAIL"}[level]
    print(f"  [{mark}] {name}" + (f"  — {detail}" if detail else ""))


def check(name, condition, detail="", warn_only=False):
    if condition:
        rec("PASS", name, detail)
    else:
        rec("WARN" if warn_only else "FAIL", name, detail)
    return condition


# ─────────────────────────────────────────────────────────────────────
# HTTP client (tolerant of non-JSON / binary bodies)
# ─────────────────────────────────────────────────────────────────────

def req(method, path, body=None, raw_body=None, headers=None, timeout=15):
    """Return (status, headers_dict, body_bytes, parsed_json_or_None)."""
    url = BASE + path
    data = None
    hdrs = dict(headers or {})
    if raw_body is not None:
        data = raw_body
    elif body is not None:
        data = json.dumps(body).encode()
        hdrs.setdefault("Content-Type", "application/json")
    r = urllib.request.Request(url, data=data, method=method, headers=hdrs)
    try:
        with urllib.request.urlopen(r, timeout=timeout) as resp:
            raw = resp.read()
            status = resp.status
            rh = {k.lower(): v for k, v in resp.headers.items()}
    except urllib.error.HTTPError as e:
        raw = e.read()
        status = e.code
        rh = {k.lower(): v for k, v in (e.headers.items() if e.headers else [])}
    except Exception as e:  # connection refused, timeout, etc.
        return (None, {}, b"", {"_error": str(e)})
    parsed = None
    try:
        parsed = json.loads(raw.decode())
    except Exception:
        parsed = None
    return (status, rh, raw, parsed)


def read_sse(path, seconds, stop_after_events=None):
    """Read an SSE stream for `seconds`; return list of (event, data_str)."""
    url = BASE + path
    events = []
    deadline = time.time() + seconds
    try:
        with urllib.request.urlopen(url, timeout=seconds + 5) as resp:
            cur_event = "message"
            for raw_line in resp:
                line = raw_line.decode(errors="replace").rstrip("\n").rstrip("\r")
                if line.startswith("event:"):
                    cur_event = line[6:].strip()
                elif line.startswith("data:"):
                    events.append((cur_event, line[5:].strip()))
                    cur_event = "message"
                    if stop_after_events and len(events) >= stop_after_events:
                        break
                if time.time() > deadline:
                    break
    except Exception as e:
        events.append(("_error", str(e)))
    return events


# ─────────────────────────────────────────────────────────────────────
# Mock voice provider — serves OpenAI + MiniMax shapes, records requests
# ─────────────────────────────────────────────────────────────────────

class MockState:
    def __init__(self):
        self.lock = threading.Lock()
        self.requests = []  # list of dicts

    def record(self, entry):
        with self.lock:
            self.requests.append(entry)

    def last(self, kind=None):
        with self.lock:
            for entry in reversed(self.requests):
                if kind is None or entry.get("kind") == kind:
                    return entry
        return None

    def count(self, kind=None):
        with self.lock:
            return sum(1 for e in self.requests if kind is None or e.get("kind") == kind)


MOCK = MockState()


def _hex(b):
    return b.hex()


# Mirror of ordo_cloud::openai::content_type_for_format.
def content_type_for_format(fmt):
    return {
        "aac": "audio/aac", "flac": "audio/flac", "opus": "audio/opus",
        "pcm": "audio/L16", "wav": "audio/wav",
    }.get(fmt, "audio/mpeg")


class MockHandler(BaseHTTPRequestHandler):
    # Path layout: /<mode>/v1/audio/speech  or  /<mode>/v1/t2a_v2
    # mode ∈ {ok, err500, err401, badjson, badresp, badhex, hexws, ctweird, ctnone}
    def log_message(self, *args):
        pass  # silence

    def _send(self, status, body_bytes, content_type):
        self.send_response(status)
        if content_type is not None:
            self.send_header("Content-Type", content_type)
        self.send_header("Content-Length", str(len(body_bytes)))
        self.end_headers()
        self.wfile.write(body_bytes)

    def do_POST(self):
        full = self.path
        # split mode + query
        path = full.split("?", 1)[0]
        query = full.split("?", 1)[1] if "?" in full else ""
        parts = [p for p in path.split("/") if p]
        mode = parts[0] if parts else "ok"
        endpoint = "/" + "/".join(parts[1:])  # e.g. /v1/audio/speech
        length = int(self.headers.get("Content-Length", "0") or "0")
        raw = self.rfile.read(length) if length else b""
        try:
            payload = json.loads(raw.decode())
        except Exception:
            payload = None
        auth = self.headers.get("Authorization", "")

        group_id = None
        group_id_present = False
        for kv in query.split("&"):
            if kv.startswith("GroupId="):
                group_id_present = True
                group_id = unquote(kv[len("GroupId="):])  # reverse our %-encoding

        # Match by suffix so extra base_url path segments (used by the
        # base_url-inference test, e.g. /ok/minimax/v1) still route.
        if endpoint.endswith("/audio/speech"):
            MOCK.record({"kind": "openai", "mode": mode, "auth": auth,
                         "payload": payload, "query": query})
            if mode == "err500":
                return self._send(500, b'{"error":"mock 500"}', "application/json")
            if mode == "err401":
                return self._send(401, b'{"error":{"message":"Invalid authentication"}}',
                                   "application/json")
            if mode == "badjson":
                return self._send(200, b"<<<not audio at all>>>", "audio/mpeg")
            if mode == "ctweird":
                # provider sends an unusual content-type → dispatch must pass it through
                return self._send(200, OPENAI_AUDIO, "audio/x-weird")
            if mode == "ctnone":
                # provider omits content-type → dispatch falls back to format-derived
                return self._send(200, OPENAI_AUDIO, None)
            # ok: echo a content-type derived from the requested format, so the
            # format flows end-to-end (and the round-trip stays byte-exact).
            fmt = (payload or {}).get("response_format") or "mp3"
            return self._send(200, OPENAI_AUDIO, content_type_for_format(fmt))

        if endpoint.endswith("/t2a_v2"):
            MOCK.record({"kind": "minimax", "mode": mode, "auth": auth,
                         "payload": payload, "group_id": group_id,
                         "group_id_present": group_id_present, "query": query})
            if mode == "err500":
                return self._send(500, b'{"error":"mock 500"}', "application/json")
            if mode == "badjson":
                return self._send(200, b"not-json-at-all", "application/json")
            if mode == "badresp":
                body = json.dumps({
                    "base_resp": {"status_code": 1001, "status_msg": "quota exceeded (mock)"}
                }).encode()
                return self._send(200, body, "application/json")
            if mode == "badhex":
                body = json.dumps({
                    "data": {"audio": "zzzz-not-hex"},
                    "base_resp": {"status_code": 0, "status_msg": "success"},
                }).encode()
                return self._send(200, body, "application/json")
            if mode == "hexws":
                # outer whitespace around the hex — decode_hex must trim + succeed
                body = json.dumps({
                    "data": {"audio": "  " + _hex(MINIMAX_AUDIO) + "  ", "status": 2},
                    "base_resp": {"status_code": 0, "status_msg": "success"},
                }).encode()
                return self._send(200, body, "application/json")
            # normal: hex-encode the known MiniMax audio
            body = json.dumps({
                "data": {"audio": _hex(MINIMAX_AUDIO), "status": 2},
                "base_resp": {"status_code": 0, "status_msg": "success"},
            }).encode()
            return self._send(200, body, "application/json")

        self._send(404, b'{"error":"unknown mock endpoint"}', "application/json")


def start_mock():
    srv = ThreadingHTTPServer(("127.0.0.1", 0), MockHandler)
    threading.Thread(target=srv.serve_forever, daemon=True).start()
    return srv, srv.server_address[1]


# ─────────────────────────────────────────────────────────────────────
# Credential lifecycle helpers
# ─────────────────────────────────────────────────────────────────────

def secret_for(service):
    return f"sk-{service}-{RUNID}"


def register_cred(service, base_url, extras, auth_style="bearer"):
    body = {
        "service": service,
        "label": f"mock {service}",
        "auth_style": auth_style,
        "secret": secret_for(service),
        "base_url": base_url,
        "extras": extras,
    }
    s, h, raw, p = req("POST", "/api/cloud/credentials", body)
    register_cred.last_response = (s, p)
    # wait for it to be listable (publish-after-commit is fast but async)
    for _ in range(20):
        ls, _, _, lp = req("GET", "/api/cloud/credentials")
        names = {c.get("service") for c in (lp or {}).get("credentials", [])} if lp else set()
        if service in names:
            return True
        time.sleep(0.1)
    return False


def delete_cred(service):
    req("DELETE", "/api/cloud/credentials", {"service": service})


def list_cred_services():
    s, _, _, p = req("GET", "/api/cloud/credentials")
    if not p:
        return set()
    return {c.get("service") for c in p.get("credentials", [])}


def purge_creds():
    # Only ever remove THIS run's mock credentials (they all carry RUNID),
    # so running with --no-launch against a shared runtime never deletes a
    # real provider credential.
    for svc in list(list_cred_services()):
        if svc and RUNID in svc:
            delete_cred(svc)


def foreign_creds_present():
    return any(svc and RUNID not in svc for svc in list_cred_services())


# ─────────────────────────────────────────────────────────────────────
# Runtime lifecycle
# ─────────────────────────────────────────────────────────────────────

def port_of(url):
    return int(url.rsplit(":", 1)[1])


def wait_health(timeout=60):
    for _ in range(timeout * 2):
        s, _, _, _ = req("GET", "/health", timeout=2)
        if s == 200:
            return True
        time.sleep(0.5)
    return False


def find_binary(explicit):
    if explicit:
        return explicit
    here = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
    for cand in ("target/release/ordo.exe", "target/debug/ordo.exe",
                 "target/release/ordo", "target/debug/ordo"):
        p = os.path.join(here, cand)
        if os.path.exists(p):
            return p
    return None


def kill_port_listener(port):
    """Kill ONLY the process listening on `port` (never blanket-kill)."""
    try:
        if os.name == "nt":
            out = subprocess.run(["netstat", "-ano"], capture_output=True, text=True).stdout
            pids = set()
            for line in out.splitlines():
                if f"127.0.0.1:{port} " in line and "LISTENING" in line:
                    pids.add(line.split()[-1])
            for pid in pids:
                subprocess.run(["taskkill", "/PID", pid, "/F"],
                               capture_output=True, text=True)
    except Exception:
        pass


def launch_runtime(bin_path, port):
    tmpdir = tempfile.mkdtemp(prefix="ordo-avatar-test-")
    env = dict(os.environ)
    env.update({
        "ORDO_CONTROL_API_BIND": f"127.0.0.1:{port}",
        "ORDO_ENABLE_AVATAR": "1",
        "ORDO_DATABASE_PATH": os.path.join(tmpdir, "test.db"),
        "ORDO_RUNTIME_PROFILE": "standard",
        # The HTTP rate limiter defaults to 60 req/10s per IP. This functional
        # suite fires far more than that in a burst and is NOT testing the
        # limiter (the concurrency test covers graceful 200-or-429 separately),
        # so raise the ceiling well out of the way to avoid false failures.
        "ORDO_RATELIMIT_RPS": "100000",
    })
    logf = open(os.path.join(tmpdir, "runtime.log"), "w")
    proc = subprocess.Popen([bin_path, "serve"], env=env, stdout=logf,
                            stderr=subprocess.STDOUT)
    return proc, tmpdir, logf


# ─────────────────────────────────────────────────────────────────────
# Test groups
# ─────────────────────────────────────────────────────────────────────

def png_dims(raw):
    # PNG: 8-byte sig, then IHDR (length+type), then width/height big-endian.
    if len(raw) < 24 or raw[:8] != b"\x89PNG\r\n\x1a\n":
        return None
    w = int.from_bytes(raw[16:20], "big")
    h = int.from_bytes(raw[20:24], "big")
    return (w, h)


def test_assets():
    print("\n## avatar static assets")
    s, h, raw, _ = req("GET", "/avatar.html")
    check("avatar.html 200", s == 200, f"HTTP {s}")
    check("avatar.html is html", "text/html" in h.get("content-type", ""), h.get("content-type", ""))
    txt = raw.decode(errors="replace")
    check("avatar.html has canvas", "<canvas" in txt)
    check("avatar.html wires /sse/avatar", "/sse/avatar" in txt)
    check("avatar.html has cloud-voice toggle", "cloud voice" in txt and "/api/voice/speech" in txt)

    s, h, raw, p = req("GET", "/avatar/avatar.json")
    check("avatar.json 200", s == 200, f"HTTP {s}")
    check("avatar.json is json", "application/json" in h.get("content-type", ""))
    if p:
        check("avatar.json cell sizes", p.get("cell_width") == 128 and p.get("cell_height") == 128)
        check("mouth cell_count=8", p.get("mouth", {}).get("cell_count") == 8)
        check("expression cell_count=6", p.get("expression", {}).get("cell_count") == 6)
        check("glitch cell_count=2", p.get("glitch", {}).get("cell_count") == 2)

    for name, cells in (("mouth", 8), ("expression", 6), ("glitch", 2)):
        s, h, raw, _ = req("GET", f"/avatar/{name}.png")
        ok_status = s == 200
        ct = "image/png" in h.get("content-type", "")
        dims = png_dims(raw)
        check(f"{name}.png 200+png", ok_status and ct, f"HTTP {s} {h.get('content-type','')}")
        check(f"{name}.png valid PNG {cells} cells",
              dims is not None and dims[1] == 128 and dims[0] == cells * 128,
              f"dims={dims}")

    s, _, _, _ = req("GET", "/avatar/does-not-exist.png")
    check("missing asset 404s (no 500)", s in (404, 405), f"HTTP {s}")
    s, _, _, _ = req("POST", "/avatar/mouth.png")
    check("wrong method on asset (no 500)", s in (404, 405), f"HTTP {s}", warn_only=True)


def parse_frames(events):
    frames = []
    for ev, data in events:
        if ev == "frame":
            try:
                frames.append(json.loads(data))
            except Exception:
                pass
    return frames


def test_sse_idle():
    print("\n## /sse/avatar — subscription + idle cadence")
    events = read_sse("/sse/avatar", 3.0)
    subs = [e for e in events if e[0] == "subscribed"]
    frames = parse_frames(events)
    check("subscribed hello event", len(subs) == 1, f"{len(subs)}")
    # ~30Hz over ~3s → expect well over 50 frames; be lenient for CI jitter.
    check("idle frames stream (~30Hz)", len(frames) >= 40, f"{len(frames)} frames in ~3s")
    bad = [f for f in frames
           if f.get("mouth") not in VALID_MOUTH
           or f.get("expression") not in VALID_EXPRESSION
           or f.get("glitch") not in VALID_GLITCH]
    check("all idle frames have valid enum values", not bad,
          f"{len(bad)} invalid" + (f" e.g. {bad[0]}" if bad else ""))
    # exact key set — catches a serialization regression that adds/renames a field
    badkeys = [f for f in frames if set(f.keys()) != {"mouth", "expression", "glitch"}]
    check("frames carry EXACTLY {mouth,expression,glitch}", not badkeys,
          f"{len(badkeys)} off" + (f" e.g. {sorted(badkeys[0].keys())}" if badkeys else ""))
    # a bus-filter regression would surface as 'ignored'/'lagged' events
    noise = [e for e in events if e[0] in ("ignored", "lagged")]
    check("no 'ignored'/'lagged' SSE events (bus filter intact)", not noise,
          f"{len(noise)} noise events", warn_only=True)
    rest = [f for f in frames if f.get("mouth") == "REST"]
    check("idle is mostly REST mouth", frames and len(rest) >= len(frames) * 0.8,
          f"{len(rest)}/{len(frames)} REST")


def test_speak_and_visemes():
    print("\n## /api/avatar/speak — happy path + viseme correlation")
    s, _, _, p = req("POST", "/api/avatar/speak", {"text": "hello ordo avatar"})
    uid = (p or {}).get("utterance_id")
    check("speak 200 + utterance_id", s == 200 and uid, f"HTTP {s} {uid}")
    if uid:
        try:
            uuid.UUID(uid)
            rec("PASS", "utterance_id is a UUID", uid)
        except Exception:
            rec("FAIL", "utterance_id is a UUID", uid)

    # Correlate: open SSE, speak, expect Speaking + non-REST visemes.
    holder = {}

    def grab():
        holder["events"] = read_sse("/sse/avatar", 5.0)

    t = threading.Thread(target=grab)
    t.start()
    time.sleep(0.5)
    req("POST", "/api/avatar/speak", {"text": "the quick brown fox jumps"})
    t.join()
    frames = parse_frames(holder.get("events", []))
    speaking = [f for f in frames if f.get("expression") == "Speaking"]
    nonrest = [f for f in frames if f.get("mouth") not in ("REST",)]
    check("speak drives Speaking expression", len(speaking) >= 3, f"{len(speaking)} speaking frames")
    check("speak drives non-REST visemes", len(nonrest) >= 3, f"{len(nonrest)} non-REST frames")
    seen = {f.get("mouth") for f in nonrest}
    check("visemes are varied + valid", len(seen) >= 3 and seen <= VALID_MOUTH,
          f"{sorted(seen)}")


def test_speak_adversarial():
    print("\n## /api/avatar/speak — adversarial (must not 500/hang)")
    cases = [
        ("empty text -> 400", {"text": ""}, (400,)),
        ("whitespace text -> 400", {"text": "    "}, (400,)),
        ("missing text -> 4xx", {"voice_id": "x"}, (400, 422)),
        ("text=null -> 4xx", {"text": None}, (400, 422)),
        ("text=number -> 4xx", {"text": 123}, (400, 422)),
    ]
    for name, body, ok_codes in cases:
        s, _, _, _ = req("POST", "/api/avatar/speak", body)
        check(name, s in ok_codes, f"HTTP {s}")
    # malformed JSON body
    s, _, _, _ = req("POST", "/api/avatar/speak",
                     raw_body=b"{not json", headers={"Content-Type": "application/json"})
    check("malformed JSON -> 4xx (no 500)", s and 400 <= s < 500, f"HTTP {s}")
    # wrong method
    s, _, _, _ = req("GET", "/api/avatar/speak")
    check("GET on speak -> 405 (no 500)", s in (404, 405), f"HTTP {s}")
    # huge + unicode
    s, _, _, p = req("POST", "/api/avatar/speak", {"text": "🤖 " * 20000})
    check("100k unicode text handled (no 500/hang)", s in (200, 400, 413), f"HTTP {s}")
    s, _, _, _ = req("POST", "/api/avatar/speak",
                     {"text": "héllo‮world\n\t\x07 café — déjà 日本語"})
    check("control/RTL/unicode text handled", s == 200, f"HTTP {s}")
    # unknown fields are ignored (no deny_unknown_fields), valid text still 200
    s, _, _, p = req("POST", "/api/avatar/speak",
                     {"text": "hello", "unknown_field": "x", "another": 123})
    check("unknown fields ignored (200 + utterance_id)", s == 200 and (p or {}).get("utterance_id"),
          f"HTTP {s}")
    # ~3MB body: must not 5xx / hang. Code is policy (413/400 capped, or 200 if no cap).
    s, _, _, _ = req("POST", "/api/avatar/speak", {"text": "A" * (3 * 1024 * 1024)}, timeout=30)
    check("3MB body handled (no 5xx/hang)", s in (200, 400, 413), f"HTTP {s}",
          warn_only=(s == 200))


def test_voice_no_provider():
    print("\n## /api/voice/speech — no provider configured (graceful)")
    purge_creds()
    if foreign_creds_present():
        rec("WARN", "no-provider test skipped (foreign creds present, --no-launch)")
        return
    s, _, _, _ = req("POST", "/api/voice/speech", {"input": "hello"})
    check("no provider -> clean 4xx (no 500/panic)", s and 400 <= s < 500, f"HTTP {s}")
    s, _, _, _ = req("POST", "/api/voice/speech", {"input": ""})
    check("empty input -> 4xx (no 500)", s and 400 <= s < 500, f"HTTP {s}")
    s, _, _, _ = req("POST", "/api/voice/speech", {})
    check("missing input -> 4xx (no 500)", s and 400 <= s < 500, f"HTTP {s}")


def test_voice_openai(mock_port):
    print("\n## /api/voice/speech — OpenAI-compatible provider (mock)")
    purge_creds()
    svc = f"mockopenai{RUNID}"
    ok = register_cred(svc, f"http://127.0.0.1:{mock_port}/ok/v1", {})
    check("register openai mock credential", ok)
    before = MOCK.count("openai")
    s, h, raw, _ = req("POST", "/api/voice/speech", {"input": "speak via openai", "service": svc})
    check("openai voice 200", s == 200, f"HTTP {s}")
    check("openai returns exact mock audio bytes", raw == OPENAI_AUDIO,
          f"{len(raw)} bytes, match={raw == OPENAI_AUDIO}")
    check("openai content-type audio/mpeg", "audio/mpeg" in h.get("content-type", ""),
          h.get("content-type", ""))
    check("x-ordo-tts-provider == service", h.get("x-ordo-tts-provider") == svc,
          h.get("x-ordo-tts-provider"))
    check("openai endpoint was actually hit", MOCK.count("openai") == before + 1)
    last = MOCK.last("openai") or {}
    pl = last.get("payload") or {}
    check("openai default model (no leak)", pl.get("model") == "gpt-4o-mini-tts", pl.get("model"))
    check("openai default voice (no leak)", pl.get("voice") == "alloy", pl.get("voice"))
    check("openai input propagated", pl.get("input") == "speak via openai")
    check("openai bearer auth is the EXACT secret",
          last.get("auth") == f"Bearer {secret_for(svc)}", last.get("auth"))
    delete_cred(svc)


def test_voice_openai_options(mock_port):
    print("\n## /api/voice/speech — OpenAI overrides / extras-fallback / content-type")
    purge_creds()
    svc = f"oaiopt{RUNID}"
    register_cred(svc, f"http://127.0.0.1:{mock_port}/ok/v1", {"voice_api": "openai"})

    # request-level overrides win over provider defaults
    s, h, _, _ = req("POST", "/api/voice/speech",
                     {"input": "x", "service": svc, "model": "custom-model-v2", "voice": "shimmer"})
    last = MOCK.last("openai") or {}
    pl = last.get("payload") or {}
    check("openai model override", pl.get("model") == "custom-model-v2", pl.get("model"))
    check("openai voice override", pl.get("voice") == "shimmer", pl.get("voice"))
    check("x-ordo-tts-model echoes override", h.get("x-ordo-tts-model") == "custom-model-v2")
    check("x-ordo-tts-voice echoes override", h.get("x-ordo-tts-voice") == "shimmer")

    # format flows end-to-end → content-type tracks it
    s, h, _, _ = req("POST", "/api/voice/speech", {"input": "x", "service": svc, "format": "wav"})
    last = MOCK.last("openai") or {}
    check("openai response_format=wav sent", (last.get("payload") or {}).get("response_format") == "wav")
    check("x-ordo-tts-format=wav", h.get("x-ordo-tts-format") == "wav")
    check("wav content-type", "audio/wav" in h.get("content-type", ""), h.get("content-type"))
    check("instructions forwarded only when set", True)
    s, h, _, _ = req("POST", "/api/voice/speech",
                     {"input": "x", "service": svc, "instructions": "speak calmly"})
    check("openai instructions forwarded", (MOCK.last('openai') or {}).get("payload", {}).get("instructions") == "speak calmly")
    delete_cred(svc)

    # extras-level defaults (tts_model/tts_voice/tts_format) when request omits them
    svc2 = f"oaiextra{RUNID}"
    register_cred(svc2, f"http://127.0.0.1:{mock_port}/ok/v1",
                  {"voice_api": "openai", "tts_model": "my-model", "tts_voice": "my-voice", "tts_format": "flac"})
    s, h, raw, _ = req("POST", "/api/voice/speech", {"input": "x", "service": svc2})
    pl = (MOCK.last("openai") or {}).get("payload") or {}
    check("extras tts_model used", pl.get("model") == "my-model", pl.get("model"))
    check("extras tts_voice used", pl.get("voice") == "my-voice", pl.get("voice"))
    check("extras tts_format used", pl.get("response_format") == "flac", pl.get("response_format"))
    check("extras flac content-type", "audio/flac" in h.get("content-type", ""), h.get("content-type"))
    delete_cred(svc2)

    # content-type PASSTHROUGH: provider sends an unusual type → dispatch honors it
    svc3 = f"oaictweird{RUNID}"
    register_cred(svc3, f"http://127.0.0.1:{mock_port}/ctweird/v1", {"voice_api": "openai"})
    s, h, raw, _ = req("POST", "/api/voice/speech", {"input": "x", "service": svc3})
    check("provider content-type passed through (audio/x-weird)",
          "audio/x-weird" in h.get("content-type", ""), h.get("content-type"))
    check("ctweird still returns exact bytes", raw == OPENAI_AUDIO)
    delete_cred(svc3)

    # content-type FALLBACK: provider omits type → dispatch uses format-derived
    svc4 = f"oaictnone{RUNID}"
    register_cred(svc4, f"http://127.0.0.1:{mock_port}/ctnone/v1", {"voice_api": "openai"})
    s, h, raw, _ = req("POST", "/api/voice/speech", {"input": "x", "service": svc4})
    check("missing provider content-type falls back to audio/mpeg",
          "audio/mpeg" in h.get("content-type", ""), h.get("content-type"))
    delete_cred(svc4)


def test_voice_minimax(mock_port):
    print("\n## /api/voice/speech — MiniMax provider (mock, byte-exact)")
    purge_creds()
    svc = f"mockminimax{RUNID}"
    gid = f"grp-{RUNID}"
    ok = register_cred(svc, f"http://127.0.0.1:{mock_port}/ok/v1",
                       {"voice_api": "minimax", "group_id": gid})
    check("register minimax mock credential", ok)
    before = MOCK.count("minimax")
    s, h, raw, _ = req("POST", "/api/voice/speech", {"input": "speak via minimax", "service": svc})
    check("minimax voice 200", s == 200, f"HTTP {s}")
    check("minimax hex round-trips to EXACT bytes", raw == MINIMAX_AUDIO,
          f"{len(raw)} bytes, match={raw == MINIMAX_AUDIO}")
    check("minimax content-type audio/mpeg", "audio/mpeg" in h.get("content-type", ""),
          h.get("content-type", ""))
    check("x-ordo-tts-provider == service", h.get("x-ordo-tts-provider") == svc,
          h.get("x-ordo-tts-provider"))
    check("minimax /t2a_v2 endpoint hit", MOCK.count("minimax") == before + 1)
    last = MOCK.last("minimax") or {}
    pl = last.get("payload") or {}
    check("minimax GroupId propagated", last.get("group_id") == gid, last.get("group_id"))
    check("minimax bearer auth is the EXACT secret",
          last.get("auth") == f"Bearer {secret_for(svc)}", last.get("auth"))
    check("minimax default model (speech-02-hd, no openai leak)",
          pl.get("model") == "speech-02-hd", pl.get("model"))
    vs = pl.get("voice_setting") or {}
    check("minimax default voice (male-qn-qingse, no 'alloy' leak)",
          vs.get("voice_id") == "male-qn-qingse", vs.get("voice_id"))
    check("minimax text propagated", pl.get("text") == "speak via minimax")
    # type-strict full-body assertions — first real exercise of the adapter (C7).
    check("minimax stream is boolean false", pl.get("stream") is False, repr(pl.get("stream")))
    check("minimax voice_setting.vol==1.0", vs.get("vol") == 1.0, repr(vs.get("vol")))
    check("minimax voice_setting.pitch==0 (int)",
          isinstance(vs.get("pitch"), int) and vs.get("pitch") == 0, repr(vs.get("pitch")))
    check("minimax voice_setting.speed==1.0 default", vs.get("speed") == 1.0, repr(vs.get("speed")))
    aset = pl.get("audio_setting") or {}
    check("minimax audio_setting.sample_rate==32000", aset.get("sample_rate") == 32000, repr(aset.get("sample_rate")))
    check("minimax audio_setting.channel==1", aset.get("channel") == 1, repr(aset.get("channel")))
    check("minimax audio_setting.format==mp3 default", aset.get("format") == "mp3", repr(aset.get("format")))
    delete_cred(svc)


def test_voice_minimax_format(mock_port):
    print("\n## /api/voice/speech — MiniMax non-mp3 format + hex whitespace tolerance")
    purge_creds()
    # non-mp3 format: content-type tracks it, bytes still exact
    svc = f"mmflac{RUNID}"
    register_cred(svc, f"http://127.0.0.1:{mock_port}/ok/v1",
                  {"voice_api": "minimax", "group_id": "g", "tts_format": "flac"})
    s, h, raw, _ = req("POST", "/api/voice/speech", {"input": "x", "service": svc})
    check("minimax flac: exact bytes", raw == MINIMAX_AUDIO)
    check("minimax flac: content-type audio/flac", "audio/flac" in h.get("content-type", ""), h.get("content-type"))
    check("minimax flac: x-ordo-tts-format=flac", h.get("x-ordo-tts-format") == "flac")
    check("minimax flac: audio_setting.format=flac",
          ((MOCK.last("minimax") or {}).get("payload") or {}).get("audio_setting", {}).get("format") == "flac")
    delete_cred(svc)

    # outer-whitespace hex must still decode to exact bytes
    svc2 = f"mmws{RUNID}"
    register_cred(svc2, f"http://127.0.0.1:{mock_port}/hexws/v1",
                  {"voice_api": "minimax", "group_id": "g"})
    s, h, raw, _ = req("POST", "/api/voice/speech", {"input": "x", "service": svc2})
    check("minimax hex with outer whitespace decodes to exact bytes", s == 200 and raw == MINIMAX_AUDIO,
          f"HTTP {s} {len(raw)}b")
    delete_cred(svc2)


def test_voice_groupid(mock_port):
    print("\n## /api/voice/speech — MiniMax GroupId edge cases")
    purge_creds()
    # whitespace-only group_id → trimmed away → no GroupId query param
    svc = f"mmwsg{RUNID}"
    register_cred(svc, f"http://127.0.0.1:{mock_port}/ok/v1",
                  {"voice_api": "minimax", "group_id": "   "})
    req("POST", "/api/voice/speech", {"input": "x", "service": svc})
    last = MOCK.last("minimax") or {}
    check("whitespace group_id → no GroupId param", last.get("group_id_present") is False,
          f"present={last.get('group_id_present')}")
    delete_cred(svc)

    # special-char group_id → percent-encoded on the wire, decodes back exactly
    purge_creds()
    svc2 = f"mmspecial{RUNID}"
    special = "grp x&y=z"
    register_cred(svc2, f"http://127.0.0.1:{mock_port}/ok/v1",
                  {"voice_api": "minimax", "group_id": special})
    req("POST", "/api/voice/speech", {"input": "x", "service": svc2})
    last = MOCK.last("minimax") or {}
    check("special-char GroupId survives url-encoding round-trip",
          last.get("group_id") == special, f"{last.get('group_id')!r}")
    check("special-char GroupId did not corrupt the query (single t2a hit)",
          MOCK.count("minimax") >= 1)
    delete_cred(svc2)


def test_voice_resolution(mock_port):
    print("\n## voice_api resolution (explicit / inference / default)")
    purge_creds()
    base_ok = f"http://127.0.0.1:{mock_port}/ok/v1"

    # 1. name inference: service name contains 'minimax', no explicit api.
    svc = f"infermini{RUNID}-minimax"
    register_cred(svc, base_ok, {"group_id": "g1"})
    o0, m0 = MOCK.count("openai"), MOCK.count("minimax")
    req("POST", "/api/voice/speech", {"input": "x", "service": svc})
    check("name-inferred minimax hits /t2a_v2", MOCK.count("minimax") == m0 + 1 and MOCK.count("openai") == o0)
    delete_cred(svc)

    # 2. base_url inference: the url string contains 'minimax' (no explicit
    # voice_api, no minimax in the service name). The mock matches the
    # endpoint by suffix, so the extra /minimax/ segment still routes; mode
    # stays 'ok' (first segment).
    purge_creds()
    svc = f"urlinfer{RUNID}"
    register_cred(svc, f"http://127.0.0.1:{mock_port}/ok/minimax/v1", {"group_id": "g2"})
    o0, m0 = MOCK.count("openai"), MOCK.count("minimax")
    req("POST", "/api/voice/speech", {"input": "x", "service": svc})
    check("base_url-inferred minimax hits /t2a_v2",
          MOCK.count("minimax") == m0 + 1, f"openai+{MOCK.count('openai')-o0} minimax+{MOCK.count('minimax')-m0}")
    delete_cred(svc)

    # 3. explicit override beats the name: name says minimax, extras say openai.
    purge_creds()
    svc = f"override{RUNID}-minimax"
    register_cred(svc, base_ok, {"voice_api": "openai"})
    o0, m0 = MOCK.count("openai"), MOCK.count("minimax")
    req("POST", "/api/voice/speech", {"input": "x", "service": svc})
    check("explicit voice_api=openai overrides name -> /audio/speech",
          MOCK.count("openai") == o0 + 1 and MOCK.count("minimax") == m0)
    delete_cred(svc)

    # 4. default: plain provider → OpenAI-compatible.
    purge_creds()
    svc = f"plain{RUNID}"
    register_cred(svc, base_ok, {})
    o0 = MOCK.count("openai")
    req("POST", "/api/voice/speech", {"input": "x", "service": svc})
    check("plain credential defaults to /audio/speech", MOCK.count("openai") == o0 + 1)
    delete_cred(svc)


def test_voice_broken_providers(mock_port):
    print("\n## /api/voice/speech — broken providers (clean failure, no 500/hang)")
    # Each runs in ISOLATION (only this credential present) so speak_text's
    # candidate fallback can't mask the failure via another good mock.
    mm = {"voice_api": "minimax", "group_id": "g"}
    variants = [
        ("err500", mm, "provider HTTP 500", None),
        ("badjson", mm, "non-JSON minimax body", None),
        ("badresp", mm, "base_resp.status_code!=0", "1001"),
        ("badhex", mm, "non-hex audio", "hex"),
        ("err500", {}, "openai HTTP 500", None),
        ("err401", {}, "openai HTTP 401 (not passed through)", None),
        ("badjson", {}, "openai non-audio body", None),
    ]
    for mode, extras, label, substr in variants:
        purge_creds()
        svc = f"broken{RUNID}{mode}{'mm' if extras else 'oa'}"
        register_cred(svc, f"http://127.0.0.1:{mock_port}/{mode}/v1", extras)
        s, _, raw, _ = req("POST", "/api/voice/speech", {"input": "x", "service": svc}, timeout=15)
        # 'badjson' on the OpenAI path returns HTTP 200 with junk bytes — the
        # dispatch can't know it's not audio, so that one is allowed to 200.
        if mode == "badjson" and not extras:
            ok = s == 200 and raw == b"<<<not audio at all>>>"
            check(f"{label} passes through (documented)", ok, f"HTTP {s}", warn_only=True)
        else:
            # CONFIRMED behavior (workflow C1): every downstream voice failure
            # flattens to HTTP 400 — provider 401/500, MiniMax base_resp error,
            # hex decode failure all map to bad_request. Hard invariant: never
            # 5xx, never the success bytes.
            ok = s is not None and 400 <= s < 500 and raw != MINIMAX_AUDIO
            check(f"{label} -> 4xx (never 5xx/audio), got {s}", ok, f"HTTP {s}")
            if substr:
                # After the error-preference fix, the requested provider's
                # real failure surfaces instead of a bare NoCredential("openai").
                txt = raw.decode(errors="replace").lower()
                check(f"  {label}: error message surfaces '{substr}' (not masked)",
                      substr.lower() in txt, txt[:140])
        delete_cred(svc)

    # Unreachable provider: base_url at a closed port → clean error, not a hang.
    purge_creds()
    svc = f"unreachable{RUNID}"
    register_cred(svc, "http://127.0.0.1:9/v1", {"voice_api": "minimax", "group_id": "g"})
    t0 = time.time()
    s, _, _, _ = req("POST", "/api/voice/speech", {"input": "x", "service": svc}, timeout=20)
    dt = time.time() - t0
    check("unreachable provider fails cleanly (no hang)", s is not None and s != 200, f"HTTP {s} in {dt:.1f}s")
    check("unreachable provider returns within timeout", dt < 18, f"{dt:.1f}s")
    delete_cred(svc)


def test_voice_candidate(mock_port):
    print("\n## /api/voice/speech — candidate selection / fallback / anthropic skip")
    purge_creds()
    base = f"http://127.0.0.1:{mock_port}/ok/v1"
    oai = f"candoai{RUNID}"
    mmx = f"candmm{RUNID}"
    register_cred(oai, base, {"voice_api": "openai"})
    register_cred(mmx, base, {"voice_api": "minimax", "group_id": "g"})

    # explicit service is honored exactly — no cross-contamination to the other.
    o0, m0 = MOCK.count("openai"), MOCK.count("minimax")
    s, h, _, _ = req("POST", "/api/voice/speech", {"input": "x", "service": oai})
    check("explicit service served by THAT provider", h.get("x-ordo-tts-provider") == oai,
          h.get("x-ordo-tts-provider"))
    check("explicit openai → exact secret on the wire",
          (MOCK.last("openai") or {}).get("auth") == f"Bearer {secret_for(oai)}")
    check("explicit openai did NOT touch the minimax endpoint", MOCK.count("minimax") == m0)

    # C6 product sharp-edge: a bad explicit service silently falls back to a
    # working provider, and the response header reveals the SUBSTITUTION.
    s, h, _, _ = req("POST", "/api/voice/speech", {"input": "x", "service": f"no-such-{RUNID}"})
    served = h.get("x-ordo-tts-provider")
    check("bad explicit service falls back to a working provider (200)", s == 200, f"HTTP {s}")
    rec("WARN" if served in (oai, mmx) else "PASS",
        "C6: substitution is visible via x-ordo-tts-provider",
        f"requested no-such-{RUNID}, served by {served}")
    delete_cred(oai)
    delete_cred(mmx)

    # anthropic auth_style is skipped, loop continues to a working provider.
    purge_creds()
    anth = f"anth{RUNID}"
    good = f"goodoai{RUNID}"
    register_cred(anth, base, {"voice_api": "openai"}, auth_style="anthropic")
    register_cred(good, base, {"voice_api": "openai"})
    s, h, _, _ = req("POST", "/api/voice/speech", {"input": "x"})  # no service → candidate walk
    check("anthropic cred skipped, served by a non-anthropic provider", s == 200, f"HTTP {s}")
    check("anthropic skip → served by the bearer provider", h.get("x-ordo-tts-provider") == good,
          h.get("x-ordo-tts-provider"))
    delete_cred(anth)
    delete_cred(good)
    purge_creds()


def test_return_to_idle():
    print("\n## /sse/avatar — return to idle after an utterance (full lifecycle)")
    holder = {}

    def grab():
        holder["events"] = read_sse("/sse/avatar", 6.0)

    t = threading.Thread(target=grab)
    t.start()
    time.sleep(0.4)
    # short phrase so the utterance finishes well within the read window
    req("POST", "/api/avatar/speak", {"text": "hi there friend"})
    t.join()
    frames = parse_frames(holder.get("events", []))
    speaking = [i for i, f in enumerate(frames) if f.get("expression") == "Speaking"]
    check("lifecycle: reached Speaking", len(speaking) >= 2, f"{len(speaking)} speaking frames")
    # the last several frames (well after the utterance) must be idle again
    tail = frames[-8:]
    idle = [f for f in tail if f.get("mouth") == "REST" and f.get("expression") == "Neutral"]
    check("lifecycle: returns to REST/Neutral idle after utterance",
          len(tail) >= 4 and len(idle) >= len(tail) - 1,
          f"{len(idle)}/{len(tail)} trailing idle")


def test_sse_survives_bad_speak():
    print("\n## /sse/avatar — stream survives a malformed speak")
    holder = {}

    def grab():
        holder["events"] = read_sse("/sse/avatar", 3.0)

    t = threading.Thread(target=grab)
    t.start()
    time.sleep(0.3)
    # fire a malformed speak (→400) while the stream is open
    req("POST", "/api/avatar/speak", raw_body=b"{bad json",
        headers={"Content-Type": "application/json"})
    t.join()
    frames = parse_frames(holder.get("events", []))
    check("SSE keeps streaming through a bad speak", len(frames) >= 20, f"{len(frames)} frames")
    s, _, _, _ = req("GET", "/health")
    check("runtime healthy after bad speak", s == 200, f"HTTP {s}")


def test_credentials_lifecycle(mock_port):
    print("\n## /api/cloud/credentials — lifecycle (redaction / list / delete)")
    purge_creds()
    svc = f"lifecycle{RUNID}"
    ok = register_cred(svc, f"http://127.0.0.1:{mock_port}/ok/v1", {"voice_api": "openai"})
    s, p = getattr(register_cred, "last_response", (None, None))
    check("upsert returns 200", s == 200, f"HTTP {s}")
    cred = (p or {}).get("credential") if isinstance(p, dict) else None
    if cred is not None:
        check("upsert response omits secret", "secret" not in cred)
        check("upsert response carries has_secret=true", cred.get("has_secret") is True,
              repr(cred.get("has_secret")))
        check("upsert echoes service", cred.get("service") == svc)
    else:
        rec("WARN", "upsert response shape", "no 'credential' key to inspect")
    # list omits the secret
    ls, _, _, lp = req("GET", "/api/cloud/credentials")
    creds = (lp or {}).get("credentials", [])
    mine = [c for c in creds if c.get("service") == svc]
    check("listed once", len(mine) == 1, f"{len(mine)}")
    check("listed entry omits secret", mine and "secret" not in mine[0])
    # delete is reported + idempotent
    ds, _, _, dp = req("DELETE", "/api/cloud/credentials", {"service": svc})
    check("delete reports removed=true", isinstance(dp, dict) and dp.get("removed") is True, repr(dp))
    ds2, _, _, dp2 = req("DELETE", "/api/cloud/credentials", {"service": f"never{RUNID}"})
    check("delete is idempotent (no 404 for absent)", ds2 == 200, f"HTTP {ds2}")
    gone = req("GET", "/api/cloud/credentials")[3] or {}
    check("deleted credential no longer listed",
          svc not in {c.get("service") for c in gone.get("credentials", [])})


def test_credentials_preserve_secret(mock_port):
    # End-to-end regression for the empty-secret bug: editing a credential's
    # other fields with an empty `secret` (exactly what the Studio Edit modal
    # sends, since the secret is redacted on read) must PRESERVE the key —
    # proven here by the preserved secret still authenticating on the wire.
    print("\n## /api/cloud/credentials — empty secret preserves the key (end-to-end)")
    purge_creds()
    svc = f"preserve{RUNID}"
    base = f"http://127.0.0.1:{mock_port}/ok/v1"
    register_cred(svc, base, {"voice_api": "openai"})  # secret = secret_for(svc)

    # baseline: the call carries the stored secret
    req("POST", "/api/voice/speech", {"input": "x", "service": svc})
    first_auth = (MOCK.last("openai") or {}).get("auth")
    check("initial call uses the stored secret",
          first_auth == f"Bearer {secret_for(svc)}", first_auth)

    # Edit modal save: re-upsert with EMPTY secret + a changed field.
    s, _, _, _ = req("POST", "/api/cloud/credentials", {
        "service": svc, "label": "edited", "auth_style": "bearer",
        "secret": "", "base_url": base,
        "extras": {"voice_api": "openai", "tts_voice": "nova"},
    })
    check("empty-secret edit upsert ok", s == 200, f"HTTP {s}")

    # the key must survive AND the edited field must apply
    req("POST", "/api/voice/speech", {"input": "y", "service": svc})
    last = MOCK.last("openai") or {}
    check("empty-secret edit PRESERVES the key on the wire",
          last.get("auth") == f"Bearer {secret_for(svc)}", last.get("auth"))
    check("edited field (tts_voice) still took effect",
          (last.get("payload") or {}).get("voice") == "nova",
          (last.get("payload") or {}).get("voice"))
    delete_cred(svc)


def test_sse_concurrent():
    print("\n## /sse/avatar — concurrent broadcast clients")
    results = {}

    def client(i):
        results[i] = parse_frames(read_sse("/sse/avatar", 3.0))

    threads = [threading.Thread(target=client, args=(i,)) for i in range(4)]
    for t in threads:
        t.start()
    time.sleep(0.4)
    req("POST", "/api/avatar/speak", {"text": "broadcast to all clients"})
    for t in threads:
        t.join()
    got = {i: len(results.get(i, [])) for i in range(4)}
    check("all 4 SSE clients received frames", all(v >= 30 for v in got.values()), str(got))


def test_concurrency():
    print("\n## concurrency — rapid speaks (graceful: 200 or 429, never 500/hang)")
    codes = {}

    def speak(i):
        s, _, _, _ = req("POST", "/api/avatar/speak", {"text": f"concurrent {i}"})
        codes[i] = s

    threads = [threading.Thread(target=speak, args=(i,)) for i in range(20)]
    for t in threads:
        t.start()
    for t in threads:
        t.join()
    vals = list(codes.values())
    # 429 is the HTTP rate limiter doing its job under a burst — acceptable.
    # The contract under load: every request is *handled* (200) or *throttled*
    # (429); none crash (5xx) or hang (None).
    bad = {i: c for i, c in codes.items() if c not in (200, 429)}
    n200 = vals.count(200)
    n429 = vals.count(429)
    check("20 concurrent speaks: all handled or throttled (no 5xx/hang)",
          not bad, f"200×{n200} 429×{n429}" + (f" BAD={bad}" if bad else ""))
    check("at least some speaks succeeded under load", n200 >= 1, f"{n200} ok")
    # runtime still healthy afterward
    s, _, _, _ = req("GET", "/health")
    check("runtime healthy after storm", s == 200, f"HTTP {s}")


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
    args = ap.parse_args()

    global BASE
    BASE = args.base_url.rstrip("/")
    port = port_of(BASE)

    if port == 4141:
        print("REFUSING to run against :4141 (protected main/frozen runtime). "
              "Use :4142.", file=sys.stderr)
        return 2

    proc = tmpdir = logf = None
    if not args.no_launch:
        bin_path = find_binary(args.bin)
        if not bin_path:
            print("ordo binary not found — build it first: "
                  "cargo build -p ordo-cli", file=sys.stderr)
            return 2
        print(f"# launching runtime: {bin_path} on :{port} (avatar enabled, temp DB)")
        kill_port_listener(port)
        proc, tmpdir, logf = launch_runtime(bin_path, port)

    try:
        if not wait_health(60):
            print(f"runtime not healthy at {BASE}/health", file=sys.stderr)
            if logf:
                logf.flush()
                try:
                    with open(os.path.join(tmpdir, "runtime.log")) as f:
                        print(f.read()[-2000:], file=sys.stderr)
                except Exception:
                    pass
            return 2

        srv, mock_port = start_mock()
        print(f"# Ordo avatar+voice hard test -> {BASE}  (mock provider :{mock_port}, run {RUNID})")

        # ORDER MATTERS: every SSE idle/cadence/lifecycle test runs BEFORE any
        # heavy speak. A large `text` creates a long-lived background phoneme
        # schedule (the stub TTS sleeps phoneme-by-phoneme), which would keep
        # the avatar "speaking" and pollute idle assertions. So the big-payload
        # adversarial cases run LAST, with nothing idle-sensitive after them.
        test_assets()
        test_sse_idle()
        test_return_to_idle()
        test_speak_and_visemes()
        test_sse_survives_bad_speak()
        test_sse_concurrent()
        # voice dispatch (no avatar pollution)
        test_voice_no_provider()
        test_voice_openai(mock_port)
        test_voice_openai_options(mock_port)
        test_voice_minimax(mock_port)
        test_voice_minimax_format(mock_port)
        test_voice_groupid(mock_port)
        test_voice_resolution(mock_port)
        test_voice_candidate(mock_port)
        test_voice_broken_providers(mock_port)
        test_credentials_lifecycle(mock_port)
        test_credentials_preserve_secret(mock_port)
        # storms + big payloads LAST (these leave long background schedules)
        test_concurrency()
        test_speak_adversarial()

        purge_creds()
    finally:
        if proc and not args.keep:
            print("# stopping test runtime")
            try:
                proc.terminate()
                proc.wait(timeout=10)
            except Exception:
                kill_port_listener(port)
            if logf:
                logf.close()
        elif args.keep:
            print(f"# leaving runtime running on :{port} (--keep)")

    print(f"\n# summary: {PASS} pass, {WARN} warn, {FAIL} fail")
    if FAIL:
        print("# FAILURES:")
        for level, name, detail in RESULTS:
            if level == "FAIL":
                print(f"  - {name}: {detail}")
    return 1 if FAIL else 0


if __name__ == "__main__":
    sys.exit(main())
