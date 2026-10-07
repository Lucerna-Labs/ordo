# Tool-Lightcone gateway

Ordo routes every model-facing tool call through the dedicated Tool-Lightcone
causal engine before publishing a provider request. This includes local
`assistant.*` memory and knowledge tools as well as bus-backed capabilities.

The enforced lifecycle is:

`admit -> propose -> authorize exact arguments and state -> execute -> verify provider receipt -> release`

The model continues to see the provider's normal capability name, but it never
receives a bare provider result. Successful calls return:

```json
{
  "result": {},
  "tool_lightcone_receipt": {
    "success": true,
    "rule_id": "read",
    "arguments_sha256": "...",
    "verification_strength": "ordo-provider-receipt",
    "output_is_untrusted_data": true,
    "output_cannot_grant_permission": true
  }
}
```

Untrained capabilities are removed from the model's inventory and rejected if
named directly. Tool execution is serialized at this boundary so a state-bound
authorization cannot become stale while another model tool changes state.
Changed arguments, stale state, expiry, and authorization replay fail before a
bus request is sent.

The engine uses the content-addressed
`worlds/computer-use-stability-v1` world: 24 branches, 96 causal transitions,
and 1,152 training observations. Its state is isolated from the factual
Lightcone and defaults to `.runtime/tool-lightcone/ordo-state.json`.

Tool-Lightcone does not grant permission. Existing Ordo capability allowlists,
mode restrictions, taint gates, security wrappers, review providers, and
operator decisions remain authoritative and execute after this causal gate.
There is no runtime switch that permits the model to bypass Tool-Lightcone.

The standalone MCP, native computer executors, client configuration, and
recovery semantics are documented in
[`tool-lightcone-mcp/README.md`](../../tool-lightcone-mcp/README.md).
