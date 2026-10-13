#!/usr/bin/env bash
# Check deploy/m0 against a real Kubernetes API server:
#   - the Fleet GitRepo and GitRepoRestriction are valid against the Fleet CRDs
#     (server-side dry run; kubeconform has no Fleet schemas);
#   - namespace laoban-dev enforces PodSecurity "restricted": the pod shape of a
#     generated app is admitted and a pod without the restricted settings is not.
#
# Usage: test-m0.sh <fleet-crds.yaml>
# Needs: kubectl (override with KUBECTL="k3s kubectl ..."), a throwaway cluster
# where it is fine to install the Fleet CRDs and create the namespaces
# fleet-default and laoban-dev.
set -euo pipefail

FLEET_CRDS=${1:?usage: test-m0.sh <fleet-crds.yaml>}
DIR=$(cd "$(dirname "$0")" && pwd)
M0=$(dirname "$DIR")
read -r -a K <<< "${KUBECTL:-kubectl}"
fail() { echo "FAIL: $*" >&2; exit 1; }

echo "== Fleet CRDs"
"${K[@]}" apply --server-side -f "$FLEET_CRDS" >/dev/null
"${K[@]}" wait --for condition=established --timeout=60s \
  crd/gitrepos.fleet.cattle.io crd/gitreporestrictions.fleet.cattle.io
"${K[@]}" create namespace fleet-default --dry-run=client -o yaml | "${K[@]}" apply -f -

echo "== Fleet objects (server-side dry run, unknown fields rejected)"
"${K[@]}" apply --dry-run=server --validate=strict \
  -f "$M0/fleet-gitreporestriction.yaml" -f "$M0/fleet-gitrepo.yaml"

echo "== namespace laoban-dev"
"${K[@]}" apply --validate=strict -f "$M0/namespace-laoban-dev.yaml"
got=$("${K[@]}" get namespace laoban-dev -o jsonpath='{.metadata.labels.pod-security\.kubernetes\.io/enforce}')
[ "$got" = "restricted" ] || fail "laoban-dev enforce label: got '$got'"

echo "== generated-app pod shape is admitted"
"${K[@]}" apply --dry-run=server -f "$DIR/psa-restricted-pod.yaml"

echo "== pod without restricted settings is rejected"
if out=$("${K[@]}" apply --dry-run=server -f "$DIR/psa-baseline-pod.yaml" 2>&1); then
  fail "pod violating restricted was admitted: $out"
fi
grep -q 'violates PodSecurity "restricted' <<< "$out" || fail "unexpected error: $out"
echo "rejected as expected: ${out%%:*}..."

echo "OK"
