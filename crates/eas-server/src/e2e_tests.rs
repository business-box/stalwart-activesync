//! End-to-end tests: real WBXML requests through the router against an
//! in-process fake of the parts of Stalwart's JMAP API the gateway uses.

use crate::{app, auth, config, handlers, push};
use axum::body::Body;
use axum::extract::State;
use axum::http::{Request, StatusCode};
use axum::routing::{get, post};
use axum::{Json, Router};
use eas_wbxml::{Element, Page};
use http_body_util::BodyExt;
use serde_json::{json, Value};
use std::net::SocketAddr;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tower::ServiceExt;

#[derive(Default)]
struct Fake {
    emails: Vec<Value>,
    state: u64,
    /// (state at which the change happened, email ID)
    log: Vec<(u64, String)>,
}

type Shared = Arc<Mutex<Fake>>;

impl Fake {
    fn bump(&mut self, id: &str) {
        self.state += 1;
        self.log.push((self.state, id.to_string()));
    }

    fn add(&mut self, id: &str, mailbox: &str, received: &str) {
        self.emails.push(json!({
            "id": id, "blobId": format!("blob-{id}"), "threadId": format!("t-{id}"),
            "mailboxIds": {mailbox: true}, "keywords": {},
            "receivedAt": received, "subject": format!("Subject {id}"),
            "from": [{"name": "Alice", "email": "alice@example.org"}],
            "to": [{"name": "Bob", "email": "bob@example.org"}],
            "preview": "hello",
            "textBody": [{"partId": "1", "type": "text/plain", "size": 5}],
            "htmlBody": [{"partId": "1", "type": "text/plain", "size": 5}],
            "attachments": [],
            "bodyValues": {"1": {"value": "hello"}},
        }));
        self.bump(id);
    }

    fn email(&mut self, id: &str) -> &mut Value {
        self.emails.iter_mut().find(|e| e["id"] == id).unwrap()
    }
}

async fn session(State(addr): State<(Shared, SocketAddr)>) -> Json<Value> {
    let base = format!("http://{}", addr.1);
    Json(json!({
        "capabilities": {"urn:ietf:params:jmap:core": {"maxConcurrentRequests": 4, "maxCallsInRequest": 16}},
        "primaryAccounts": {"urn:ietf:params:jmap:mail": "acc"},
        "username": "alice@example.org",
        "apiUrl": format!("{base}/jmap"),
        "downloadUrl": format!("{base}/download/{{accountId}}/{{blobId}}/{{name}}?accept={{type}}"),
        "uploadUrl": format!("{base}/upload/{{accountId}}/"),
        "eventSourceUrl": format!("{base}/es"),
        "state": "s"
    }))
}

fn method(fake: &mut Fake, name: &str, args: &Value) -> (String, Value) {
    let ok = |v: Value| (name.to_string(), v);
    match name {
        "Mailbox/get" => ok(json!({"state": "m1", "list": [
            {"id": "inbox", "name": "Inbox", "role": "inbox", "parentId": null},
            {"id": "trash", "name": "Trash", "role": "trash", "parentId": null},
            {"id": "sub", "name": "Projects", "role": null, "parentId": "inbox"}
        ]})),
        "Email/get" => {
            let ids: Vec<&str> = args["ids"]
                .as_array()
                .unwrap()
                .iter()
                .map(|v| v.as_str().unwrap())
                .collect();
            let list: Vec<Value> = fake
                .emails
                .iter()
                .filter(|e| ids.contains(&e["id"].as_str().unwrap()))
                .cloned()
                .collect();
            ok(json!({"state": fake.state.to_string(), "list": list}))
        }
        "Email/query" => {
            let mb = args["filter"]["inMailbox"].as_str().unwrap();
            let after = args["filter"]["after"].as_str();
            let mut list: Vec<&Value> = fake
                .emails
                .iter()
                .filter(|e| e["mailboxIds"].get(mb).is_some())
                .filter(|e| after.map_or(true, |a| e["receivedAt"].as_str().unwrap() > a))
                .collect();
            list.sort_by(|a, b| b["receivedAt"].as_str().cmp(&a["receivedAt"].as_str()));
            let pos = args["position"].as_u64().unwrap_or(0) as usize;
            let ids: Vec<&Value> = list.iter().skip(pos).map(|e| &e["id"]).collect();
            ok(json!({"ids": ids}))
        }
        "Email/changes" => {
            let Ok(since) = args["sinceState"].as_str().unwrap().parse::<u64>() else {
                return ("error".into(), json!({"type": "cannotCalculateChanges"}));
            };
            let updated: Vec<&String> = fake
                .log
                .iter()
                .filter(|(s, _)| *s > since)
                .map(|(_, id)| id)
                .collect();
            ok(
                json!({"newState": fake.state.to_string(), "created": [], "updated": updated, "destroyed": [], "hasMoreChanges": false}),
            )
        }
        "Email/set" => {
            let mut not_updated = serde_json::Map::new();
            for (id, patch) in args["update"].as_object().cloned().unwrap_or_default() {
                if !fake.emails.iter().any(|e| e["id"] == id.as_str()) {
                    not_updated.insert(id, json!({"type": "notFound"}));
                    continue;
                }
                let e = fake.email(&id);
                for (path, v) in patch.as_object().unwrap() {
                    let (prop, key) = path.split_once('/').unwrap();
                    let map = e[prop].as_object_mut().unwrap();
                    if v.is_null() {
                        map.remove(key);
                    } else {
                        map.insert(key.to_string(), v.clone());
                    }
                }
                fake.bump(&id);
            }
            for id in args["destroy"].as_array().cloned().unwrap_or_default() {
                fake.emails.retain(|e| e["id"] != id);
                fake.bump(id.as_str().unwrap());
            }
            ok(json!({"notUpdated": not_updated}))
        }
        other => panic!("fake JMAP: unexpected {other}"),
    }
}

async fn api(State(st): State<(Shared, SocketAddr)>, Json(req): Json<Value>) -> Json<Value> {
    let mut fake = st.0.lock().unwrap();
    let responses: Vec<Value> = req["methodCalls"]
        .as_array()
        .unwrap()
        .iter()
        .map(|c| {
            let (name, args) = method(&mut fake, c[0].as_str().unwrap(), &c[1]);
            json!([name, args, c[2]])
        })
        .collect();
    Json(json!({"methodResponses": responses, "sessionState": "s"}))
}

/// EventSource: a `StateChange` whenever the fake's state moves.
async fn event_source(State(st): State<(Shared, SocketAddr)>) -> axum::response::Response {
    let shared = st.0;
    let start = shared.lock().unwrap().state;
    let stream = futures_util::stream::unfold(start, move |last| {
        let shared = shared.clone();
        async move {
            loop {
                let now = shared.lock().unwrap().state;
                if now != last {
                    let ev = format!(
                        "event: state\ndata: {{\"@type\":\"StateChange\",\"changed\":{{\"acc\":{{\"Email\":\"{now}\"}}}}}}\n\n"
                    );
                    return Some((Ok::<_, std::convert::Infallible>(ev), now));
                }
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
        }
    });
    axum::response::Response::builder()
        .header("Content-Type", "text/event-stream")
        .body(Body::from_stream(stream))
        .unwrap()
}

async fn start_fake() -> (Shared, String) {
    let shared: Shared = Arc::default();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let router = Router::new()
        .route("/.well-known/jmap", get(session))
        .route("/jmap", post(api))
        .route(
            "/download/{acc}/{blob}/{name}",
            get(|| async { "Subject: raw\r\n\r\nraw body" }),
        )
        .route("/es", get(event_source))
        .with_state((shared.clone(), addr));
    tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
    (shared, format!("http://{addr}/"))
}

struct Gateway {
    app: Router,
    policy_key: Option<String>,
}

impl Gateway {
    async fn new(stalwart_url: &str) -> Gateway {
        let config = config::Config {
            listen: "127.0.0.1:0".parse().unwrap(),
            stalwart_url: stalwart_url.into(),
            public_url: "https://mail.example.org".into(),
            trust_forwarded: true,
            max_body_bytes: 1024 * 1024,
            auth_cache_secs: 300,
            db_url: "sqlite::memory:".into(),
            require_provisioning: true,
        };
        let endpoint = eas_jmap::StalwartEndpoint::new(&config.stalwart_url).unwrap();
        let auth = auth::Authenticator::new(endpoint, Duration::from_secs(300));
        let app = app::router(Arc::new(app::AppState {
            config,
            auth,
            store: eas_state::Store::memory().await.unwrap(),
            limits: eas_jmap::UserLimits::new(),
            locks: handlers::DeviceLocks::default(),
            push: push::PushHub::default(),
            pings: push::PingSlots::default(),
            policy: eas_proto::provision::Policy::default(),
        }))
        .layer(axum::extract::connect_info::MockConnectInfo(
            SocketAddr::from(([127, 0, 0, 1], 40000)),
        ));
        Gateway {
            app,
            policy_key: None,
        }
    }

    /// Sends a command; `None` for an empty response body.
    async fn send(&self, cmd: &str, body: Option<&Element>) -> (StatusCode, Option<Element>) {
        let mut req = Request::post(format!(
            "/Microsoft-Server-ActiveSync?Cmd={cmd}&User=alice&DeviceId=DEV1&DeviceType=iPhone"
        ))
        .header("MS-ASProtocolVersion", "16.1")
        // alice:secret
        .header("Authorization", "Basic YWxpY2U6c2VjcmV0")
        .header("Content-Type", eas_wbxml::CONTENT_TYPE);
        if let Some(k) = &self.policy_key {
            req = req.header("X-MS-PolicyKey", k);
        }
        let body = body.map(eas_wbxml::encode).unwrap_or_default();
        let resp = self
            .app
            .clone()
            .oneshot(req.body(Body::from(body)).unwrap())
            .await
            .unwrap();
        let status = resp.status();
        let bytes = resp.into_body().collect().await.unwrap().to_bytes();
        let el = (!bytes.is_empty()).then(|| eas_wbxml::decode(&bytes).unwrap());
        (status, el)
    }

    async fn ok(&self, cmd: &str, body: &Element) -> Element {
        let (status, el) = self.send(cmd, Some(body)).await;
        assert_eq!(status, StatusCode::OK);
        el.expect("response body")
    }
}

fn t(page: Page, name: &str, text: &str) -> Element {
    Element::new(page, name).with_text(text)
}

fn provision_request(key: Option<&str>) -> Element {
    let mut policy = Element::new(Page::Provision, "Policy").with_child(t(
        Page::Provision,
        "PolicyType",
        "MS-EAS-Provisioning-WBXML",
    ));
    if let Some(k) = key {
        policy = policy
            .with_child(t(Page::Provision, "PolicyKey", k))
            .with_child(t(Page::Provision, "Status", "1"));
    }
    Element::new(Page::Provision, "Provision")
        .with_child(Element::new(Page::Provision, "Policies").with_child(policy))
}

fn policy_key(resp: &Element) -> String {
    resp.child(Page::Provision, "Policies")
        .and_then(|p| p.child(Page::Provision, "Policy"))
        .and_then(|p| p.child_text(Page::Provision, "PolicyKey"))
        .unwrap()
}

fn folder_sync(key: &str) -> Element {
    Element::new(Page::FolderHierarchy, "FolderSync").with_child(t(
        Page::FolderHierarchy,
        "SyncKey",
        key,
    ))
}

fn sync(key: &str, cid: &str, window: Option<&str>, commands: Vec<Element>) -> Element {
    let mut c = Element::new(Page::AirSync, "Collection")
        .with_child(t(Page::AirSync, "SyncKey", key))
        .with_child(t(Page::AirSync, "CollectionId", cid));
    if let Some(w) = window {
        c = c.with_child(t(Page::AirSync, "WindowSize", w));
    }
    if key == "0" {
        c = c.with_child(Element::new(Page::AirSync, "Options").with_child(
            Element::new(Page::AirSyncBase, "BodyPreference").with_child(t(
                Page::AirSyncBase,
                "Type",
                "2",
            )),
        ));
    }
    if !commands.is_empty() {
        let mut cmds = Element::new(Page::AirSync, "Commands");
        for x in commands {
            cmds.push(x);
        }
        c.push(cmds);
    }
    Element::new(Page::AirSync, "Sync")
        .with_child(Element::new(Page::AirSync, "Collections").with_child(c))
}

fn collection(resp: &Element) -> &Element {
    resp.child(Page::AirSync, "Collections")
        .and_then(|c| c.child(Page::AirSync, "Collection"))
        .expect("collection")
}

fn key_of(coll: &Element) -> String {
    coll.child_text(Page::AirSync, "SyncKey").unwrap()
}

fn commands<'a>(coll: &'a Element, name: &'a str) -> Vec<&'a Element> {
    coll.child(Page::AirSync, "Commands")
        .map(|c| c.children_named(Page::AirSync, name).collect())
        .unwrap_or_default()
}

#[tokio::test]
async fn provision_folder_sync_and_mail_sync() {
    let (fake, url) = start_fake().await;
    {
        let mut f = fake.lock().unwrap();
        f.add("e1", "inbox", "2026-10-01T10:00:00Z");
        f.add("e2", "inbox", "2026-10-02T10:00:00Z");
        f.add("e3", "inbox", "2026-10-03T10:00:00Z");
        f.add("e4", "sub", "2026-10-03T11:00:00Z");
    }
    let mut gw = Gateway::new(&url).await;

    // Without a policy key, commands are refused with status 142.
    let resp = gw.ok("FolderSync", &folder_sync("0")).await;
    assert_eq!(
        resp.child_text(Page::FolderHierarchy, "Status").as_deref(),
        Some("142")
    );

    // Two-step provisioning.
    let resp = gw.ok("Provision", &provision_request(None)).await;
    let temp = policy_key(&resp);
    assert!(resp.to_xml().contains("EASProvisionDoc"));
    let resp = gw.ok("Provision", &provision_request(Some(&temp))).await;
    let final_key = policy_key(&resp);
    assert_ne!(temp, final_key);
    gw.policy_key = Some(temp.clone());
    let resp = gw.ok("FolderSync", &folder_sync("0")).await;
    assert_eq!(
        resp.child_text(Page::FolderHierarchy, "Status").as_deref(),
        Some("144"),
        "the temporary key is not valid for commands"
    );
    gw.policy_key = Some(final_key);

    // Folder tree: parents first, roles mapped to types.
    let resp = gw.ok("FolderSync", &folder_sync("0")).await;
    assert_eq!(
        resp.child_text(Page::FolderHierarchy, "Status").as_deref(),
        Some("1")
    );
    let fkey = resp.child_text(Page::FolderHierarchy, "SyncKey").unwrap();
    let changes = resp.child(Page::FolderHierarchy, "Changes").unwrap();
    assert_eq!(
        changes
            .child_text(Page::FolderHierarchy, "Count")
            .as_deref(),
        Some("3")
    );
    let adds: Vec<_> = changes
        .children_named(Page::FolderHierarchy, "Add")
        .collect();
    let find = |name: &str| {
        adds.iter()
            .find(|a| {
                a.child_text(Page::FolderHierarchy, "DisplayName")
                    .as_deref()
                    == Some(name)
            })
            .unwrap()
    };
    let inbox_id = find("Inbox")
        .child_text(Page::FolderHierarchy, "ServerId")
        .unwrap();
    assert_eq!(
        find("Inbox")
            .child_text(Page::FolderHierarchy, "Type")
            .as_deref(),
        Some("2")
    );
    assert_eq!(
        find("Trash")
            .child_text(Page::FolderHierarchy, "Type")
            .as_deref(),
        Some("4")
    );
    assert_eq!(
        find("Projects").child_text(Page::FolderHierarchy, "ParentId"),
        Some(inbox_id.clone())
    );
    // Unchanged tree: same key, no changes.
    let resp = gw.ok("FolderSync", &folder_sync(&fkey)).await;
    assert_eq!(
        resp.child_text(Page::FolderHierarchy, "SyncKey"),
        Some(fkey.clone())
    );
    // Unknown key: 9.
    let resp = gw.ok("FolderSync", &folder_sync("bogus")).await;
    assert_eq!(
        resp.child_text(Page::FolderHierarchy, "Status").as_deref(),
        Some("9")
    );

    // Initial Sync returns a key and nothing else.
    let resp = gw.ok("Sync", &sync("0", &inbox_id, None, vec![])).await;
    let coll = collection(&resp);
    assert_eq!(
        coll.child_text(Page::AirSync, "Status").as_deref(),
        Some("1")
    );
    assert!(coll.child(Page::AirSync, "Commands").is_none());
    let k1 = key_of(coll);

    // Window of 2: newest first, MoreAvailable.
    let resp = gw
        .ok("Sync", &sync(&k1, &inbox_id, Some("2"), vec![]))
        .await;
    let coll = collection(&resp).clone();
    let k2 = key_of(&coll);
    assert!(coll.child(Page::AirSync, "MoreAvailable").is_some());
    let added: Vec<String> = commands(&coll, "Add")
        .iter()
        .map(|a| a.child_text(Page::AirSync, "ServerId").unwrap())
        .collect();
    assert_eq!(
        added,
        vec![format!("{inbox_id}:e3"), format!("{inbox_id}:e2")]
    );
    let data = commands(&coll, "Add")[0]
        .child(Page::AirSync, "ApplicationData")
        .unwrap();
    assert_eq!(
        data.child_text(Page::Email, "Subject").as_deref(),
        Some("Subject e3")
    );
    let body = data.child(Page::AirSyncBase, "Body").unwrap();
    assert_eq!(
        body.child_text(Page::AirSyncBase, "Type").as_deref(),
        Some("2")
    );

    // A retry with the previous key replays the same answer.
    let resp = gw
        .ok("Sync", &sync(&k1, &inbox_id, Some("2"), vec![]))
        .await;
    assert_eq!(collection(&resp), &coll);

    // The rest; options were remembered from SyncKey 0.
    let resp = gw
        .ok("Sync", &sync(&k2, &inbox_id, Some("2"), vec![]))
        .await;
    let coll = collection(&resp).clone();
    assert!(coll.child(Page::AirSync, "MoreAvailable").is_none());
    assert_eq!(commands(&coll, "Add").len(), 1);
    let k3 = key_of(&coll);

    // Nothing new: empty response, key unchanged.
    let (status, body) = gw
        .send("Sync", Some(&sync(&k3, &inbox_id, None, vec![])))
        .await;
    assert_eq!(status, StatusCode::OK);
    assert!(body.is_none());

    // Server-side flag: one Change.
    {
        let mut f = fake.lock().unwrap();
        f.email("e1")["keywords"]["$flagged"] = json!(true);
        f.bump("e1");
    }
    let resp = gw.ok("Sync", &sync(&k3, &inbox_id, None, vec![])).await;
    let coll = collection(&resp).clone();
    let changes = commands(&coll, "Change");
    assert_eq!(changes.len(), 1);
    assert_eq!(
        changes[0].child_text(Page::AirSync, "ServerId"),
        Some(format!("{inbox_id}:e1"))
    );
    let k4 = key_of(&coll);

    // Device marks e2 read and deletes e3: read lands in JMAP, delete moves
    // to Trash, and neither is echoed back.
    let read = Element::new(Page::AirSync, "Change")
        .with_child(t(Page::AirSync, "ServerId", &format!("{inbox_id}:e2")))
        .with_child(Element::new(Page::AirSync, "ApplicationData").with_child(t(
            Page::Email,
            "Read",
            "1",
        )));
    let del = Element::new(Page::AirSync, "Delete").with_child(t(
        Page::AirSync,
        "ServerId",
        &format!("{inbox_id}:e3"),
    ));
    let resp = gw
        .ok("Sync", &sync(&k4, &inbox_id, None, vec![read, del]))
        .await;
    let coll = collection(&resp).clone();
    assert!(
        coll.child(Page::AirSync, "Commands").is_none(),
        "{}",
        coll.to_xml()
    );
    assert!(coll.child(Page::AirSync, "Responses").is_none());
    let k5 = key_of(&coll);
    {
        let mut f = fake.lock().unwrap();
        assert_eq!(f.email("e2")["keywords"]["$seen"], json!(true));
        assert_eq!(f.email("e3")["mailboxIds"], json!({"trash": true}));
    }
    let (_, body) = gw
        .send("Sync", Some(&sync(&k5, &inbox_id, None, vec![])))
        .await;
    assert!(body.is_none());

    // Estimate after a new message: 1.
    fake.lock()
        .unwrap()
        .add("e5", "inbox", "2026-10-04T10:00:00Z");
    let est = Element::new(Page::GetItemEstimate, "GetItemEstimate").with_child(
        Element::new(Page::GetItemEstimate, "Collections").with_child(
            Element::new(Page::GetItemEstimate, "Collection")
                .with_child(t(Page::AirSync, "SyncKey", &k5))
                .with_child(t(Page::GetItemEstimate, "CollectionId", &inbox_id)),
        ),
    );
    let resp = gw.ok("GetItemEstimate", &est).await;
    let r = resp.child(Page::GetItemEstimate, "Response").unwrap();
    assert_eq!(
        r.child_text(Page::GetItemEstimate, "Status").as_deref(),
        Some("1")
    );
    assert_eq!(
        r.child(Page::GetItemEstimate, "Collection")
            .unwrap()
            .child_text(Page::GetItemEstimate, "Estimate")
            .as_deref(),
        Some("1")
    );

    // An empty Sync repeats the last request at the current key.
    let (_, body) = gw.send("Sync", None).await;
    let body = body.unwrap();
    let coll = collection(&body);
    assert_eq!(commands(coll, "Add").len(), 1);

    // Unknown collection: hierarchy changed.
    let resp = gw.ok("Sync", &sync("1-x", "999", None, vec![])).await;
    assert_eq!(
        collection(&resp)
            .child_text(Page::AirSync, "Status")
            .as_deref(),
        Some("12")
    );
}

fn ping(heartbeat: Option<&str>, folders: &[&str]) -> Element {
    let mut p = Element::new(Page::Ping, "Ping");
    if let Some(h) = heartbeat {
        p.push(t(Page::Ping, "HeartbeatInterval", h));
    }
    if !folders.is_empty() {
        let mut fs = Element::new(Page::Ping, "Folders");
        for f in folders {
            fs.push(
                Element::new(Page::Ping, "Folder")
                    .with_child(t(Page::Ping, "Id", f))
                    .with_child(t(Page::Ping, "Class", "Email")),
            );
        }
        p.push(fs);
    }
    p
}

/// Provisioned gateway with the inbox fully synced; returns the inbox ID.
async fn synced_gateway(fake: &Shared, url: &str) -> (Gateway, String) {
    fake.lock().unwrap().add("e1", "inbox", "2026-10-01T10:00:00Z");
    let mut gw = Gateway::new(url).await;
    let temp = policy_key(&gw.ok("Provision", &provision_request(None)).await);
    let key = policy_key(&gw.ok("Provision", &provision_request(Some(&temp))).await);
    gw.policy_key = Some(key);
    let resp = gw.ok("FolderSync", &folder_sync("0")).await;
    let inbox = resp
        .child(Page::FolderHierarchy, "Changes")
        .unwrap()
        .children_named(Page::FolderHierarchy, "Add")
        .find(|a| a.child_text(Page::FolderHierarchy, "Type").as_deref() == Some("2"))
        .unwrap()
        .child_text(Page::FolderHierarchy, "ServerId")
        .unwrap();
    let k = key_of(collection(&gw.ok("Sync", &sync("0", &inbox, None, vec![])).await));
    let resp = gw.ok("Sync", &sync(&k, &inbox, None, vec![])).await;
    assert_eq!(commands(collection(&resp), "Add").len(), 1);
    (gw, inbox)
}

fn ping_status(resp: &Element) -> String {
    resp.child_text(Page::Ping, "Status").unwrap()
}

#[tokio::test]
async fn ping_wakes_on_push_and_validates() {
    let (fake, url) = start_fake().await;
    let (gw, inbox) = synced_gateway(&fake, &url).await;

    // Parameter checks.
    let (_, resp) = gw.send("Ping", None).await;
    assert_eq!(ping_status(&resp.unwrap()), "3");
    let resp = gw.ok("Ping", &ping(Some("10"), &[&inbox])).await;
    assert_eq!(ping_status(&resp), "5");
    assert_eq!(
        resp.child_text(Page::Ping, "HeartbeatInterval").as_deref(),
        Some("60")
    );
    let resp = gw.ok("Ping", &ping(Some("60"), &["999"])).await;
    assert_eq!(ping_status(&resp), "7");

    // Nothing pending: the Ping waits until push reports new mail.
    let gw = Arc::new(gw);
    let started = std::time::Instant::now();
    let pinging = {
        let gw = gw.clone();
        let body = ping(Some("600"), &[&inbox]);
        tokio::spawn(async move { gw.ok("Ping", &body).await })
    };
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert!(!pinging.is_finished(), "Ping returned before any change");
    fake.lock().unwrap().add("e2", "inbox", "2026-10-02T10:00:00Z");
    let resp = tokio::time::timeout(Duration::from_secs(10), pinging)
        .await
        .expect("Ping woken by push")
        .unwrap();
    assert_eq!(ping_status(&resp), "2");
    let folders = resp.child(Page::Ping, "Folders").unwrap();
    assert_eq!(folders.child_text(Page::Ping, "Folder"), Some(inbox.clone()));
    assert!(started.elapsed() < Duration::from_secs(10));

    // Pending changes answer at once; an empty Ping reuses the last folders.
    let (_, resp) = gw.send("Ping", None).await;
    assert_eq!(ping_status(&resp.unwrap()), "2");
}

#[tokio::test]
async fn new_ping_ends_the_waiting_one() {
    let (fake, url) = start_fake().await;
    let (gw, inbox) = synced_gateway(&fake, &url).await;
    let gw = Arc::new(gw);
    let first = {
        let gw = gw.clone();
        let body = ping(Some("600"), &[&inbox]);
        tokio::spawn(async move { gw.ok("Ping", &body).await })
    };
    tokio::time::sleep(Duration::from_millis(300)).await;
    let second = {
        let gw = gw.clone();
        let body = ping(Some("600"), &[&inbox]);
        tokio::spawn(async move { gw.ok("Ping", &body).await })
    };
    let resp = tokio::time::timeout(Duration::from_secs(5), first)
        .await
        .expect("first Ping ended")
        .unwrap();
    assert_eq!(ping_status(&resp), "1");
    second.abort();
}
