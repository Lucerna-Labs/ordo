# Security

MCP plugins are arbitrary subprocesses that see every argument the
runtime sends them and produce every payload the runtime consumes.
That's powerful â€” and exactly the kind of thing a security layer
should be sitting in front of. This is that layer.

## The idea in one sentence

Every tool call into a gated provider has its arguments scanned
**before** execution and its result scanned **after**. Findings land
in a bounded audit log. Severe findings block the call outright; less
severe ones audit-and-pass.

## MCP Defense-In-Depth

Ordo's MCP security model follows a research-informed, modern
defense-in-depth stance: assume MCP servers and plugins are powerful,
untrusted capability providers until they are inspected, constrained, and
graduated by the operator.

The MCP security posture includes:

- signed lockfiles for installed server identity, provenance, and declared
  capability shape
- trust states instead of implicit trust
- quarantine for suspicious, changed, or unverified servers
- re-authorization when a server's advertised tool catalog drifts
- expected-lane enforcement so servers cannot quietly claim unrelated tools
- subprocess/worker isolation and sandboxing where available
- pre-call and post-call scanning around tool arguments and results
- redacted findings so audit reports do not expose matched secrets
- bounded audit logs visible through CLI, control API, and studio surfaces

These measures are not a claim that arbitrary native code becomes safe. They
are layered controls designed to reduce MCP supply-chain risk, prompt-injection
flow-through, secret leakage, capability drift, and accidental overreach.

## Data flow

```
                â”Œâ”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”
  brain â”€â”€â”€â”€â”€â”€â–º â”‚  SecurityGatedProvider             â”‚ â”€â”€â”€â”€â–º plugin MCP
  (tool call)   â”‚                                    â”‚       subprocess
                â”‚  1. pre-call scan (classifiers)    â”‚
                â”‚  2. policy verdict                 â”‚
                â”‚  3. audit event                    â”‚
                â”‚  4. block â†’ Failed                 â”‚
                â”‚  5. otherwise forward call         â”‚
                â”‚  6. post-call scan                 â”‚
                â”‚  7. policy verdict + audit         â”‚
                â”‚  8. block â†’ Failed                 â”‚
                â”‚  9. return result                  â”‚
                â””â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”˜
```

## Terminology

- **Classifier** â€” a pluggable scanner. Takes a `ScanInput` and returns
  zero or more `Finding`s. Built-ins are regex-based; future ones can
  be ML models or LLM-judges behind the same trait.
- **Finding** â€” a single classifier match. Has a stable `rule_id`
  (e.g. `secret.openai_key`), a severity, a human message, a
  **redacted** preview of what matched, and a JSON pointer into the
  payload so operators can see *where* the finding came from.
- **Severity** â€” `info`, `warn`, `error`. Purely descriptive; the
  policy decides what each severity means for behaviour.
- **Policy** â€” maps severity + rule_id + plugin to a `Verdict`.
- **Verdict** â€” `allow`, `warn`, `block`. `warn` still audits; `block`
  short-circuits the call with a `Failed` result.
- **Audit log** â€” a bounded ring buffer of `AuditEvent`s. Inspectable
  through the CLI, the control API, and the studio.

## Built-in classifiers

| Rule | Severity | Fires on |
|---|---|---|
| `secret.openai_key` | error | `sk-â€¦` OpenAI-style keys |
| `secret.anthropic_key` | error | `sk-ant-â€¦` Anthropic keys |
| `secret.aws_access_key` | error | `AKIA[A-Z0-9]{16}` |
| `secret.github_token` | error | `ghp_` / `github_pat_` tokens |
| `secret.slack_token` | error | `xox[baprs]-â€¦` |
| `secret.private_key_pem` | error | `-----BEGIN â€¦ PRIVATE KEY-----` headers |
| `secret.generic_bearer` | warn | `Authorization: Bearer <â€¦>` |
| `prompt.injection` | warn | "ignore previous instructions", "disregard prior", "you are now", "system prompt:" |
| `path.escape_parent` | warn | `../` and `..\` in pre-call payloads |
| `path.system_unix` | warn | `/etc/passwd`, `/etc/shadow`, `~/.ssh/` |
| `path.system_windows` | warn | `C:\Windows\`, `\Users\*\AppData\`, `\Users\*\.ssh\` |
| `pii.email` | info | email-shaped substrings |
| `pii.credit_card_shape` | warn | 16-digit number-shaped substrings (no Luhn check) |
| `volume.post_call_large` | warn | post-call payloads larger than 256 KB |

Severity is a classification signal, not a fixed consequence. The
policy decides what to do with it.

## Default policy

| Severity | Default verdict |
|---|---|
| `error` | **block** |
| `warn` | **warn** (audits, doesn't block) |
| `info` | **allow** (still audits if other rules fire alongside) |

Overrides, from highest to lowest precedence:

1. Plugin-specific rule verdict (`plugins.my-plugin.rule_verdicts["pii.email"] = "allow"`)
2. Plugin-specific mute (`plugins.my-plugin.muted_rules = ["pii.email"]`)
3. Global rule verdict (`rule_verdicts["secret.generic_bearer"] = "block"`)
4. Severity default

## What's gated today

**Every plugin** registered through `ordo-plugins` is wrapped in a
`SecurityGatedProvider` at runtime boot. Built-in capability providers
(`CloudOpsProvider`, etc.) are **not** gated â€”
they are first-party and live in the same trust boundary as the
runtime itself. The `SecurityGatedProvider::new` constructor takes any
`CapabilityProvider`, so extending the gate to first-party providers
is a one-line runtime change if needed later.

## Operator surfaces

### Control API

- `GET /api/security/audit?limit=100` â€” recent audit events, newest
  last
- `GET /api/security/rules` â€” classifier inventory

### CLI

```bash
ordo security audit --limit 50
ordo security rules
```

### Studio

New **Security** tab with two views:

- **Audit** â€” timeline of every flagged tool call, with plugin name,
  capability, phase (pre-call / post-call), verdict, per-finding
  rule_id + severity + message + redacted preview + JSON pointer
- **Rules** â€” classifier inventory card grid

## Redaction

Findings never carry the raw match text. The `match_preview` field
always goes through `redact_preview()` which shows at most the first
4 and last 4 characters of a match, middle blanked. Matches shorter
than 10 characters collapse to `***`. This holds for both the audit
log and the control-API responses â€” so screenshots, bug reports, and
log exports can't leak the value that tripped a secret rule.

## Writing a new classifier

The `Classifier` trait is small:

```rust
pub trait Classifier: Send + Sync {
    fn id(&self) -> &str;
    fn description(&self) -> &str;
    fn default_severity(&self) -> Severity;
    fn applies_to(&self, phase: Phase) -> bool { true }
    fn scan(&self, input: &ScanInput<'_>) -> Vec<Finding>;
}
```

To add one:

1. Implement the trait for your classifier struct.
2. Push a `Box::new(your_classifier)` onto the `Vec` returned by a
   new `default_classifiers()`-like function, or compose your own
   `Pipeline::new([...])`.
3. If you want runtime-wide coverage, extend `SecurityStack` to use
   your custom pipeline.

LLM-judge example (sketch):

```rust
struct LlmGuardClassifier {
    cloud: ordo_cloud::CloudHttp,
    credential: ordo_cloud::CloudCredential,
}

impl Classifier for LlmGuardClassifier {
    fn id(&self) -> &str { "llm.policy_guard" }
    fn description(&self) -> &str {
        "Uses a cloud LLM to judge whether the payload violates operator-supplied policy."
    }
    fn default_severity(&self) -> Severity { Severity::Warn }

    fn scan(&self, input: &ScanInput<'_>) -> Vec<Finding> {
        // Block in a blocking-classifier shim or adapt the trait to
        // async; real implementation elided.
        vec![]
    }
}
```

## Threat model

The security layer in this crate aims to catch **accidental and
opportunistic** problems that a bad-or-buggy MCP plugin might cause:

- Leaking an operator-supplied secret through a tool call argument
- Returning a huge payload that suggests exfiltration
- Prompting a downstream LLM to "ignore previous instructions"
- Reading files it shouldn't by passing `../` paths

It is **not** a defense against a determined adversary running code
on your machine. An MCP plugin is native code with network access. If
you don't trust the author, don't install the plugin. The security
layer is defense in depth, not sandbox escape prevention.

Hardening the subprocess itself (seccomp, AppArmor, container
isolation, capability dropping, filesystem jails) is the next layer
down â€” tracked in the roadmap.

## Roadmap

- **Per-plugin policy files** stored in SQLite so operator overrides
  survive restart
- **LLM-judge classifier** using a configured `cloud.*` credential
- **Embedding-based similarity** to catch close-paraphrase prompt
  injection
- **Allowlist mode**: a strict policy that `block`s everything except
  explicitly approved rule outcomes
- **Redaction rewrite**: `Redact` verdict that strips matches from the
  payload in place rather than blocking
- **Subprocess sandboxing** (seccomp on Linux, AppContainer on
  Windows, sandbox-exec on macOS)
