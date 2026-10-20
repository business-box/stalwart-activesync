{{- define "eas-stack.commonLabels" -}}
helm.sh/chart: {{ printf "%s-%s" .Chart.Name .Chart.Version | replace "+" "_" | trunc 63 | trimSuffix "-" }}
app.kubernetes.io/part-of: eas-stack
app.kubernetes.io/managed-by: {{ .Release.Service }}
{{- end }}

{{- define "eas-stack.labels" -}}
{{ include "eas-stack.commonLabels" . }}
app.kubernetes.io/instance: {{ .Release.Name }}
{{- end }}

{{- define "eas-stack.stalwart" -}}
{{- printf "%s-stalwart" .Release.Name | trunc 63 | trimSuffix "-" }}
{{- end }}
{{- define "eas-stack.stalwartSelector" -}}
app.kubernetes.io/name: stalwart
app.kubernetes.io/instance: {{ .Release.Name }}
{{- end }}

{{- define "eas-stack.bulwark" -}}
{{- printf "%s-bulwark" .Release.Name | trunc 63 | trimSuffix "-" }}
{{- end }}
{{- define "eas-stack.bulwarkSelector" -}}
app.kubernetes.io/name: bulwark
app.kubernetes.io/instance: {{ .Release.Name }}
{{- end }}

{{/* The gateway's Service name, as the subchart names it. */}}
{{- define "eas-stack.gateway" -}}
{{- $sub := index .Values "stalwart-activesync" }}
{{- if $sub.fullnameOverride }}
{{- $sub.fullnameOverride | trunc 63 | trimSuffix "-" }}
{{- else }}
{{- $name := default "stalwart-activesync" $sub.nameOverride }}
{{- if contains $name .Release.Name }}
{{- .Release.Name | trunc 63 | trimSuffix "-" }}
{{- else }}
{{- printf "%s-%s" .Release.Name $name | trunc 63 | trimSuffix "-" }}
{{- end }}
{{- end }}
{{- end }}

{{- define "eas-stack.stalwartSecret" -}}
{{- default (include "eas-stack.stalwart" .) .Values.stalwart.existingSecret }}
{{- end }}

{{/* Mail domain for the bootstrap job. */}}
{{- define "eas-stack.domain" -}}
{{- if .Values.bootstrap.domain }}{{ .Values.bootstrap.domain }}
{{- else }}{{ regexReplaceAll "^[^.]+\\." .Values.global.mailHost "" }}
{{- end }}
{{- end }}

{{/* A value from an existing Secret if present, else the given default. */}}
{{- define "eas-stack.keep" -}}
{{- $old := lookup "v1" "Secret" .ns .name }}
{{- if and $old $old.data (index $old.data .key) }}
{{- index $old.data .key | b64dec }}
{{- else }}
{{- .default }}
{{- end }}
{{- end }}
