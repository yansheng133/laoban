//! T-14: what counts as "exposes externally".

use laoban_policy::{ExposureChange, Risk};
use serde_json::{json, Value};

fn deployment() -> Value {
    json!({"apiVersion": "apps/v1", "kind": "Deployment",
           "metadata": {"name": "web", "namespace": "shop"},
           "spec": {"replicas": 1}})
}

fn service(kind: &str) -> Value {
    json!({"apiVersion": "v1", "kind": "Service",
           "metadata": {"name": "web", "namespace": "shop"},
           "spec": {"type": kind, "ports": [{"port": 80}]}})
}

fn netpol(spec: Value) -> Value {
    json!({"apiVersion": "networking.k8s.io/v1", "kind": "NetworkPolicy",
           "metadata": {"name": "web", "namespace": "shop"},
           "spec": spec})
}

fn restricted_policy() -> Value {
    netpol(json!({"podSelector": {}, "policyTypes": ["Ingress"],
                  "ingress": [{"from": [{"podSelector": {"matchLabels": {"app": "api"}}}]}]}))
}

#[test]
fn text_only_change_has_no_risk() {
    let mut changed = deployment();
    changed["metadata"]["annotations"] = json!({"description": "新的說明文字"});
    let risk = Risk::from_manifests(&[deployment()], &[changed], false);
    assert_eq!(risk, Risk::none());
}

#[test]
fn new_ingress_is_exposure() {
    let ingress = json!({"apiVersion": "networking.k8s.io/v1", "kind": "Ingress",
                         "metadata": {"name": "web", "namespace": "shop"}, "spec": {}});
    let risk = Risk::from_manifests(&[deployment()], &[deployment(), ingress], false);
    assert_eq!(
        risk.exposure,
        vec![ExposureChange::NewIngress {
            namespace: "shop".into(),
            name: "web".into()
        }]
    );
}

#[test]
fn unchanged_existing_ingress_is_not_exposure() {
    let ingress = json!({"apiVersion": "networking.k8s.io/v1", "kind": "Ingress",
                         "metadata": {"name": "web", "namespace": "shop"}, "spec": {}});
    let risk = Risk::from_manifests(
        std::slice::from_ref(&ingress),
        std::slice::from_ref(&ingress),
        false,
    );
    assert!(risk.exposure.is_empty());
}

#[test]
fn new_gateway_route_is_exposure() {
    let route = json!({"apiVersion": "gateway.networking.k8s.io/v1", "kind": "HTTPRoute",
                       "metadata": {"name": "web", "namespace": "shop"}, "spec": {}});
    let risk = Risk::from_manifests(&[], &[route], false);
    assert!(matches!(
        risk.exposure.as_slice(),
        [ExposureChange::NewGatewayResource { kind, .. }] if kind == "HTTPRoute"
    ));
}

#[test]
fn service_becoming_loadbalancer_or_nodeport_is_exposure() {
    for t in ["LoadBalancer", "NodePort"] {
        let risk = Risk::from_manifests(&[service("ClusterIP")], &[service(t)], false);
        assert_eq!(
            risk.exposure,
            vec![ExposureChange::ServiceExposed {
                namespace: "shop".into(),
                name: "web".into(),
                service_type: t.into()
            }],
            "type {t}"
        );
    }
}

#[test]
fn new_service_without_type_is_clusterip_and_not_exposure() {
    let mut svc = service("ClusterIP");
    svc["spec"].as_object_mut().unwrap().remove("type");
    let risk = Risk::from_manifests(&[], &[svc], false);
    assert!(risk.exposure.is_empty());
}

#[test]
fn already_exposed_service_is_not_reported_again() {
    let risk = Risk::from_manifests(&[service("NodePort")], &[service("NodePort")], false);
    assert!(risk.exposure.is_empty());
}

#[test]
fn removing_a_network_policy_is_exposure() {
    let risk = Risk::from_manifests(&[restricted_policy()], &[], false);
    assert_eq!(
        risk.exposure,
        vec![ExposureChange::NetworkPolicyRemoved {
            namespace: "shop".into(),
            name: "web".into()
        }]
    );
}

#[test]
fn network_policy_opened_to_everyone_is_exposure() {
    let open_variants = [
        json!({"podSelector": {}, "ingress": [{}]}),
        json!({"podSelector": {}, "ingress": [{"from": []}]}),
        json!({"podSelector": {}, "ingress": [{"from": [{"ipBlock": {"cidr": "0.0.0.0/0"}}]}]}),
    ];
    for spec in open_variants {
        let risk = Risk::from_manifests(&[restricted_policy()], &[netpol(spec.clone())], false);
        assert_eq!(
            risk.exposure,
            vec![ExposureChange::NetworkPolicyAllowsAll {
                namespace: "shop".into(),
                name: "web".into(),
                direction: "ingress".into()
            }],
            "spec {spec}"
        );
    }
}

#[test]
fn open_cidr_with_exceptions_is_not_allow_all() {
    let spec = json!({"podSelector": {},
        "ingress": [{"from": [{"ipBlock": {"cidr": "0.0.0.0/0", "except": ["10.0.0.0/8"]}}]}]});
    let risk = Risk::from_manifests(&[restricted_policy()], &[netpol(spec)], false);
    assert!(risk.exposure.is_empty());
}

#[test]
fn egress_opened_is_reported_with_direction() {
    let spec = json!({"podSelector": {}, "egress": [{"to": [{"ipBlock": {"cidr": "::/0"}}]}]});
    let risk = Risk::from_manifests(&[restricted_policy()], &[netpol(spec)], false);
    assert!(risk.exposure.iter().any(|e| matches!(e,
        ExposureChange::NetworkPolicyAllowsAll { direction, .. } if direction == "egress")));
}

#[test]
fn rbac_changes_are_permission_changes() {
    let role = json!({"apiVersion": "rbac.authorization.k8s.io/v1", "kind": "Role",
                      "metadata": {"name": "reader", "namespace": "shop"},
                      "rules": [{"apiGroups": [""], "resources": ["pods"], "verbs": ["get"]}]});
    let mut widened = role.clone();
    widened["rules"][0]["verbs"] = json!(["get", "delete"]);

    assert!(
        Risk::from_manifests(&[], std::slice::from_ref(&role), false).permission_change,
        "added"
    );
    assert!(
        Risk::from_manifests(std::slice::from_ref(&role), &[widened], false).permission_change,
        "changed"
    );
    assert!(
        Risk::from_manifests(std::slice::from_ref(&role), &[], false).permission_change,
        "removed"
    );
    assert!(
        !Risk::from_manifests(
            std::slice::from_ref(&role),
            std::slice::from_ref(&role),
            false
        )
        .permission_change,
        "same"
    );
}

#[test]
fn schema_change_flag_is_passed_through() {
    assert!(Risk::from_manifests(&[], &[], true).database_schema_change);
}
