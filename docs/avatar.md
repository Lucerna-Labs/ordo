# Avatar

Avatar is Ordo's companion/voice surface. It is separate from the Assistant
chatbox.

## Boundary

- Assistant chatbox: text chat and dictation/STT.
- Avatar tab: voice output, avatar behavior, avatar appearance, TTS/STT
  provider settings, and pop-out companion behavior.

This keeps the main chat composer clean while still allowing a richer voice
and avatar experience.

## Runtime Pieces

- `ordo-tts`: text-to-phoneme and speech timing support.
- `ordo-avatar`: avatar frame/state driver.
- `ordo-control`: avatar and voice API endpoints.
- `ordo-studio`: Avatar tab and pop-out UI.

Representative local routes:

- `GET /avatar.html`
- `GET /avatar/*`
- `GET /sse/avatar`
- `POST /api/avatar/speak`
- `POST /api/voice/speech`
- `POST /api/voice/transcribe`

## Launch

Use the Servo launcher:

```powershell
.\Launch-Ordo-Servo.ps1
```

The old Studio/Portable launchers are retired.

## Provider-Agnostic Voice

Voice paths should be provider-agnostic:

- browser/local fallback where available
- OpenAI-compatible TTS/STT
- MiniMax-compatible TTS where configured
- future compatible providers through the provider layer

Provider defaults should not leak from one provider family into another.

## Avatar Brain

Avatar can use its own mode and model/provider choice so it can operate as a
companion without fighting the main Assistant for the same model endpoint.

The Avatar mode should remain concise and spoken-response friendly.

## Tech Specialist Support

Tech Specialist should be able to help configure Avatar because avatar setup is
complex:

- voice provider setup
- model/provider selection
- avatar behavior
- pop-out troubleshooting
- microphone troubleshooting
- local/cloud voice path diagnosis

Secrets and API keys still belong behind vault/UI paths.

## Safety

- Avatar starts muted unless the user enables microphone behavior.
- Microphone access should be explicit.
- Avatar voice should not hear itself.
- Voice errors should be visible and diagnosable.
- Voice controls belong on Avatar, not in the main Assistant composer.
