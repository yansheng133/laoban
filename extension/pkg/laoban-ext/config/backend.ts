// Where the M0 page finds the laoban backend and Fleet, defined in one place.
// Values follow docs/m0-design.md. The page runs inside the Rancher UI and
// every request uses the signed-in user's own Rancher session.

// Downstream cluster (rke2) that runs the laoban backend.
export const BACKEND_CLUSTER_ID = 'c-m-26kbxk5f';

export const BACKEND_NAMESPACE = 'laoban-system';

export const BACKEND_SERVICE = 'laoban';

export const BACKEND_PORT = 8080;

// Rancher service proxy to the backend Service. API paths ("/api/plan", ...)
// are appended to this.
export const BACKEND_BASE_PATH = `/k8s/clusters/${ BACKEND_CLUSTER_ID }/api/v1/namespaces/${ BACKEND_NAMESPACE }/services/http:${ BACKEND_SERVICE }:${ BACKEND_PORT }/proxy`;

// Fleet runs on the Rancher local cluster; bundles are read through Steve.
export const FLEET_BUNDLE_TYPE = 'fleet.cattle.io.bundle';

export const FLEET_WORKSPACE = 'fleet-default';

export const FLEET_BUNDLES_PATH = `/v1/${ FLEET_BUNDLE_TYPE }s/${ FLEET_WORKSPACE }`;

export const BUNDLE_POLL_INTERVAL_MS = 5 * 1000;

export const BUNDLE_POLL_TIMEOUT_MS = 10 * 60 * 1000;

// AI calls usually take 30-60 s; give up well after that.
export const AI_REQUEST_TIMEOUT_MS = 3 * 60 * 1000;

export const DEFAULT_REQUEST_TIMEOUT_MS = 30 * 1000;

// Same rule as the backend (docs/m0-design.md, POST /api/plan).
export const APP_NAME_PATTERN = /^[a-z][a-z0-9-]{1,30}$/;
