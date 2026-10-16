#!/usr/bin/env bash
# Check the laoban CRDs against a real Kubernetes API server (structural
# schema, defaults, enums and CEL rules, which kubeconform cannot check).
#
# Usage: test-crds.sh <rendered-crds.yaml>
# Needs: kubectl (override with KUBECTL="k3s kubectl ..."), a reachable cluster
# where it is fine to create the laoban-system namespace, python3.
set -euo pipefail

CRDS=${1:?usage: test-crds.sh <rendered-crds.yaml>}
DIR=$(cd "$(dirname "$0")" && pwd)
read -r -a K <<< "${KUBECTL:-kubectl}"
NS=laoban-system
fail() { echo "FAIL: $*" >&2; exit 1; }

"${K[@]}" apply --server-side -f "$CRDS"
"${K[@]}" wait --for condition=established --timeout=60s \
  crd/projectpolicies.laoban.yansheng133.github.io \
  crd/changerequests.laoban.yansheng133.github.io \
  crd/architectureversions.laoban.yansheng133.github.io \
  crd/rounds.laoban.yansheng133.github.io
"${K[@]}" create namespace "$NS" --dry-run=client -o yaml | "${K[@]}" apply -f -

echo "== valid examples are accepted"
"${K[@]}" apply -f "$DIR/examples.yaml"

echo "== defaults are applied"
got=$("${K[@]}" -n "$NS" get projectpolicy shop -o jsonpath='{.spec.productionAlwaysRequiresReview}/{.spec.riskBasedReview}')
[ "$got" = "true/true" ] || fail "ProjectPolicy defaults: got $got"
got=$("${K[@]}" -n "$NS" get changerequest shop-v3 -o jsonpath='{.spec.risk.permissionChange}')
[ "$got" = "false" ] || fail "ChangeRequest risk default: got $got"

echo "== a Round keeps the backend's data as written"
got=$("${K[@]}" -n "$NS" get round shop-r-20261016-0a1b2c -o jsonpath='{.spec.round.opinions[0].text}')
[ "$got" = "清單要能排序" ] || fail "Round spec.round not kept: got $got"

echo "== invalid examples are rejected"
tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' EXIT
python3 - "$DIR/invalid-examples.yaml" "$tmp" <<'EOF'
import sys
docs = [d for d in open(sys.argv[1], encoding="utf-8").read().split("\n---\n") if "kind:" in d]
for i, d in enumerate(docs):
    open(f"{sys.argv[2]}/{i}.yaml", "w", encoding="utf-8").write(d)
EOF
for f in "$tmp"/*.yaml; do
  if "${K[@]}" create --dry-run=server -f "$f" >/dev/null 2>"$tmp/err"; then
    fail "accepted invalid object: $(grep -m1 'name:' "$f")"
  fi
  echo "rejected as expected: $(grep -m1 '^  name:' "$f" | tr -d ' ') ($(head -c 160 "$tmp/err"))"
done

echo "== spec is immutable"
if "${K[@]}" -n "$NS" patch architectureversion shop-v1 --type merge -p '{"spec":{"author":"local://u-other"}}' 2>"$tmp/err"; then
  fail "ArchitectureVersion spec was changed"
fi
grep -q immutable "$tmp/err" || fail "unexpected error: $(cat "$tmp/err")"
if "${K[@]}" -n "$NS" patch changerequest shop-v3 --type merge -p '{"spec":{"target":{"production":false}}}' 2>"$tmp/err"; then
  fail "ChangeRequest spec was changed"
fi
grep -q immutable "$tmp/err" || fail "unexpected error: $(cat "$tmp/err")"

echo "== status rules"
if "${K[@]}" -n "$NS" patch changerequest shop-v3 --subresource status --type merge \
    -p '{"status":{"decision":"Rejected"}}' 2>"$tmp/err"; then
  fail "Rejected without rejectReason was accepted"
fi
"${K[@]}" -n "$NS" patch changerequest shop-v3 --subresource status --type merge \
  -p '{"status":{"decision":"ReviewRequired","reviewers":["local://u-owner"],"reasons":["ProductionLayer","ExposesExternally","DatabaseSchemaChange"]}}'
got=$("${K[@]}" -n "$NS" get changerequest shop-v3 -o jsonpath='{.status.decision}')
[ "$got" = "ReviewRequired" ] || fail "status decision: got $got"

echo "== cleanup"
"${K[@]}" delete -f "$DIR/examples.yaml" --wait=false
echo "all CRD checks passed"
