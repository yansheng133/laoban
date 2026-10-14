// Where the M0 page finds the laoban backend and Fleet, defined in one place.
// Values follow docs/m0-design.md. The page runs inside the Rancher UI and
// every request uses the signed-in user's own Rancher session.

// Downstream cluster (rke2) that runs the laoban backend.
export const BACKEND_CLUSTER_ID = 'c-m-26kbxk5f';

// The backend is a Kubernetes aggregated API on that cluster: the API server
// verifies the signed-in user and checks RBAC before a request reaches it.
export const API_GROUP = 'api.laoban.yansheng133.github.io';

export const API_VERSION = 'v1alpha1';

// Namespace the apps are deployed to; the API is served under it, so access
// follows that namespace's (Rancher Project's) permissions.
export const TARGET_NAMESPACE = 'laoban-dev';

// Paths ("/config", "/apps", "/plans", ...) are appended to this.
export const BACKEND_BASE_PATH = `/k8s/clusters/${ BACKEND_CLUSTER_ID }/apis/${ API_GROUP }/${ API_VERSION }/namespaces/${ TARGET_NAMESPACE }`;

// Fleet runs on the Rancher local cluster; bundles are read through Steve.
export const FLEET_BUNDLE_TYPE = 'fleet.cattle.io.bundle';

export const FLEET_WORKSPACE = 'fleet-default';

export const FLEET_BUNDLES_PATH = `/v1/${ FLEET_BUNDLE_TYPE }s/${ FLEET_WORKSPACE }`;

export const BUNDLE_POLL_INTERVAL_MS = 5 * 1000;

export const BUNDLE_POLL_TIMEOUT_MS = 10 * 60 * 1000;

// Used until GET /api/config has answered, or when it has no
// aiTimeoutSeconds (older backend). The backend's own default is 300 s.
export const AI_REQUEST_TIMEOUT_MS = 5 * 60 * 1000 + 60 * 1000;

// Added to the backend's AI timeout: the apply call also commits to GitHub
// after the AI answers, and the request goes through the Rancher proxy.
export const AI_REQUEST_MARGIN_MS = 60 * 1000;

export const DEFAULT_REQUEST_TIMEOUT_MS = 30 * 1000;

// Same rule as the backend (docs/m0-design.md, POST /api/plan).
export const APP_NAME_PATTERN = /^[a-z][a-z0-9-]{1,30}$/;
