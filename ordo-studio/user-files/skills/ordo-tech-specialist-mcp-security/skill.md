---
name: ordo-tech-specialist-mcp-security
description: MCP and plugin security playbook for Ordo Tech Specialist. Use in diagnostic mode when installing, inspecting, repairing, authorizing, quarantining, or explaining MCP servers, plugins, capabilities, expected lanes, trust state, and security audit findings.
category:
  - diagnostic
  - tech-specialist
  - mcp
  - security
available_to_modes:
  - diagnostic
risk_level: high
requires_tools: true
---

# Ordo Tech Specialist MCP Security

Use this skill for MCP servers, plugins, tool catalogs, capability drift,
quarantine, trust graduation, and audit findings.

## Security Posture

Treat every MCP server and plugin as a powerful untrusted subprocess until the
operator reviews it. MCP security is defense in depth, not a promise that native
code is safe.

The specialist must remember these controls:

- installed servers should have a signed or stable lockfile/provenance record
- new servers start disabled, untrusted, or quarantined until reviewed
- advertised tools must match expected lanes
- reserved core lanes must not be claimed by third-party plugins
- catalog drift requires re-authorization
- suspicious or changed servers should be quarantined, not silently trusted
- tool call arguments and results are scanned before and after execution
- security audit findings must be redacted before display or logging
- large post-call payloads may be exfiltration-shaped and should be reviewed

## Install And Review Checklist

Before enabling an MCP server or plugin:

1. Identify source, command, args, working directory, and version.
2. Inspect declared `expected_lanes` and reject broad or unrelated lanes.
3. Confirm required environment variables are minimal and do not expose secrets
   unless explicitly approved.
4. Check that the tool catalog matches the manifest intent.
5. Confirm the install lands disabled or untrusted first.
6. Explain the risk to the operator in plain language.
7. Enable only after explicit approval.
8. Record evidence and verification in diagnostic logs.

## Drift And Quarantine

If an MCP server changes advertised tools, expected lanes, command path, args,
version, hash, or provenance:

- do not auto-trust the new shape
- quarantine or keep disabled
- surface the exact drift without exposing secrets
- request re-authorization from the operator
- record the decision and verification

## Audit Findings

When reading security audit events:

- group by plugin/server, phase, capability, severity, verdict, and rule id
- explain `block`, `warn`, and `allow` in operator language
- never reveal the raw matched secret; use the redacted preview only
- distinguish prompt injection, path escape, secret leakage, PII, and oversized
  payload findings
- suggest the smallest remediation: disable, quarantine, narrow lanes, remove
  env access, or uninstall

## Forbidden Moves

- Do not invoke raw MCP tools directly from diagnostic mode.
- Do not let a plugin claim `cloud.`, `runtime.`, `filesystem.`, `memory.`,
  `knowledge.`, `self_heal.`, or other reserved core lanes unless it is a
  trusted first-party component with explicit operator approval.
- Do not paste secrets into plugin manifests.
- Do not treat scanner pass as proof of safety.
