# Helm charts

| Chart | What it installs |
| --- | --- |
| [`stalwart-activesync`](stalwart-activesync) | The gateway alone, for a cluster that already runs Stalwart |
| [`eas-stack`](eas-stack) | Stalwart, the gateway, Bulwark webmail, one Ingress for all three names and a bootstrap Job (domain, users, forwarded headers, CORS) |

Both need Helm 3.8 or newer. CI lints them, validates the output with kubeconform and installs `eas-stack` into a kind cluster on every change (`.github/workflows/helm.yml`).

## The whole stack

Needs an ingress controller (the defaults are for [ingress-nginx](https://kubernetes.github.io/ingress-nginx/)), a default StorageClass and, for certificates, [cert-manager](https://cert-manager.io) with an issuer.

```sh
cd deploy/helm/eas-stack
helm dependency build .
helm install mail . -n mail --create-namespace \
  --set global.mailHost=mail.example.org \
  --set global.webmailHost=webmail.example.org \
  --set global.autodiscoverHost=autodiscover.example.org \
  --set ingress.tls.certManagerIssuer=letsencrypt-prod \
  --set 'bootstrap.users[0]=alice:a-long-password-for-alice'
```

Then point DNS for the three names at the ingress controller, and the domain's `MX` at the `mail-stalwart-mail` LoadBalancer (SMTP 25, submissions 465, IMAPS 993). `helm status mail -n mail` repeats the next steps.

What it does:

- **Stalwart** runs as a single-replica StatefulSet on one volume (config in `etc/`, RocksDB in `data/`). An init container writes a minimal `config.json` on first start so Stalwart skips its setup wizard (`stalwart.seedConfig=false` to run the wizard instead). The recovery admin's password is generated and kept in the Secret `<release>-stalwart`.
- **The gateway** is the `stalwart-activesync` chart as a subchart. It reaches Stalwart at `http://<release>-stalwart:8080` and keeps its SQLite state on its own volume.
- **Bulwark** runs as a Deployment with its settings on a volume (`bulwark.enabled=false` to leave it out).
- **The Ingress** routes `/Microsoft-Server-ActiveSync` and autodiscover on the mail and autodiscover names to the gateway, everything else on the mail name to Stalwart, and the webmail name to Bulwark. Read and send timeouts are 65 minutes for long-held `Ping`s. The gateway's `/admin` and `/metrics` are never routed.
- **The bootstrap Job** (a post-install and post-upgrade hook) runs [`deploy/bootstrap.py`](../bootstrap.py): creates the domain and the users in `bootstrap.users`, sets the default hostname, turns on `X-Forwarded-For` and CORS for the webmail, and deletes Stalwart's pod once when those HTTP settings changed, since Stalwart only applies them on restart. Its Role may only get and delete that one pod. Existing users are left alone, so it is safe on every upgrade; keep passwords out of values with `bootstrap.existingSecret` (key `users`, `name:password` pairs separated by commas).

Ingress controllers other than ingress-nginx: replace `ingress.annotations` with the controller's equivalents for long read/send timeouts, no response buffering and a 50 MB body limit.

## Only the gateway

```sh
helm install eas deploy/helm/stalwart-activesync -n mail \
  --set stalwartUrl=http://stalwart.mail.svc:8080 \
  --set publicUrl=https://mail.example.org \
  --set ingress.enabled=true --set ingress.className=nginx \
  --set 'ingress.hosts[0]=mail.example.org' \
  --set 'ingress.tls[0].secretName=mail-tls' --set 'ingress.tls[0].hosts[0]=mail.example.org'
```

The chart's Ingress carries only the ActiveSync and autodiscover paths; ingress-nginx merges it with the Ingress that already serves Stalwart on the same host. Turn on Stalwart's forwarded-header handling (Settings → HTTP) so failed logins count against the phone's address.

The gateway's admin token is generated and kept in the Secret `<release>-stalwart-activesync` (or bring your own with `adminToken.existingSecret`). The admin API is not exposed; use a port-forward:

```sh
kubectl -n mail port-forward svc/eas-stalwart-activesync 8081:8080
curl -H "Authorization: Bearer $TOKEN" http://127.0.0.1:8081/admin/devices
```

`metrics.serviceMonitor.enabled=true` adds a ServiceMonitor for the Prometheus Operator.

## Why single replicas

The gateway keeps device and sync state in SQLite, and holds every phone's `Ping` in memory together with the push connection to Stalwart, so it runs as exactly one pod; 500 waiting phones take about 50 MB. Stalwart in this chart uses RocksDB on a local volume, which is single-node too; for a Stalwart cluster use its own deployment guide and the gateway chart alone.

## Checking a deployment

```sh
kubectl -n mail exec -it mail-stalwart-activesync-0 -- \
  stalwart-activesync probe http://127.0.0.1:8080 alice@example.org 'password' --full
```

plays a phone against the gateway: provisioning, folders, a first sync, a push after a mail to itself, and creating and deleting a contact and an event. Against `https://mail.example.org` instead of `127.0.0.1` it goes through the Ingress and TLS as a real phone would.
