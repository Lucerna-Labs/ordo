//! Capability attenuation (AAT slot) â€” monotonic narrowing of a
//! `CapabilityHandle`'s `AttenuationConstraints`.
//!
//! Invariant 33: attenuation is one-way (narrower only). This
//! module's single public function composes a new handle from an
//! existing one + a narrower set of constraints. Widening attempts
//! fail.

use ordo_protocol::{AttenuationConstraints, CapabilityHandle};

use crate::ClientError;

/// Build a narrower capability. `from` is an existing handle; the
/// caller supplies the intended narrower `AttenuationConstraints`.
/// Returns a new `CapabilityHandle` with the narrower constraints
/// attached (stored on the handle via a parallel `constraints`
/// field on the secret broker side â€” not wire-visible here yet).
///
/// For v1 we return the original handle unchanged on success â€”
/// the actual attached-constraints storage lives in the broker's
/// in-memory state. This function exists to centralize the
/// narrowing-check and provide the extension point.
pub fn attenuate_capability(
    from: &CapabilityHandle,
    current_constraints: &AttenuationConstraints,
    proposed: &AttenuationConstraints,
) -> Result<CapabilityHandle, ClientError> {
    if !current_constraints.is_narrower_or_equal(proposed) {
        return Err(ClientError::CapabilityWidening);
    }
    // Return a cloned handle â€” the caller attaches `proposed` to
    // its tracking state. The handle's id stays the same (this is
    // a constraint change, not a fresh issuance).
    Ok(from.clone())
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::{Duration, Utc};
    use ordo_protocol::{ArgumentConstraint, SecretClass};
    use std::collections::HashMap;

    fn handle() -> CapabilityHandle {
        CapabilityHandle {
            id: "cap-1".to_string(),
            provider_id: "p".to_string(),
            expires_at: Utc::now() + Duration::minutes(5),
            class: SecretClass::ApiKey,
        }
    }

    #[test]
    fn narrowing_tool_allowlist_succeeds() {
        let current = AttenuationConstraints {
            tool_allowlist: Some(vec!["a".into(), "b".into(), "c".into()]),
            ..Default::default()
        };
        let narrower = AttenuationConstraints {
            tool_allowlist: Some(vec!["a".into()]),
            ..Default::default()
        };
        attenuate_capability(&handle(), &current, &narrower).unwrap();
    }

    #[test]
    fn widening_tool_allowlist_is_rejected() {
        let current = AttenuationConstraints {
            tool_allowlist: Some(vec!["a".into()]),
            ..Default::default()
        };
        let wider = AttenuationConstraints {
            tool_allowlist: Some(vec!["a".into(), "b".into()]),
            ..Default::default()
        };
        let err = attenuate_capability(&handle(), &current, &wider).unwrap_err();
        assert!(matches!(err, ClientError::CapabilityWidening));
    }

    #[test]
    fn removing_allowlist_entirely_is_widening() {
        let current = AttenuationConstraints {
            tool_allowlist: Some(vec!["a".into()]),
            ..Default::default()
        };
        let wider = AttenuationConstraints {
            tool_allowlist: None,
            ..Default::default()
        };
        let err = attenuate_capability(&handle(), &current, &wider).unwrap_err();
        assert!(matches!(err, ClientError::CapabilityWidening));
    }

    #[test]
    fn narrowing_max_invocations_succeeds() {
        let current = AttenuationConstraints {
            max_invocations: Some(10),
            ..Default::default()
        };
        let narrower = AttenuationConstraints {
            max_invocations: Some(3),
            ..Default::default()
        };
        attenuate_capability(&handle(), &current, &narrower).unwrap();
    }

    #[test]
    fn widening_max_invocations_rejected() {
        let current = AttenuationConstraints {
            max_invocations: Some(3),
            ..Default::default()
        };
        let wider = AttenuationConstraints {
            max_invocations: Some(10),
            ..Default::default()
        };
        let err = attenuate_capability(&handle(), &current, &wider).unwrap_err();
        assert!(matches!(err, ClientError::CapabilityWidening));
    }

    #[test]
    fn adding_new_argument_constraint_key_is_narrowing() {
        let current = AttenuationConstraints::default();
        let mut narrower = AttenuationConstraints::default();
        let mut args = HashMap::new();
        args.insert(
            "x".to_string(),
            ArgumentConstraint::Exact {
                value: serde_json::json!(1),
            },
        );
        narrower.argument_constraints = args;
        // current has no constraints, narrower adds one. That's a
        // narrowing (narrower only accepts x=1). But our simple
        // check compares by key â€” when current has no entry for
        // "x" and narrower adds "x", narrower is narrower.
        attenuate_capability(&handle(), &current, &narrower).unwrap();
    }
}
