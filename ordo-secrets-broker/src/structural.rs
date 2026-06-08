//! Structural output limits.
//!
//! A tool's output is capped by an explicit byte budget. If a
//! tool tries to emit more, we reject the output and emit
//! `SecretsStructuralRejection` on the bus. This is a structural
//! check: even if the canary is somehow avoided, a tool dumping
//! a 100 KB SSH key into a 256 B status field will fail here.

use ordo_protocol::StructuralOutputCheck;

#[derive(Debug, Clone)]
pub struct StructuralPolicy {
    pub byte_budget: u64,
    /// Optional operator-facing reason to surface when the cap
    /// is hit. If `None`, a default message is used.
    pub reject_reason: Option<String>,
}

pub fn enforce_structural_limit(
    tool_invocation_id: &str,
    actual_bytes: u64,
    policy: &StructuralPolicy,
) -> StructuralOutputCheck {
    let rejected = actual_bytes > policy.byte_budget;
    let reason = if rejected {
        Some(
            policy
                .reject_reason
                .clone()
                .unwrap_or_else(|| "output exceeded structural byte budget".to_string()),
        )
    } else {
        None
    };
    StructuralOutputCheck {
        tool_invocation_id: tool_invocation_id.to_string(),
        byte_budget: policy.byte_budget,
        actual_bytes,
        rejected,
        reason,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn within_budget_is_accepted() {
        let p = StructuralPolicy {
            byte_budget: 1024,
            reject_reason: None,
        };
        let check = enforce_structural_limit("inv-1", 512, &p);
        assert!(!check.rejected);
        assert!(check.reason.is_none());
    }

    #[test]
    fn over_budget_is_rejected_with_default_reason() {
        let p = StructuralPolicy {
            byte_budget: 1024,
            reject_reason: None,
        };
        let check = enforce_structural_limit("inv-1", 2048, &p);
        assert!(check.rejected);
        assert!(check.reason.unwrap().contains("structural byte budget"));
    }

    #[test]
    fn at_exact_budget_is_accepted() {
        let p = StructuralPolicy {
            byte_budget: 100,
            reject_reason: None,
        };
        let check = enforce_structural_limit("inv-1", 100, &p);
        assert!(!check.rejected);
    }
}
