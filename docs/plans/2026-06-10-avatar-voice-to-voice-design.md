# Avatar voice-to-voice — beta design (2026-06-10)

## Goal
Make the avatar a **voice-to-voice companion**: you talk to it, it transcribes,
the Ordo assistant thinks, and the avatar speaks the reply back with lip-sync.
Beta = the simplest version that's genuinely functional. Polish (tight sync,
barge-in, customization) comes later.

## Decisions (brainstormed)
- **Core purpose:** voice-to-voice companion (full loop), not just a talking face.
- **STT:** agnostic provider endpoint — OpenAI-compatible `POST {base_url}/audio/transcriptions`
  (Whisper shape). `base_url` can point at a **local** server (whisper.cpp /
  faster-whisper / LocalAI) **or** cloud. Mirrors the existing agnostic TTS.
- **Capture UX:** tap-to-start / tap-to-stop with a recording indicator.
- **The loop:** transcribed text becomes a real **assistant turn** (full brain —
  modes, memory, skills/tools) in a **dedicated voice session**, kept separate
  from the typed chat.

## What already exists (reused, ~80%)
- `ordo-tts` → `ordo-avatar` (30Hz frames + expression engine: Speaking /
  Thinking / Alarmed / Neutral) → `/sse/avatar` → canvas renderer.
- Agnostic **TTS**: `ordo-cloud::voice::synthesize` + `POST /api/voice/speech`
  (browser / OpenAI-compatible / MiniMax), with provider-aware defaults.
- `POST /api/avatar/speak` (drives lip-sync visemes).
- Assistant: `POST /api/assistant/sessions`, `POST /api/assistant/turn`,
  `GET /api/assistant/sessions/:id/stream` (SSE).
- Avatar tab (Preview + scaffolded Appearance/Persona/Skills), resizable pop-out.

## What's new (the input half)
1. **`ordo-cloud::voice::transcribe()`** — multipart `POST {base_url}/audio/transcriptions`
   (`file` + `model`, default `whisper-1`), bearer auth, parse `{ text }`.
   Beta = OpenAI-shape only (MiniMax-ASR deferred, like MiniMax-TTS was).
2. **`ordo-assistant::transcribe_audio()`** — picks an STT credential (same
   candidate pattern as `speak_text`), calls the dispatch.
3. **`POST /api/voice/transcribe`** — JSON in `{ audio_base64, format, service? }`
   → `{ text, provider, model }` out. Base64 JSON keeps Ordo's API consistent and
   testable; only the *provider* call is multipart.
4. **Avatar-page voice UI** (in `avatar.html`, so the tab preview + pop-out both
   get it): tap-to-talk mic button + indicator, `getUserMedia`/`MediaRecorder`,
   a 2-line transcript ("you:" / "Ordo:"), provider pickers reusing existing
   credentials (default to the cloud default).
5. **Dedicated voice assistant session** — created once, reused.

## The loop
```
tap → record → tap → POST /api/voice/transcribe ─► text
  └► POST /api/assistant/turn {voice_session, text} ─► reply (SSE stream)
        └► POST /api/voice/speech (audio, agnostic)   ┐ avatar speaks
           POST /api/avatar/speak (lip-sync visemes)  ┘ Thinking→Speaking→Neutral
```

## Beta scope (YAGNI)
- **In:** the full loop, OpenAI-shape STT, tap-to-talk, dedicated voice session,
  browser-default + agnostic TTS, a transcribe round-trip test.
- **Deferred:** MiniMax STT, VAD / wake-word, tight audio↔viseme sync,
  Appearance/Persona customization UI, barge-in / interrupt.

## Risks
- Mic permission inside the WebView2 pop-out — handle denial gracefully with a
  clear message.
- `getUserMedia` needs a secure context — `http://127.0.0.1:4141` (localhost) is
  treated as secure, so OK.
- Lip-sync (stub phoneme cadence) vs real provider audio are two timelines —
  roughly aligned (both scale with text length), not lip-perfect. Acceptable
  for beta.

## Testing
- Extend `scripts/ordo_avatar_test.py`: mock `/audio/transcriptions` provider →
  assert `POST /api/voice/transcribe` round-trips audio→text, sends the model +
  bearer secret, and reports the provider. Full mic loop is manual.
- Build gate `RUSTFLAGS="-D warnings"`; `cargo test` for the cloud/assistant crates.
