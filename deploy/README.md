# Test stack

Stalwart, the ActiveSync gateway and Bulwark webmail behind Caddy, for trying
the whole thing on a laptop or a server.

| Service | What it does | Reached at |
| --- | --- | --- |
| `caddy` | TLS and routing: ActiveSync and autodiscover to the gateway, the rest of the mail host to Stalwart, the webmail host to Bulwark | `https://MAIL_HOST`, `https://WEBMAIL_HOST`, `https://AUTODISCOVER_HOST` |
| `stalwart` | Mail, calendars and contacts (JMAP, IMAP, SMTP), web UI at `/admin` | via Caddy; SMTP 25/465 and IMAPS 993 directly; plain HTTP on `127.0.0.1:8082` for setup |
| `activesync` | This gateway | via Caddy; admin API and `/metrics` on `127.0.0.1:8081` |
| `bulwark` | [Bulwark](https://github.com/bulwarkmail/webmail) webmail over JMAP | `https://WEBMAIL_HOST` |
| `stalwart-init` | Writes a minimal `config.json` on first start so Stalwart skips its setup wizard | runs once |

For Kubernetes, see [helm/README.md](helm/README.md).

All services share one Docker network; only Caddy, Stalwart's mail ports and the two loopback ports are published.

## Quick start on a laptop

```sh
cd deploy
cp .env.example .env            # set the three secrets at the top
docker login ghcr.io            # the gateway image is in the repo's package registry
docker compose up -d
python3 bootstrap.py            # domain, users, forwarded-header and CORS settings
docker compose restart stalwart # HTTP settings apply on restart
docker compose run --rm activesync probe https://mail.localhost alice@example.test alice-test-pass --insecure
```

The probe plays a phone: OPTIONS, the two Provision round trips, FolderSync,
and a first Sync of the inbox, contacts and calendar. It prints each step:

```
OPTIONS        HTTP 200 OK versions 14.0,14.1,16.0,16.1
Provision      ok, policy key 3272105934
FolderSync     7 folders
     1  type  9  Stalwart Address Book (alice@example.test)
     2  type  8  Stalwart Calendar (alice@example.test)
     5  type  2  Inbox
     ...
Sync Inbox     1 item(s)
  - Hello from the test stack
probe finished
```

Then:

- Webmail: <https://webmail.localhost>, log in as `alice@example.test`. Mail
  sent from here shows up on the phone and the other way round.
- Stalwart web UI: <https://mail.localhost/admin>, user `admin`, the
  `STALWART_ADMIN_PASSWORD` from `.env`.
- Gateway devices: `curl -H "Authorization: Bearer $EAS_ADMIN_TOKEN" http://127.0.0.1:8081/admin/devices`

`*.localhost` names resolve to 127.0.0.1 in browsers and curl, and Caddy
signs them with its own CA. Accept the browser warning, or trust the root:

```sh
docker compose cp caddy:/data/caddy/pki/authorities/local/root.crt caddy-root.crt
# macOS: sudo security add-trusted-cert -d -r trustRoot -k /Library/Keychains/System.keychain caddy-root.crt
# Debian/Ubuntu: sudo cp caddy-root.crt /usr/local/share/ca-certificates/ && sudo update-ca-certificates
```

To build the gateway from this checkout instead of pulling it:

```sh
docker compose -f docker-compose.yml -f docker-compose.build.yml up -d --build
```

## Phones against a laptop

Phones need a name that resolves to the laptop and a certificate they trust.

1. Pick a name on your LAN address, e.g. with nip.io: in `.env` set
   `MAIL_HOST=mail.192-168-1-20.nip.io` (and `WEBMAIL_HOST`,
   `AUTODISCOVER_HOST` the same way), keep `TLS_MODE=internal`.
2. `docker compose up -d && python3 bootstrap.py --domain example.test && docker compose restart stalwart`
3. Install `caddy-root.crt` (see above) on the phone and trust it
   (iOS: Settings → General → About → Certificate Trust Settings).
4. Add an Exchange account: address `alice@example.test`, server
   `mail.192-168-1-20.nip.io`, the password from `bootstrap.py`.

## On a server

1. Point DNS for the three names at the server (`A`/`AAAA`), and an `MX` for
   the mail domain if it should receive mail from outside.
2. In `.env`: real names, `TLS_MODE=acme`, `ACME_EMAIL`, strong secrets.
3. Open 80 and 443 (and 25, 465, 993 for mail clients and other servers).
4. `docker compose up -d`, then `python3 bootstrap.py --domain example.org --users you:password`
   and `docker compose restart stalwart`.
5. Phones: add an Exchange account with the address and password; with
   `AUTODISCOVER_HOST=autodiscover.example.org` they find the server alone.

Stalwart's SMTP and IMAP ports use its own certificate (self-signed until you
set up ACME in Stalwart's web UI); Caddy only handles HTTPS.

## Debugging

- `docker compose logs -f activesync` with `EAS_LOG=info,stalwart_activesync=debug`
  (the default in `.env.example`) shows every decoded request and response;
  binary data is logged as a size only.
- `curl -H "Authorization: Bearer $EAS_ADMIN_TOKEN" -X POST http://127.0.0.1:8081/admin/devices/<id>/reset`
  makes a phone provision and resync from scratch.
- `curl http://127.0.0.1:8081/metrics` counts requests per command and status.
- Stalwart's own logs: `docker compose logs -f stalwart`.

## How the pieces talk

- The gateway reaches Stalwart at `http://stalwart:8080` inside the network.
  Stalwart's JMAP session names its public `https://MAIL_HOST` URLs; the
  gateway rewrites them onto `STALWART_URL` (`STALWART_REWRITE_URLS`, on by
  default), so it never needs Caddy's certificate.
- Bulwark talks to `https://MAIL_HOST` both from the browser and from its
  server. Inside the network that name points at Caddy (a network alias), and
  `NODE_EXTRA_CA_CERTS` makes Node trust Caddy's local CA.
- Stalwart trusts `X-Forwarded-For` once `bootstrap.py` has run, so its
  brute-force protection sees the phone's address. That is safe here because
  Stalwart's HTTP ports are only reachable inside the stack.
- Without `bootstrap.py`, do the same in Stalwart's web UI: Settings → System
  (default hostname) and Settings → HTTP (use X-Forwarded-For, permissive
  CORS).
- Don't add the stack's network to Stalwart's "proxy trusted networks": that
  setting is for the HAProxy PROXY protocol, and Stalwart then refuses plain
  HTTP from those addresses, including the gateway and Caddy.

## Resetting

```sh
docker compose down -v          # removes all volumes: mail, settings, devices
```
