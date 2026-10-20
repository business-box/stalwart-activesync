#!/usr/bin/env python3
"""Prepare the test stack's Stalwart for logins, without the web UI.

Talks to Stalwart's JMAP management API (`x:*` methods) as the recovery
admin on the plain HTTP port published to 127.0.0.1, and:

- sets the default hostname and domain,
- trusts X-Forwarded-For (so failed logins count against the phone's
  address, not Caddy's or the gateway's; Stalwart's HTTP port is only
  reachable inside the stack),
- allows cross-origin JMAP for the webmail,
- creates the test domain and users.

Safe to run again: existing domains and users are left as they are, and
Stalwart only needs a restart when the HTTP settings actually changed
(--restart-flag FILE writes FILE in that case).

    python3 bootstrap.py                      # reads .env next to this file
    python3 bootstrap.py --users carol:secret --domain example.org

Standard library only.
"""

import argparse
import base64
import json
import os
import sys
import urllib.error
import urllib.request

HERE = os.path.dirname(os.path.abspath(__file__))
USING = ["urn:ietf:params:jmap:core", "urn:stalwart:jmap"]


def load_env(path):
    env = {}
    if os.path.exists(path):
        with open(path) as f:
            for line in f:
                line = line.strip()
                if line and not line.startswith("#") and "=" in line:
                    k, v = line.split("=", 1)
                    env[k.strip()] = v.strip().strip('"').strip("'")
    return env


class Stalwart:
    def __init__(self, url, user, password):
        self.url = url.rstrip("/")
        token = base64.b64encode(f"{user}:{password}".encode()).decode()
        self.auth = {"Authorization": f"Basic {token}"}
        session = self.get("/jmap/session")
        self.account = session["primaryAccounts"].get("urn:stalwart:jmap")
        if not self.account:
            sys.exit("this login has no management rights (urn:stalwart:jmap)")

    def get(self, path):
        req = urllib.request.Request(self.url + path, headers=self.auth)
        with urllib.request.urlopen(req, timeout=30) as r:
            return json.load(r)

    def call(self, *calls):
        body = json.dumps(
            {
                "using": USING,
                "methodCalls": [[m, {"accountId": self.account, **a}, str(i)] for i, (m, a) in enumerate(calls)],
            }
        ).encode()
        req = urllib.request.Request(
            self.url + "/jmap/",
            data=body,
            headers={**self.auth, "Content-Type": "application/json"},
        )
        with urllib.request.urlopen(req, timeout=30) as r:
            out = json.load(r)
        results = []
        for name, args, _ in out["methodResponses"]:
            if name == "error":
                sys.exit(f"JMAP error: {args}")
            results.append(args)
        return results


def main():
    env = {**load_env(os.path.join(HERE, ".env")), **os.environ}
    mail_host = env.get("MAIL_HOST", "mail.localhost")
    default_domain = mail_host.split(".", 1)[1] if mail_host.count(".") >= 1 else mail_host
    if default_domain == "localhost":
        default_domain = "example.test"

    p = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    p.add_argument("--url", default=f"http://127.0.0.1:{env.get('STALWART_HTTP_PORT', '8082')}")
    p.add_argument("--admin-password", default=env.get("STALWART_ADMIN_PASSWORD"))
    p.add_argument("--domain", default=env.get("TEST_DOMAIN", default_domain))
    p.add_argument(
        "--users",
        default=env.get("TEST_USERS", "alice:alice-test-pass,bob:bob-test-pass"),
        help="comma-separated name:password pairs (local part only)",
    )
    p.add_argument("--restart-flag", help="write this file when Stalwart needs a restart")
    args = p.parse_args()
    if not args.admin_password:
        sys.exit("set STALWART_ADMIN_PASSWORD in .env or pass --admin-password")

    try:
        s = Stalwart(args.url, "admin", args.admin_password)
    except urllib.error.URLError as e:
        sys.exit(f"cannot reach Stalwart at {args.url}: {e}")

    # Domain: reuse if it exists.
    (domains,) = s.call(("x:Domain/get", {"ids": None, "properties": ["name"]}))
    domain_id = next((d["id"] for d in domains["list"] if d.get("name") == args.domain), None)
    if domain_id is None:
        (res,) = s.call(("x:Domain/set", {"create": {"d": {"name": args.domain}}}))
        if "d" not in res.get("created", {}):
            sys.exit(f"could not create domain {args.domain}: {res.get('notCreated')}")
        domain_id = res["created"]["d"]["id"]
        print(f"domain   {args.domain} created")
    else:
        print(f"domain   {args.domain} exists")

    wanted_http = {"useXForwarded": True, "usePermissiveCors": True}
    (current,) = s.call(("x:Http/get", {"ids": ["singleton"], "properties": list(wanted_http)}))
    current = (current.get("list") or [{}])[0]
    http_changed = any(current.get(k) != v for k, v in wanted_http.items())
    calls = [("x:SystemSettings/set", {"update": {"singleton": {"defaultHostname": mail_host, "defaultDomainId": domain_id}}})]
    if http_changed:
        calls.append(("x:Http/set", {"update": {"singleton": wanted_http}}))
    results = s.call(*calls)
    for name, res in zip(("system settings", "http settings"), results):
        if "singleton" not in res.get("updated", {}):
            sys.exit(f"could not update {name}: {res.get('notUpdated')}")
    print(f"settings hostname {mail_host}, X-Forwarded-For on, CORS open" + ("" if http_changed else " (unchanged)"))

    (accounts,) = s.call(("x:Account/get", {"ids": None, "properties": ["name", "domainId"]}))
    existing = {(a.get("name"), a.get("domainId")) for a in accounts["list"]}
    for pair in filter(None, args.users.split(",")):
        name, _, password = pair.partition(":")
        name = name.strip()
        if (name, domain_id) in existing:
            print(f"user     {name}@{args.domain} exists")
            continue
        (res,) = s.call(
            (
                "x:Account/set",
                {
                    "create": {
                        "u": {
                            "@type": "User",
                            "name": name,
                            "domainId": domain_id,
                            "credentials": {"0": {"@type": "Password", "secret": password}},
                        }
                    }
                },
            )
        )
        if "u" not in res.get("created", {}):
            sys.exit(f"could not create {name}: {res.get('notCreated')}")
        print(f"user     {name}@{args.domain} created (password {password})")
    if http_changed:
        if args.restart_flag:
            with open(args.restart_flag, "w") as f:
                f.write("http settings changed\n")
        print("done; restart Stalwart so the HTTP settings apply (docker compose restart stalwart)")
    else:
        print("done")


if __name__ == "__main__":
    main()
