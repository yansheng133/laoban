{{- define "laoban-crd.metadata" -}}
labels:
  app.kubernetes.io/name: laoban-crd
  app.kubernetes.io/part-of: laoban
  app.kubernetes.io/managed-by: {{ .Release.Service }}
  helm.sh/chart: {{ printf "%s-%s" .Chart.Name .Chart.Version | replace "+" "_" | trunc 63 | trimSuffix "-" }}
{{- if .Values.keepOnUninstall }}
annotations:
  helm.sh/resource-policy: keep
{{- end }}
{{- end }}
