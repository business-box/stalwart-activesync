{{- define "stalwart-activesync.name" -}}
{{- default .Chart.Name .Values.nameOverride | trunc 63 | trimSuffix "-" }}
{{- end }}

{{- define "stalwart-activesync.fullname" -}}
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

{{- define "stalwart-activesync.labels" -}}
helm.sh/chart: {{ printf "%s-%s" .Chart.Name .Chart.Version | replace "+" "_" | trunc 63 | trimSuffix "-" }}
{{ include "stalwart-activesync.selectorLabels" . }}
app.kubernetes.io/version: {{ .Chart.AppVersion | quote }}
app.kubernetes.io/managed-by: {{ .Release.Service }}
{{- end }}

{{- define "stalwart-activesync.selectorLabels" -}}
app.kubernetes.io/name: {{ include "stalwart-activesync.name" . }}
app.kubernetes.io/instance: {{ .Release.Name }}
{{- end }}

{{- define "stalwart-activesync.serviceAccountName" -}}
{{- if .Values.serviceAccount.create }}
{{- default (include "stalwart-activesync.fullname" .) .Values.serviceAccount.name }}
{{- else }}
{{- default "default" .Values.serviceAccount.name }}
{{- end }}
{{- end }}

{{/* The Secret holding the admin token and its key. */}}
{{- define "stalwart-activesync.adminSecret" -}}
{{- default (include "stalwart-activesync.fullname" .) .Values.adminToken.existingSecret }}
{{- end }}
{{- define "stalwart-activesync.adminSecretKey" -}}
{{- if .Values.adminToken.existingSecret }}{{ .Values.adminToken.existingSecretKey }}{{ else }}admin-token{{ end }}
{{- end }}

{{/*
Ingress paths for the gateway: the ActiveSync endpoint and the three
spellings of autodiscover phones use. Takes (dict "service" "port").
*/}}
{{- define "stalwart-activesync.ingressPaths" -}}
- path: /Microsoft-Server-ActiveSync
  pathType: Prefix
  backend:
    service:
      name: {{ .service }}
      port:
        number: {{ .port }}
{{- range list "/autodiscover/autodiscover.xml" "/Autodiscover/Autodiscover.xml" "/AutoDiscover/AutoDiscover.xml" }}
- path: {{ . }}
  pathType: Exact
  backend:
    service:
      name: {{ $.service }}
      port:
        number: {{ $.port }}
{{- end }}
{{- end }}
