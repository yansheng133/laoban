mod common;

use common::*;
use laoban_ai::{
    parse_plan, resolve_model, validate_plan, ModelError, ModelSelection, PlanError, PlanPolicy,
};

fn check(json: &str) -> Result<(), PlanError> {
    let plan = parse_plan(json)?;
    validate_plan(&plan, &PlanPolicy::default(), CLUSTER)
}

#[test]
fn valid_plan_passes() {
    check(&plan_json(CLUSTER, "Deployment")).unwrap();
    // A single ```json fence is accepted.
    check(&format!("```json\n{}\n```", plan_json(CLUSTER, "Service"))).unwrap();
}

#[test]
fn schema_failures_are_rejected() {
    for bad in [
        "not json",
        "Sure! Here is the plan: {}",
        r#"{"summary":"x"}"#,
        r#"{"summary":"","steps":["a"]}"#,
        r#"{"summary":"x","steps":[]}"#,
        r#"{"summary":"x","steps":["a"],"run_shell":"rm -rf /"}"#,
        r#"{"summary":"x","steps":["a"],"files_to_change":[{"path":"a","action":"chmod"}]}"#,
    ] {
        assert!(
            matches!(check(bad), Err(PlanError::Schema(_))),
            "accepted: {bad}"
        );
    }
}

#[test]
fn disallowed_resources_are_rejected() {
    assert!(matches!(
        check(&plan_json("c-other02", "Deployment")),
        Err(PlanError::WrongCluster { .. })
    ));
    for kind in [
        "Secret",
        "secrets",
        "ClusterRoleBinding",
        "rolebindings",
        "ClusterRole",
    ] {
        assert!(
            matches!(
                check(&plan_json(CLUSTER, kind)),
                Err(PlanError::DeniedKind { .. })
            ),
            "accepted kind {kind}"
        );
    }
    let rbac_group = plan_json(CLUSTER, "Deployment").replace(
        r#""api_group":"apps""#,
        r#""api_group":"rbac.authorization.k8s.io""#,
    );
    assert!(matches!(
        check(&rbac_group),
        Err(PlanError::DeniedApiGroup { .. })
    ));
    let system_ns = plan_json(CLUSTER, "Deployment")
        .replace(r#""namespace":"demo""#, r#""namespace":"kube-system""#);
    assert!(matches!(
        check(&system_ns),
        Err(PlanError::DeniedNamespace { .. })
    ));
}

#[test]
fn unsafe_file_paths_are_rejected() {
    for path in ["/etc/passwd", "../x", "a/../../b", "C:\\x", "a//b", ""] {
        let json = plan_json(CLUSTER, "Deployment").replace("apps/demo/deployment.yaml", path);
        assert!(
            matches!(
                check(&json),
                Err(PlanError::UnsafePath { .. }) | Err(PlanError::Schema(_))
            ),
            "accepted path {path:?}"
        );
    }
}

#[test]
fn forced_model() {
    let sel = ModelSelection::Forced {
        model: MODEL_A.into(),
    };
    assert_eq!(resolve_model(&sel, None).unwrap(), MODEL_A);
    assert_eq!(resolve_model(&sel, Some(MODEL_A)).unwrap(), MODEL_A);
    assert_eq!(resolve_model(&sel, Some("  ")).unwrap(), MODEL_A);
    assert!(matches!(
        resolve_model(&sel, Some(MODEL_B)),
        Err(ModelError::NotAllowed { .. })
    ));
}

#[test]
fn user_choice_from_allow_list() {
    let sel = ModelSelection::UserChoice {
        allowed: vec![MODEL_A.into(), MODEL_B.into()],
        default: MODEL_A.into(),
    };
    assert_eq!(resolve_model(&sel, None).unwrap(), MODEL_A);
    assert_eq!(resolve_model(&sel, Some(MODEL_B)).unwrap(), MODEL_B);
    assert!(matches!(
        resolve_model(&sel, Some("model-c-test")),
        Err(ModelError::NotAllowed { .. })
    ));
}

#[test]
fn bad_model_config_is_rejected() {
    for sel in [
        ModelSelection::Forced { model: " ".into() },
        ModelSelection::UserChoice {
            allowed: vec![],
            default: MODEL_A.into(),
        },
        ModelSelection::UserChoice {
            allowed: vec![MODEL_B.into()],
            default: MODEL_A.into(),
        },
        ModelSelection::UserChoice {
            allowed: vec![MODEL_A.into(), "".into()],
            default: MODEL_A.into(),
        },
    ] {
        assert!(matches!(
            resolve_model(&sel, None),
            Err(ModelError::Config(_))
        ));
    }
}

#[test]
fn model_selection_config_deserializes() {
    let sel: ModelSelection = serde_json::from_str(
        r#"{"mode":"user_choice","allowed":["model-a-test"],"default":"model-a-test"}"#,
    )
    .unwrap();
    assert_eq!(resolve_model(&sel, None).unwrap(), MODEL_A);
}
