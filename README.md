# stalwart-activesync

Exchange ActiveSync (EAS) gateway for [Stalwart](https://github.com/stalwartlabs/stalwart), written in Rust.

It lets the built-in mail, calendar and contacts apps on iOS and Android (Gmail, Samsung Email) sync with a Stalwart account without installing anything. The gateway speaks EAS 14.1 and 16.1 to phones and only JMAP to Stalwart, so Stalwart runs unmodified. Desktop clients are out of scope: use Thunderbird (IMAP, CalDAV, CardDAV) directly against Stalwart.

**Status: early development.** Mail is complete end to end against the in-process JMAP test double: provisioning, folders, sync, push, sending, replies and forwards, attachments, moves, search and out-of-office. CI runs it against a real Stalwart container (sync, send with push, contact and event writes); it has not been tried with real phones yet. Contacts, calendars and folder management work too; commands that aren't implemented answer `501 Not Implemented`. See [docs/PLAN.md](docs/PLAN.md) for scope, architecture, the work breakdown and the roadmap.

## What works today

| Area | State |
| --- | --- |
| WBXML codec (`eas-wbxml`) | Decoder and encoder with all 26 EAS code pages, hostile-input limits, XML rendering for logs |
| Request line (`eas-proto`) | Plain and base64 query forms, protocol version negotiation, command codes |
| `OPTIONS` | Advertises 14.0, 14.1, 16.0, 16.1 and the supported commands |
| Authentication | Basic credentials checked by opening the user's JMAP session on Stalwart, cached for 5 minutes; client IP forwarded |
| Autodiscover | Answers `mobilesync` requests; passes Outlook-schema requests to Stalwart |
| `Provision` | Two-step policy keys, permissive default policy, remote wipe answered and acknowledged; other commands get status 142/144/140 until provisioned |
| `FolderSync` | Mailboxes as folders with stable short IDs, roles mapped to Inbox/Drafts/Trash/Sent, adds/updates/deletes, retry replay |
| `Sync` (mail) | SyncKey state machine with retry replay, window size and `MoreAvailable`, filter types, plain/HTML/MIME bodies with truncation and preview, attachments listed; device read/flag changes, deletes (to Trash) and fetches; server-side adds, deletes and read/flag changes; empty (cached) requests; `Wait`/`HeartbeatInterval` held by polling until push lands |
| `GetItemEstimate` | Mail folders |
| State (`eas-state`) | SQLite: devices, policy keys, folder map, per-folder sync state with replay data, items |
| Calendar | Calendars as folders (the default one as the device's Calendar); events synced both ways with time zones, all-day events, reminders, busy status, privacy, categories, attendees, recurrence (daily, weekly, monthly, nth weekday, yearly) and exceptions; calendar FilterType as a date window; device edits patch only what changed; 16.x devices get server-sent invitations |
| `MeetingResponse` | Accept, tentative or decline from the calendar item or the invitation mail (found by UID); Stalwart sends the reply |
| Invitation mails | Mails with a `text/calendar` part carry meeting details (`MeetingRequest`, `GlobalObjId`, meeting message class) taken from Stalwart's copy of the event or, failing that, from the invitation itself |
| Not yet | `ResolveRecipients`, `Find`, GAL, drafts sync (16.x), responses to single occurrences, contact photos, creating contact or calendar folders from the device, tasks |

## Layout

| Crate | Role |
| --- | --- |
| `crates/eas-wbxml` | WBXML tokens, code pages, encoder and decoder |
| `crates/eas-proto` | Versions, commands, request lines, OPTIONS, autodiscover, status codes, the policy document |
| `crates/eas-jmap` | JMAP session, batched calls, method errors, blobs, per-user concurrency limit, EventSource, mail, submission and vacation helpers |
| `crates/eas-convert` | Email items, read/flag patches, outgoing MIME (envelope, Bcc, smart reply/forward), JSContact cards, JSCalendar events, Windows time zone blobs |
| `crates/eas-state` | Device and sync state in SQLite |
| `crates/eas-server` | The `stalwart-activesync` binary: HTTP, auth, policy checks, command handlers |

## Running

For a complete local or server test setup (Stalwart, this gateway, Bulwark webmail and Caddy) see [deploy/README.md](deploy/README.md): `docker compose up`, one bootstrap script for a domain and test users, and a probe that plays a phone.

`stalwart-activesync probe <https://mail.example.org> <user> <password> [--insecure] [--full]` checks any deployment end to end: OPTIONS, Provision, FolderSync, and a first Sync of the inbox, contacts and calendar. With `--full` it also holds a `Ping`, mails itself and waits for the push, and creates and deletes a contact and an event (it writes to the account, so use a test user).

Images are built by GitHub Actions and pushed to the GitHub Container Registry: `ghcr.io/business-box/stalwart-activesync`, tagged with the branch name, the short commit SHA, `latest` for the default branch and the version for `v*` tags.

```sh
docker run -d --name stalwart-activesync \
  -e STALWART_URL=http://stalwart:8080 \
  -e EAS_PUBLIC_URL=https://mail.example.org \
  -v eas-data:/data -p 8080:8080 \
  ghcr.io/business-box/stalwart-activesync:latest
```

From source:

```sh
STALWART_URL=http://stalwart:8080 \
EAS_PUBLIC_URL=https://mail.example.org \
cargo run --release -p eas-server
```

| Variable | Default | Meaning |
| --- | --- | --- |
| `STALWART_URL` | required | Stalwart's HTTP root as the gateway sees it |
| `EAS_PUBLIC_URL` | required | Public origin phones use; autodiscover points them at `<EAS_PUBLIC_URL>/Microsoft-Server-ActiveSync` |
| `EAS_LISTEN` | `0.0.0.0:8080` | Listen address |
| `EAS_TRUST_FORWARDED` | `true` | Take the client IP from `X-Forwarded-For` (only behind a reverse proxy) |
| `EAS_MAX_BODY_MB` | `50` | Largest request body |
| `EAS_AUTH_CACHE_SECS` | `300` | How long a successful login is reused |
| `EAS_DB_URL` | `sqlite://eas.db` | State database (`sqlite:///data/eas.db` in the container image) |
| `EAS_REQUIRE_PROVISIONING` | `true` | Refuse commands from devices without a policy key; remote wipe needs it |
| `EAS_ADMIN_TOKEN` | unset | Bearer token for the admin API; the API is off without it |
| `STALWART_REWRITE_URLS` | `true` | Reach the JMAP API, blobs and push at `STALWART_URL` even though Stalwart's session names its public `https://` hostname |
| `RUST_LOG` | `info` | Log filter; `debug` logs decoded request bodies (opaque data is never logged) |

### Admin API and metrics

With `EAS_ADMIN_TOKEN` set, the gateway serves a small admin API on its own port. Don't route `/admin` or `/metrics` through the public proxy.

```sh
T="Authorization: Bearer $EAS_ADMIN_TOKEN"
curl -H "$T" http://gateway:8080/admin/devices                      # all devices
curl -H "$T" 'http://gateway:8080/admin/devices?user=alice@example.org'
curl -H "$T" -X POST http://gateway:8080/admin/devices/7/reset     # re-provision and resync from scratch
curl -H "$T" -X POST http://gateway:8080/admin/devices/7/block     # or /unblock
curl -H "$T" -X POST http://gateway:8080/admin/devices/7/wipe      # remote wipe on next contact
curl -H "$T" -X DELETE http://gateway:8080/admin/devices/7         # forget the device (cancels a wipe)
```

`GET /metrics` serves Prometheus counters for requests by command and status, time spent per command, waiting `Ping`/`Sync` requests and the number of known devices.

### Caddy

Route the EAS endpoint and autodiscover to the gateway, everything else to Stalwart, and allow long-polling `Ping` requests:

```caddyfile
mail.example.org {
	@eas path /Microsoft-Server-ActiveSync* /autodiscover/autodiscover.xml /Autodiscover/Autodiscover.xml
	handle @eas {
		reverse_proxy stalwart-activesync:8080 {
			transport http {
				read_timeout 65m
				write_timeout 65m
			}
		}
	}
	handle {
		reverse_proxy stalwart:8080
	}
}
```

Enable Stalwart's forwarded-header handling for HTTP so failed logins are counted against the phone's IP rather than the gateway's.

## Development

```sh
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
cargo fmt --all
```

`crates/eas-server/src/e2e_tests.rs` drives real WBXML requests through the router against a small fake of Stalwart's JMAP API (session, mailboxes, email query/get/changes/set/import, submission, identities, vacation response, blob upload and download, EventSource), so the sync logic is tested without a running Stalwart.

### Against a real Stalwart

`tests/integration/run.sh [binary]` starts a throwaway Stalwart container, creates a domain and a user with `deploy/bootstrap.py`, runs the gateway binary against it and plays a phone with `probe --full`. CI runs it on every push. Needs Docker, Python 3 and curl.

### Fuzzing

`fuzz/` holds [cargo-fuzz](https://github.com/rust-fuzz/cargo-fuzz) targets for everything that parses phone input: the WBXML decoder (with an encode round trip), the request line (plain and base64), device-sent contacts, events and mail changes, iCalendar invitations, time zone blobs and outgoing MIME. A nightly workflow runs each for five minutes.

```sh
cd fuzz && cargo +nightly fuzz run wbxml_decode -- -max_total_time=60
```

### Load

`stalwart-activesync load <url> <password> --accounts N --devices M` sets up N×M phones (users `load<n>@example.test`, all with one password), holds a `Ping` on every one, mails each account once and reports how quickly every `Ping` woke. Against a local Stalwart 0.16 and a release build, 100 accounts × 5 devices: setup of all 500 phones in 7 s, all 500 `Ping`s woken, latency after send p50 1.7 s, p99 1.8 s, gateway at 49 MB RSS with 13 threads.

```sh
python3 deploy/bootstrap.py --users "$(seq -s, -f 'load%g:a-long-shared-pass' 1 100)" ...
stalwart-activesync load http://127.0.0.1:8081 a-long-shared-pass --accounts 100 --devices 5
```

### Code pages

The code page tables in `crates/eas-wbxml/src/codepages.rs` are generated by `tools/gen_codepages.py` from [go-activesync](https://github.com/hstern/go-activesync) (MIT, official MS-ASWBXML names) and [Z-Push](https://github.com/Z-Hub/Z-Push) (AGPL-3.0, for tokens the former lacks).

## Licence

AGPL-3.0-only. See [LICENSE](LICENSE).
