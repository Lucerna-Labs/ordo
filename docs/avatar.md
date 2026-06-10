# Avatar

A talking-head avatar that lip-syncs to spoken text, rendered in its own
**resizable pop-out window** (drag it onto a spare monitor). Ported into this
build from the `Ordo+webview+avatar` prototype, fitted into the existing UXI
without changing it — the only addition is one button.

## What it is

A sprite-atlas face driven by a phoneme stream on the bus:

```
text ──POST /api/avatar/speak──▶ ordo-tts (phoneme schedule, ~12/s)
                                    │  publishes ordo.tts.* on the bus
                                    ▼
                              ordo-avatar driver  ── composes AvatarFrame @30Hz
                                    │  publishes ordo.avatar.frame.emitted
                                    ▼
   /sse/avatar (SSE) ──▶ avatar.html canvas renderer  (expression → mouth → glitch)
```

- `ordo-tts` — turns text into a timed phoneme schedule and publishes
  `TtsUtteranceStarted` / `TtsPhonemeFrame` / `TtsUtteranceEnded`. The current
  text→phoneme map is a deliberately simple stub; the wire format is what
  matters, so a real engine can replace it without touching consumers.
- `ordo-avatar` — subscribes to the TTS stream and emits one `AvatarFrame`
  (`mouth` viseme + `expression` + `glitch`) every ~33ms.
- `ordo-control` — `/sse/avatar` mirrors the frame stream; `/api/avatar/speak`
  drives the visemes; `/avatar.html` + `/avatar/*` serve the pop-out page and
  its sprite atlas (embedded in the binary, same origin as the API so the
  page's relative URLs resolve with no CORS).

The frames are **resamplable** — a slow client just catches up on the next
tick, so there's no recovery handshake.

## Enabling it

The avatar driver is gated by `ORDO_ENABLE_AVATAR=1` (one ~30Hz task; off by
default to keep idle CPU at zero). Both launchers
(`Launch-Ordo-Studio.ps1`, `Launch-Ordo-Portable.ps1`) now set it, so it works
out of the box. To disable, set it to `0`.

The static page + SSE routes are always registered; without the driver the
window just shows the idle face.

## The pop-out window

In Ordo Studio, the **Bot button** next to the voice controls (the composer
toolbar, beside the "speak" toggle) opens the avatar.

- **In the Tauri desktop shell** it spawns a native, resizable OS window via
  `WebviewWindow` (label `avatar`) pointing at
  `http://127.0.0.1:4141/avatar.html`. Capability:
  `core:webview:allow-create-webview-window` (see
  `ordo-studio/src-tauri/capabilities/default.json`).
- **In a plain browser** it falls back to `window.open`.
- The window only renders the control-API page (plain `fetch` + `EventSource`,
  no Tauri IPC), so it needs no capabilities of its own.

You can also open it directly in any browser:
`http://127.0.0.1:4141/avatar.html`.

## Voice — provider-agnostic

Two independent channels, both timed off the same text:

1. **Visemes** (`POST /api/avatar/speak`) — always fire; drive the lip-sync.
2. **Audio** — selectable in the window via the **cloud voice** toggle:
   - **OFF (default, zero-config)** → the browser's `speechSynthesis` voice.
   - **ON** → `POST /api/voice/speech`, which routes to whatever voice provider
     is configured. On any failure it falls back to the browser voice.

The dispatch lives in `ordo-cloud::voice`:

| Provider kind | Endpoint | Notes |
|---|---|---|
| `OpenAiCompatible` (default) | `POST {base_url}/audio/speech` | OpenAI + any gateway that clones its contract. **Tested path.** |
| `MiniMax` | `POST {base_url}/t2a_v2` | Hex audio under `data.audio`; needs `extras.group_id`. Structurally complete, **not yet verified against a live MiniMax key.** |

Which API a credential speaks is resolved as:

1. Explicit `extras.voice_api` (`openai` / `openai_compatible` / `minimax`).
2. Inference — service name or `base_url` containing `minimax` → MiniMax.
3. Default → OpenAI-compatible.

Model/voice/format defaults are **per-provider** (`voice::defaults_for`), so an
OpenAI default like `alloy` never leaks into a MiniMax request.

### Configuring a voice provider

Add a cloud credential (Studio → Cloud, or the credentials API) for the
provider. Useful `extras` keys:

- `voice_api` — force the API shape (otherwise inferred).
- `tts_model`, `tts_voice`, `tts_format` — per-credential defaults.
- `group_id` — **required for MiniMax** (account GroupId).

Any OpenAI-compatible TTS endpoint works today by just setting the credential's
`base_url`. Adding a brand-new API shape is a two-line change in
`ordo-cloud/src/voice.rs`: a `VoiceApi` variant + one async wrapper —
`synthesize` is the single entry point, so no callers change.

## Files touched

- **New crates:** `ordo-tts`, `ordo-avatar` (+ workspace members).
- **Protocol:** `ordo-protocol/src/{avatar,tts}.rs` + 4 `OrdoMessage` variants;
  `ordo-router` (`message_kind`) and `ordo-classify` (Interactive class) arms.
- **Control:** `/sse/avatar`, `/api/avatar/speak`, `/avatar.html`, `/avatar/*`;
  raw bus handle + `TtsService` on `ControlApiState`.
- **Runtime:** spawns the `avatar` component when `ORDO_ENABLE_AVATAR=1`.
- **Voice:** `ordo-cloud/src/voice.rs` (agnostic dispatch + MiniMax adapter);
  `ordo-assistant` `speak_text` now dispatches through it.
- **UI:** one Bot button in `OrdoShell` + `openAvatarPopout()` in `api.ts`;
  `core:webview:allow-create-webview-window` capability.
- **Page:** `ordo-studio/public/avatar.html` (canvas renderer + cloud-voice
  toggle) and `ordo-studio/public/avatar/*` (atlas).

## Verifying

The hard-test harness is the source of truth:

```bash
python scripts/ordo_avatar_test.py     # self-launches a runtime on :4142
```

It is hermetic — launches its own runtime (temp DB, avatar enabled, never
touches `:4141`), stands up an in-process **mock voice provider** that speaks
both the OpenAI `/audio/speech` and MiniMax `/t2a_v2` contracts, and runs **134
checks**: static-asset/PNG validation, SSE cadence + enum validity + broadcast,
the full speak→Speaking→idle lifecycle, byte-exact OpenAI **and** MiniMax audio
round-trips (the MiniMax path's first real exercise — strict body assertions:
`stream`, `vol`, `pitch`, `sample_rate`, `channel`, GroupId, exact bearer
secret, no default leakage), voice_api resolution, every broken-provider variant
failing cleanly (never 5xx/hang), and credential lifecycle.

Manual smoke test (runtime up with `ORDO_ENABLE_AVATAR=1`):

```bash
curl -s http://127.0.0.1:4141/avatar.html | head -1
curl -s http://127.0.0.1:4141/avatar/avatar.json
curl -sN http://127.0.0.1:4141/sse/avatar &
curl -s -X POST http://127.0.0.1:4141/api/avatar/speak \
  -H 'content-type: application/json' -d '{"text":"hello ordo avatar"}'
```

## Limits / future work

- The text→phoneme map in `ordo-tts` is a stub. Real phonemization (and tighter
  audio↔viseme sync via Web Speech `boundary` events) is the next step. Note a
  very long `text` produces a correspondingly long background phoneme schedule
  (one detached task, mostly sleeping) — the real engine will chunk/bound this.
- The MiniMax adapter is now exercised end-to-end by the mock harness (request
  shaping, GroupId encoding, hex decode, byte round-trip), but still needs a
  **live key + `group_id`** to confirm against the real service.
- "Voice-to-voice" input (speech-in → response) is not wired — this build makes
  the **output voice** provider-agnostic. Speech-to-text input is a future
  extension that would feed the assistant the same way typed text does.

## Known sharp-edges (surfaced by the test harness)

- **Voice errors flatten to HTTP 400.** Every downstream voice failure (provider
  401/500, MiniMax `base_resp` error, hex-decode failure, connection refused)
  maps to `400` via `map_assistant_error`. The harness pins this and asserts the
  message now surfaces the *real* provider error (after a fix so a trailing
  `NoCredential("openai")` no longer masks it). Mapping these to `401/502/504`
  would need `AssistantError` to carry the upstream status — deferred.
- **Candidate fallback can substitute providers.** `speak_text` falls back to a
  hardcoded `"openai"` then to all credentials, so a request naming a missing
  provider is silently served by a *different* one; `x-ordo-tts-provider` reveals
  the substitution. Whether to surface that to the operator is a product call.
- **Empty secret over HTTP now preserves the key (fixed).** `POST
  /api/cloud/credentials` with `secret:""` previously overwrote the stored
  secret with empty (the "preserve on empty" contract held only on the bus
  path). `cloud_credentials_upsert` now maps empty→`None` to match
  `full_into_update`, so editing a voice provider's other fields no longer
  clears the key. Covered by `cloud_credentials_secret_tests` (unit) and an
  end-to-end case in the harness (preserved key still authenticates).
