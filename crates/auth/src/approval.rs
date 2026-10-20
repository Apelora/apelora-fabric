// SPDX-License-Identifier: AGPL-3.0-only
//! Four-eyes approval for risky actions (machine deletion, hardware power, unsafe evacuation,
//! sensitive cluster changes). A request names the exact action and target; a different person
//! with the approval permission decides; the approval can be consumed once, by the requester,
//! for exactly that action and target, before it expires.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "utoipa", derive(utoipa::ToSchema))]
#[serde(rename_all = "kebab-case")]
pub enum ApprovalState {
    Pending,
    Approved,
    Rejected,
    Cancelled,
    Consumed,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Approval {
    pub action: String,
    pub target: String,
    /// Stable id of who asked (for example `user:<uuid>`).
    pub requester: String,
    pub approver: Option<String>,
    pub state: ApprovalState,
    /// Seconds since the epoch. A pending request expires `request_ttl` after creation; an
    /// approved one must be used before `expires_at`.
    pub expires_at: i64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ApprovalError {
    NotPending,
    NotApproved,
    Expired,
    SameAsRequester,
    ApproverNotPermitted,
    RequesterOnly,
    Mismatch,
}

impl std::fmt::Display for ApprovalError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            ApprovalError::NotPending => "the request has already been decided",
            ApprovalError::NotApproved => "the request is not approved",
            ApprovalError::Expired => "the request has expired",
            ApprovalError::SameAsRequester => {
                "a request must be approved by someone other than the requester"
            }
            ApprovalError::ApproverNotPermitted => "you may not approve this kind of request",
            ApprovalError::RequesterOnly => "only the person who asked can use this approval",
            ApprovalError::Mismatch => "the approval is for a different action or target",
        })
    }
}

/// Pending lifetime of a request, and how long an approval stays usable.
pub const REQUEST_TTL_SECS: i64 = 24 * 3600;
pub const APPROVAL_TTL_SECS: i64 = 30 * 60;

impl Approval {
    pub fn request(action: &str, target: &str, requester: &str, now: i64) -> Self {
        Self {
            action: action.to_owned(),
            target: target.to_owned(),
            requester: requester.to_owned(),
            approver: None,
            state: ApprovalState::Pending,
            expires_at: now + REQUEST_TTL_SECS,
        }
    }

    /// Approves or rejects. `approver_may` is the caller's own check that the approver holds the
    /// approval permission for this action in the right scope.
    pub fn decide(
        &self,
        approver: &str,
        approver_may: bool,
        approve: bool,
        now: i64,
    ) -> Result<Approval, ApprovalError> {
        if self.state != ApprovalState::Pending {
            return Err(ApprovalError::NotPending);
        }
        if now >= self.expires_at {
            return Err(ApprovalError::Expired);
        }
        if approver == self.requester {
            return Err(ApprovalError::SameAsRequester);
        }
        if !approver_may {
            return Err(ApprovalError::ApproverNotPermitted);
        }
        let mut next = self.clone();
        next.approver = Some(approver.to_owned());
        if approve {
            next.state = ApprovalState::Approved;
            next.expires_at = now + APPROVAL_TTL_SECS;
        } else {
            next.state = ApprovalState::Rejected;
        }
        Ok(next)
    }

    pub fn cancel(&self, who: &str) -> Result<Approval, ApprovalError> {
        if self.state != ApprovalState::Pending {
            return Err(ApprovalError::NotPending);
        }
        if who != self.requester {
            return Err(ApprovalError::RequesterOnly);
        }
        Ok(Approval {
            state: ApprovalState::Cancelled,
            ..self.clone()
        })
    }

    /// Spends the approval for `action` on `target`. Call it in the same transaction that
    /// carries the action out, so a crash cannot leave a spent approval without the action.
    pub fn consume(
        &self,
        action: &str,
        target: &str,
        who: &str,
        now: i64,
    ) -> Result<Approval, ApprovalError> {
        if self.state != ApprovalState::Approved {
            return Err(ApprovalError::NotApproved);
        }
        if now >= self.expires_at {
            return Err(ApprovalError::Expired);
        }
        if who != self.requester {
            return Err(ApprovalError::RequesterOnly);
        }
        if action != self.action || target != self.target {
            return Err(ApprovalError::Mismatch);
        }
        Ok(Approval {
            state: ApprovalState::Consumed,
            ..self.clone()
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const NOW: i64 = 5_000;

    fn pending() -> Approval {
        Approval::request("hardware.delete", "machine:rack1-07", "user:a", NOW)
    }

    #[test]
    fn the_requester_cannot_approve_their_own_request() {
        assert_eq!(
            pending().decide("user:a", true, true, NOW),
            Err(ApprovalError::SameAsRequester)
        );
    }

    #[test]
    fn only_someone_permitted_can_decide() {
        assert_eq!(
            pending().decide("user:b", false, true, NOW),
            Err(ApprovalError::ApproverNotPermitted)
        );
        let ok = pending()
            .decide("user:b", true, true, NOW)
            .expect("approved");
        assert_eq!(
            (ok.state, ok.approver.as_deref()),
            (ApprovalState::Approved, Some("user:b"))
        );
    }

    #[test]
    fn an_approval_is_single_use_exact_and_requester_only() {
        let ok = pending()
            .decide("user:b", true, true, NOW)
            .expect("approved");
        assert_eq!(
            ok.consume("hardware.power", "machine:rack1-07", "user:a", NOW),
            Err(ApprovalError::Mismatch)
        );
        assert_eq!(
            ok.consume("hardware.delete", "machine:rack1-08", "user:a", NOW),
            Err(ApprovalError::Mismatch)
        );
        assert_eq!(
            ok.consume("hardware.delete", "machine:rack1-07", "user:c", NOW),
            Err(ApprovalError::RequesterOnly)
        );
        let used = ok
            .consume("hardware.delete", "machine:rack1-07", "user:a", NOW + 1)
            .expect("consumed");
        assert_eq!(used.state, ApprovalState::Consumed);
        assert_eq!(
            used.consume("hardware.delete", "machine:rack1-07", "user:a", NOW + 2),
            Err(ApprovalError::NotApproved),
            "single use"
        );
    }

    #[test]
    fn approvals_and_requests_expire() {
        let ok = pending()
            .decide("user:b", true, true, NOW)
            .expect("approved");
        assert_eq!(
            ok.consume(
                "hardware.delete",
                "machine:rack1-07",
                "user:a",
                NOW + APPROVAL_TTL_SECS
            ),
            Err(ApprovalError::Expired)
        );
        assert_eq!(
            pending().decide("user:b", true, true, NOW + REQUEST_TTL_SECS),
            Err(ApprovalError::Expired)
        );
    }

    #[test]
    fn rejected_pending_and_cancelled_requests_cannot_be_used() {
        let rejected = pending()
            .decide("user:b", true, false, NOW)
            .expect("rejected");
        assert_eq!(rejected.state, ApprovalState::Rejected);
        assert_eq!(
            rejected.consume("hardware.delete", "machine:rack1-07", "user:a", NOW),
            Err(ApprovalError::NotApproved)
        );
        assert_eq!(
            pending().consume("hardware.delete", "machine:rack1-07", "user:a", NOW),
            Err(ApprovalError::NotApproved)
        );
        assert_eq!(
            pending().cancel("user:b"),
            Err(ApprovalError::RequesterOnly)
        );
        let c = pending().cancel("user:a").expect("cancelled");
        assert_eq!(
            c.decide("user:b", true, true, NOW),
            Err(ApprovalError::NotPending)
        );
    }
}
