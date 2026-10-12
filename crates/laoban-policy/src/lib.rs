//! Deployment review policy for laoban.
//!
//! Given a change request (who, which roles, which environment layer, what
//! the change touches), [`evaluate`] decides whether it can be deployed
//! directly, needs human review and by whom, or is rejected outright.
//! [`approve`] checks that a reviewer is allowed to approve it.
//!
//! Phase-1 rules encoded here:
//!
//! * Roles are scoped to a Project and stack as a union: the most permissive
//!   role the author holds decides the role-based requirement.
//! * Business-unit users always need review. Developers and platform
//!   engineers follow the Project's configuration.
//! * Temporary rule: anything going to a production layer always needs
//!   human review, whatever the author's roles
//!   ([`ProjectPolicy::production_always_requires_review`]).
//! * Changes that expose a workload externally, change a database schema or
//!   change permissions need review ([`ProjectPolicy::risk_based_review`]).
//! * Nobody can approve their own change. Reviewers are the Project owners
//!   other than the author; when there are none, the admins other than the
//!   author.
//! * New users with only the read-only role cannot submit changes.

use serde::{Deserialize, Serialize};
use thiserror::Error;

pub mod risk;

pub use risk::{ExposureChange, Risk};

/// A Rancher principal ID, e.g. `local://u-abc12`. Using the principal ID
/// keeps audit records traceable to a real identity.
pub type Principal = String;

/// Roles a user can hold within a Project. They stack.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Role {
    /// Default for new users: can look, cannot submit.
    BusinessUnitReadOnly,
    BusinessUnit,
    Developer,
    PlatformEngineer,
}

/// Role-based review requirement, ordered from most to least permissive.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReviewMode {
    /// Deploy without review.
    Exempt,
    /// Deploy without review, but notify the reviewers.
    NotifyOnly,
    /// Human review required.
    Review,
}

/// Per-Project policy.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct ProjectPolicy {
    pub owners: Vec<Principal>,
    pub admins: Vec<Principal>,
    pub developer: ReviewMode,
    pub platform_engineer: ReviewMode,
    /// Phase-1 temporary rule. Keep `true` until environment-based review
    /// replaces it.
    pub production_always_requires_review: bool,
    /// Review changes that expose workloads, change schemas or permissions.
    pub risk_based_review: bool,
}

impl Default for ProjectPolicy {
    fn default() -> Self {
        Self {
            owners: Vec::new(),
            admins: Vec::new(),
            developer: ReviewMode::Review,
            platform_engineer: ReviewMode::Review,
            production_always_requires_review: true,
            risk_based_review: true,
        }
    }
}

/// An environment layer a change is deployed to.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Layer {
    pub name: String,
    pub production: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChangeRequest {
    pub author: Principal,
    pub author_roles: Vec<Role>,
    pub target: Layer,
    pub risk: Risk,
}

/// Why a change needs review. All applicable reasons are reported.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReviewReason {
    RoleRequiresReview,
    ProductionLayer,
    ExposesExternally,
    DatabaseSchemaChange,
    PermissionChange,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RejectReason {
    /// The author has no role in this Project.
    NoRole,
    /// The author only has the read-only role.
    ReadOnly,
    /// Review is required but nobody other than the author can approve.
    NoEligibleReviewer,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "decision")]
pub enum Decision {
    Deploy {
        notify: bool,
    },
    ReviewRequired {
        reviewers: Vec<Principal>,
        reasons: Vec<ReviewReason>,
    },
    Rejected {
        reason: RejectReason,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum ApprovalError {
    #[error("authors cannot approve their own changes")]
    SelfApproval,
    #[error("this principal is not an eligible reviewer for the change")]
    NotAReviewer,
    #[error("this change does not need review: {0:?}")]
    NotReviewable(Decision),
}

fn role_mode(policy: &ProjectPolicy, role: Role) -> Option<ReviewMode> {
    match role {
        Role::BusinessUnitReadOnly => None,
        Role::BusinessUnit => Some(ReviewMode::Review),
        Role::Developer => Some(policy.developer),
        Role::PlatformEngineer => Some(policy.platform_engineer),
    }
}

/// Reviewers for a change: owners other than the author, falling back to
/// admins other than the author. Order is preserved and duplicates removed.
pub fn eligible_reviewers(policy: &ProjectPolicy, author: &str) -> Vec<Principal> {
    let pick = |list: &[Principal]| -> Vec<Principal> {
        let mut out: Vec<Principal> = Vec::new();
        for p in list {
            if p != author && !out.contains(p) {
                out.push(p.clone());
            }
        }
        out
    };
    let owners = pick(&policy.owners);
    if owners.is_empty() {
        pick(&policy.admins)
    } else {
        owners
    }
}

/// Decide what happens to a change request.
pub fn evaluate(policy: &ProjectPolicy, request: &ChangeRequest) -> Decision {
    if request.author_roles.is_empty() {
        return Decision::Rejected {
            reason: RejectReason::NoRole,
        };
    }
    // Union of roles: the most permissive submitting role wins.
    let Some(mode) = request
        .author_roles
        .iter()
        .filter_map(|r| role_mode(policy, *r))
        .min()
    else {
        return Decision::Rejected {
            reason: RejectReason::ReadOnly,
        };
    };

    let mut reasons = Vec::new();
    if mode == ReviewMode::Review {
        reasons.push(ReviewReason::RoleRequiresReview);
    }
    if request.target.production && policy.production_always_requires_review {
        reasons.push(ReviewReason::ProductionLayer);
    }
    if policy.risk_based_review {
        if !request.risk.exposure.is_empty() {
            reasons.push(ReviewReason::ExposesExternally);
        }
        if request.risk.database_schema_change {
            reasons.push(ReviewReason::DatabaseSchemaChange);
        }
        if request.risk.permission_change {
            reasons.push(ReviewReason::PermissionChange);
        }
    }

    if reasons.is_empty() {
        return Decision::Deploy {
            notify: mode == ReviewMode::NotifyOnly,
        };
    }

    let reviewers = eligible_reviewers(policy, &request.author);
    if reviewers.is_empty() {
        return Decision::Rejected {
            reason: RejectReason::NoEligibleReviewer,
        };
    }
    Decision::ReviewRequired { reviewers, reasons }
}

/// Check that `reviewer` may approve `request`.
pub fn approve(
    policy: &ProjectPolicy,
    request: &ChangeRequest,
    reviewer: &str,
) -> Result<(), ApprovalError> {
    if reviewer == request.author {
        return Err(ApprovalError::SelfApproval);
    }
    match evaluate(policy, request) {
        Decision::ReviewRequired { reviewers, .. } => {
            if reviewers.iter().any(|r| r == reviewer) {
                Ok(())
            } else {
                Err(ApprovalError::NotAReviewer)
            }
        }
        other => Err(ApprovalError::NotReviewable(other)),
    }
}
