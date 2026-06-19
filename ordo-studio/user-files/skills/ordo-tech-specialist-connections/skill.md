---
name: ordo-tech-specialist-connections
description: Connection, API key, SSH, email/remote communication, and avatar setup playbook for Ordo Tech Specialist. Use in diagnostic mode when configuring provider keys, SSH/API descriptors, mailboxes, Signal/Telegram-style channels, or avatar behavior.
category:
  - diagnostic
  - tech-specialist
  - connections
  - avatar
  - email
available_to_modes:
  - diagnostic
risk_level: medium
requires_tools: true
---

# Ordo Tech Specialist Connections

Use this skill for provider/API keys, SSH descriptors, remote communication,
email client setup, and avatar configuration.

## Secret Boundary

Never ask the operator to paste secrets into chat. Guide them to the approved
credential UI or vault-backed install flow. When confirming setup, say whether a
secret exists or tested successfully, never what the secret value is.

## Provider And API Key Setup

1. Identify provider, base URL, model list, and intended mode.
2. Check whether the provider is local or cloud.
3. In diagnostic mode, cloud is denied unless explicitly allowed for this task.
4. Store credentials only through approved provider/profile setup.
5. Test with a safe non-sensitive request.
6. Set defaults only after operator approval.

## SSH Setup

Tech Specialist may set up host descriptors and auth metadata, but must not
execute remote shell commands.

For SSH:

- record host alias, hostname, port, username, key reference, and intended use
- never display private key material
- verify descriptor shape and connectivity only through approved non-destructive
  checks
- explain the risk before enabling use

## Email And Remote Communication

Ordo supports user-controlled mailboxes and remote communication channels.
Access must be explicit per account or channel:

- none: Ordo cannot read or write
- read: Ordo can inspect messages for approved workflows
- write: Ordo can draft/send only through approved controls

No SMS. For Signal, Telegram, or similar channels, require provider-specific
setup, explicit permissions, and clear separation between user accounts and
Ordo-owned remote-command channels.

## Avatar Setup

Avatar is a dedicated surface and mode. Tech Specialist may help configure:

- avatar model/brain endpoint
- persona and speaking style
- tool/skill access
- appearance clips and behavior states
- voice/TTS settings owned by the Avatar tab

Do not put avatar voice controls into the assistant chatbox. The chatbox uses
dictation only.

## Verification

After setup, show:

- what account/channel/profile exists
- access level: none, read, or write
- whether tests passed
- where the operator can change or revoke access
- what remains disabled
