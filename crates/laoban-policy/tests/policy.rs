//! Test IDs in comments refer to the laoban PoC test plan.

use laoban_policy::{
    approve, evaluate, ApprovalError, ChangeRequest, Decision, Layer, ProjectPolicy, RejectReason,
    ReviewMode, ReviewReason, Risk, Role,
};

const OWNER: &str = "local://u-owner";
const OWNER2: &str = "local://u-owner2";
const ADMIN: &str = "local://u-admin";
const DEV: &str = "local://u-dev";
const BIZ: &str = "local://u-biz";

fn dev_layer() -> Layer {
    Layer {
        name: "dev".into(),
        production: false,
    }
}

fn prod_layer() -> Layer {
    Layer {
        name: "prod".into(),
        production: true,
    }
}

fn policy() -> ProjectPolicy {
    ProjectPolicy {
        owners: vec![OWNER.into()],
        admins: vec![ADMIN.into()],
        developer: ReviewMode::Exempt,
        ..ProjectPolicy::default()
    }
}

fn request(author: &str, roles: &[Role], target: Layer, risk: Risk) -> ChangeRequest {
    ChangeRequest {
        author: author.into(),
        author_roles: roles.to_vec(),
        target,
        risk,
    }
}

fn reasons(d: &Decision) -> Vec<ReviewReason> {
    match d {
        Decision::ReviewRequired { reasons, .. } => reasons.clone(),
        other => panic!("expected review, got {other:?}"),
    }
}

// T-12: same change, business unit needs review, exempt developer deploys.
#[test]
fn business_unit_needs_review_exempt_developer_deploys() {
    let biz = evaluate(
        &policy(),
        &request(BIZ, &[Role::BusinessUnit], dev_layer(), Risk::none()),
    );
    assert_eq!(reasons(&biz), vec![ReviewReason::RoleRequiresReview]);

    let dev = evaluate(
        &policy(),
        &request(DEV, &[Role::Developer], dev_layer(), Risk::none()),
    );
    assert_eq!(dev, Decision::Deploy { notify: false });
}

#[test]
fn notify_only_developer_deploys_with_notification() {
    let p = ProjectPolicy {
        developer: ReviewMode::NotifyOnly,
        ..policy()
    };
    let d = evaluate(
        &p,
        &request(DEV, &[Role::Developer], dev_layer(), Risk::none()),
    );
    assert_eq!(d, Decision::Deploy { notify: true });
}

// T-23: phase-1 temporary rule, production always needs review.
#[test]
fn exempt_developer_still_needs_review_for_production() {
    let d = evaluate(
        &policy(),
        &request(DEV, &[Role::Developer], prod_layer(), Risk::none()),
    );
    assert_eq!(reasons(&d), vec![ReviewReason::ProductionLayer]);
}

#[test]
fn production_rule_can_be_turned_off_later() {
    let p = ProjectPolicy {
        production_always_requires_review: false,
        ..policy()
    };
    let d = evaluate(
        &p,
        &request(DEV, &[Role::Developer], prod_layer(), Risk::none()),
    );
    assert_eq!(d, Decision::Deploy { notify: false });
}

// U-04: roles stack as a union, the most permissive one wins.
#[test]
fn stacked_roles_take_the_most_permissive() {
    let d = evaluate(
        &policy(),
        &request(
            BIZ,
            &[Role::BusinessUnit, Role::Developer],
            dev_layer(),
            Risk::none(),
        ),
    );
    assert_eq!(d, Decision::Deploy { notify: false });
}

// U-09: new users are read-only.
#[test]
fn read_only_user_cannot_submit() {
    let d = evaluate(
        &policy(),
        &request(
            BIZ,
            &[Role::BusinessUnitReadOnly],
            dev_layer(),
            Risk::none(),
        ),
    );
    assert_eq!(
        d,
        Decision::Rejected {
            reason: RejectReason::ReadOnly
        }
    );
}

#[test]
fn read_only_plus_real_role_can_submit() {
    let d = evaluate(
        &policy(),
        &request(
            DEV,
            &[Role::BusinessUnitReadOnly, Role::Developer],
            dev_layer(),
            Risk::none(),
        ),
    );
    assert_eq!(d, Decision::Deploy { notify: false });
}

#[test]
fn user_without_roles_is_rejected() {
    let d = evaluate(&policy(), &request(DEV, &[], dev_layer(), Risk::none()));
    assert_eq!(
        d,
        Decision::Rejected {
            reason: RejectReason::NoRole
        }
    );
}

// T-24: no self-approval; falls back to admin when the owner is the author.
#[test]
fn owner_cannot_approve_own_change_and_admin_is_used() {
    let r = request(OWNER, &[Role::BusinessUnit], dev_layer(), Risk::none());
    match evaluate(&policy(), &r) {
        Decision::ReviewRequired { reviewers, .. } => {
            assert_eq!(reviewers, vec![ADMIN.to_string()])
        }
        other => panic!("expected review, got {other:?}"),
    }
    assert_eq!(
        approve(&policy(), &r, OWNER),
        Err(ApprovalError::SelfApproval)
    );
    assert_eq!(approve(&policy(), &r, ADMIN), Ok(()));
}

#[test]
fn other_owner_is_preferred_over_admin() {
    let p = ProjectPolicy {
        owners: vec![OWNER.into(), OWNER2.into()],
        ..policy()
    };
    let r = request(OWNER, &[Role::BusinessUnit], dev_layer(), Risk::none());
    match evaluate(&p, &r) {
        Decision::ReviewRequired { reviewers, .. } => {
            assert_eq!(reviewers, vec![OWNER2.to_string()])
        }
        other => panic!("expected review, got {other:?}"),
    }
    assert_eq!(approve(&p, &r, ADMIN), Err(ApprovalError::NotAReviewer));
    assert_eq!(approve(&p, &r, OWNER2), Ok(()));
}

#[test]
fn admin_cannot_approve_own_change_when_admin_is_the_only_fallback() {
    let p = ProjectPolicy {
        owners: vec![],
        admins: vec![ADMIN.into()],
        ..policy()
    };
    let r = request(ADMIN, &[Role::BusinessUnit], dev_layer(), Risk::none());
    assert_eq!(
        evaluate(&p, &r),
        Decision::Rejected {
            reason: RejectReason::NoEligibleReviewer
        }
    );
    assert_eq!(approve(&p, &r, ADMIN), Err(ApprovalError::SelfApproval));
}

#[test]
fn random_user_is_not_a_reviewer() {
    let r = request(BIZ, &[Role::BusinessUnit], dev_layer(), Risk::none());
    assert_eq!(
        approve(&policy(), &r, DEV),
        Err(ApprovalError::NotAReviewer)
    );
}

#[test]
fn change_that_needs_no_review_cannot_be_approved() {
    let r = request(DEV, &[Role::Developer], dev_layer(), Risk::none());
    assert!(matches!(
        approve(&policy(), &r, OWNER),
        Err(ApprovalError::NotReviewable(Decision::Deploy { .. }))
    ));
}

#[test]
fn all_reasons_are_reported() {
    let risk = Risk {
        database_schema_change: true,
        permission_change: true,
        ..Risk::none()
    };
    let d = evaluate(
        &policy(),
        &request(BIZ, &[Role::BusinessUnit], prod_layer(), risk),
    );
    assert_eq!(
        reasons(&d),
        vec![
            ReviewReason::RoleRequiresReview,
            ReviewReason::ProductionLayer,
            ReviewReason::DatabaseSchemaChange,
            ReviewReason::PermissionChange,
        ]
    );
}

#[test]
fn risk_based_review_applies_to_exempt_developers() {
    let risk = Risk {
        database_schema_change: true,
        ..Risk::none()
    };
    let d = evaluate(
        &policy(),
        &request(DEV, &[Role::Developer], dev_layer(), risk),
    );
    assert_eq!(reasons(&d), vec![ReviewReason::DatabaseSchemaChange]);
}
