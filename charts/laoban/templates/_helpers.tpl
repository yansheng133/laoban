{{/*
Chart name, truncated to 63 characters (DNS label limit).
*/}}
{{- define "laoban.name" -}}
{{- default .Chart.Name .Values.nameOverride | trunc 63 | trimSuffix "-" }}
{{- end }}

{{/*
Full name. With the fixed release name "laoban" from the Rancher catalog this is
"laoban", which the UI extension uses in the proxy URL.
*/}}
{{- define "laoban.fullname" -}}
{{- if .Values.fullnameOverride }}
{{- .Values.fullnameOverride | trunc 63 | trimSuffix "-" }}
{{- else }}
{{- $name := default .Chart.Name .Values.nameOverride }}
{{- if contains $name .Release.Name }}
{{- .Release.Name | trunc 63 | trimSuffix "-" }}
{{- else }}
{{- printf "%s-%s" .Release.Name $name | trunc 63 | trimSuffix "-" }}
{{- end }}
{{- end }}
{{- end }}

{{- define "laoban.chart" -}}
{{- printf "%s-%s" .Chart.Name .Chart.Version | replace "+" "_" | trunc 63 | trimSuffix "-" }}
{{- end }}

{{- define "laoban.selectorLabels" -}}
app.kubernetes.io/name: {{ include "laoban.name" . }}
app.kubernetes.io/instance: {{ .Release.Name }}
{{- end }}

{{- define "laoban.labels" -}}
helm.sh/chart: {{ include "laoban.chart" . }}
{{ include "laoban.selectorLabels" . }}
app.kubernetes.io/version: {{ .Chart.AppVersion | quote }}
app.kubernetes.io/managed-by: {{ .Release.Service }}
app.kubernetes.io/part-of: laoban
{{- end }}

{{- define "laoban.serviceAccountName" -}}
{{- default (include "laoban.fullname" .) .Values.serviceAccount.name }}
{{- end }}

{{/*
Cluster-scoped object names carry the "laoban-" prefix and the release
namespace so two installs cannot collide.
*/}}
{{- define "laoban.clusterRoleName" -}}
{{- printf "laoban-impersonator-%s-%s" .Release.Namespace (include "laoban.fullname" .) | trunc 63 | trimSuffix "-" }}
{{- end }}

{{/*
Image reference, always by digest. The tag is informational and never used.
*/}}
{{- define "laoban.image" -}}
{{- $repo := required "image.repository is required" .Values.image.repository }}
{{- if contains "REPLACE_ME" $repo }}
{{- fail "image.repository still contains the placeholder REPLACE_ME: set it to the real laoban-server repository" }}
{{- end }}
{{- $digest := required "image.digest is required: laoban pulls images by digest only (e.g. sha256:<64 hex>). The tag is informational." .Values.image.digest }}
{{- if not (regexMatch "^sha256:[a-f0-9]{64}$" $digest) }}
{{- fail (printf "image.digest must look like sha256:<64 lowercase hex>, got %q" $digest) }}
{{- end }}
{{- printf "%s@%s" $repo $digest }}
{{- end }}

{{/*
CRD presence check. The CRDs ship in the separate laoban-crd chart and are
not used in M0, so the check only runs when crds.required is true.
*/}}
{{- define "laoban.crdCheck" -}}
{{- if .Values.crds.required }}
{{- $gv := "laoban.yansheng133.github.io/v1alpha1" }}
{{- if not (.Capabilities.APIVersions.Has $gv) }}
{{- fail (printf "API %s not found: install the laoban-crd chart first (or set crds.required=false / pass --api-versions %s when rendering offline)" $gv $gv) }}
{{- end }}
{{- end }}
{{- end }}
