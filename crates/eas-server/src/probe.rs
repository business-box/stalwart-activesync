//! `stalwart-activesync probe`: plays a phone against a running gateway, for
//! checking a deployment end to end.
//!
//! By default it reads: OPTIONS, the two Provision round trips, FolderSync,
//! then an initial Sync of the inbox, contacts and calendar. With `--full`
//! it also writes and waits for push:
//!
//! - a `Ping` on the inbox, then `SendMail` to the probe's own address; the
//!   Ping must report the inbox and the next Sync must bring the message;
//! - a contact and an event added through Sync, then deleted again.
//!
//! It uses a fixed device ID, so it shows up once in the admin API however
//! often it runs. Exit status 1 on the first failure, for CI.

use eas_wbxml::{Element, Page};
use std::time::{Duration, Instant};

const USAGE: &str = "usage: stalwart-activesync probe <https://mail.example.org> <user> <password> [--insecure] [--full]";

#[derive(Clone)]
pub(crate) struct Probe {
    pub http: reqwest::Client,
    pub url: String,
    pub user: String,
    pub password: String,
    pub device: String,
    pub policy_key: Option<String>,
}

/// An error with its whole cause chain, which reqwest otherwise hides.
pub(crate) fn chain(e: &dyn std::error::Error) -> String {
    let mut s = e.to_string();
    let mut cur = e.source();
    while let Some(c) = cur {
        s.push_str(": ");
        s.push_str(&c.to_string());
        cur = c.source();
    }
    s
}

impl Probe {
    pub async fn send(&self, cmd: &str, body: &Element) -> Result<Option<Element>, String> {
        let url = format!(
            "{}/Microsoft-Server-ActiveSync?Cmd={cmd}&User={}&DeviceId={}&DeviceType=Probe",
            self.url, self.user, self.device
        );
        let mut req = self
            .http
            .post(&url)
            .basic_auth(&self.user, Some(&self.password))
            .header("MS-ASProtocolVersion", "16.1")
            .header("Content-Type", eas_wbxml::CONTENT_TYPE)
            .body(eas_wbxml::encode(body));
        if let Some(k) = &self.policy_key {
            req = req.header("X-MS-PolicyKey", k);
        }
        let resp = req
            .send()
            .await
            .map_err(|e| format!("{cmd}: {}", chain(&e)))?;
        let status = resp.status();
        let bytes = resp
            .bytes()
            .await
            .map_err(|e| format!("{cmd}: {}", chain(&e)))?;
        if !status.is_success() {
            return Err(format!(
                "{cmd}: HTTP {status} {}",
                String::from_utf8_lossy(&bytes)
            ));
        }
        if bytes.is_empty() {
            return Ok(None);
        }
        eas_wbxml::decode(&bytes)
            .map(Some)
            .map_err(|e| format!("{cmd}: undecodable response: {e}"))
    }

    /// One Sync round on a folder: optional client commands, returns the
    /// collection element (None when nothing changed) and the new key.
    pub async fn sync(
        &self,
        key: &str,
        cid: &str,
        commands: Vec<Element>,
    ) -> Result<(Option<Element>, String), String> {
        let mut c = Element::new(Page::AirSync, "Collection")
            .with_child(t(Page::AirSync, "SyncKey", key))
            .with_child(t(Page::AirSync, "CollectionId", cid));
        if key != "0" {
            c = c.with_child(t(Page::AirSync, "WindowSize", "25"));
        }
        if !commands.is_empty() {
            let mut cmds = Element::new(Page::AirSync, "Commands");
            for x in commands {
                cmds.push(x);
            }
            c.push(cmds);
        }
        let req = Element::new(Page::AirSync, "Sync")
            .with_child(Element::new(Page::AirSync, "Collections").with_child(c));
        let resp = self.send("Sync", &req).await?;
        let coll = resp.as_ref().and_then(collection).cloned();
        if let Some(c) = &coll {
            let status = c.child_text(Page::AirSync, "Status").unwrap_or_default();
            if status != "1" {
                return Err(format!("Sync {cid}: status {status}"));
            }
        }
        let new_key = coll
            .as_ref()
            .and_then(|c| c.child_text(Page::AirSync, "SyncKey"))
            .unwrap_or_else(|| key.to_string());
        Ok((coll, new_key))
    }

    /// Syncs until nothing more is available; returns the key and every
    /// added item's ApplicationData.
    pub async fn sync_all(&self, cid: &str) -> Result<(String, Vec<Element>), String> {
        let (_, mut key) = self.sync("0", cid, vec![]).await?;
        let mut items = Vec::new();
        for _ in 0..50 {
            let (coll, k) = self.sync(&key, cid, vec![]).await?;
            key = k;
            let Some(c) = coll else { break };
            items.extend(adds(&c));
            if c.child(Page::AirSync, "MoreAvailable").is_none() {
                break;
            }
        }
        Ok((key, items))
    }
}

pub(crate) fn t(page: Page, name: &str, v: &str) -> Element {
    Element::new(page, name).with_text(v)
}

pub(crate) fn provision(key: Option<&str>) -> Element {
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

pub(crate) fn policy_key(resp: &Element) -> Option<String> {
    resp.child(Page::Provision, "Policies")?
        .child(Page::Provision, "Policy")?
        .child_text(Page::Provision, "PolicyKey")
}

fn collection(resp: &Element) -> Option<&Element> {
    resp.child(Page::AirSync, "Collections")?
        .child(Page::AirSync, "Collection")
}

pub(crate) fn adds(coll: &Element) -> Vec<Element> {
    coll.child(Page::AirSync, "Commands")
        .map(|c| {
            c.children_named(Page::AirSync, "Add")
                .filter_map(|a| a.child(Page::AirSync, "ApplicationData").cloned())
                .collect()
        })
        .unwrap_or_default()
}

/// A one-line description of an item for the report.
pub(crate) fn describe(data: &Element) -> String {
    for (page, name) in [
        (Page::Email, "Subject"),
        (Page::Calendar, "Subject"),
        (Page::Contacts, "FileAs"),
    ] {
        if let Some(v) = data.child_text(page, name) {
            return v;
        }
    }
    "(no title)".into()
}

/// Adds an item through Sync and deletes it again; returns its server ID.
async fn add_and_delete(p: &Probe, label: &str, cid: &str, data: Element) -> Result<(), String> {
    let (_, key) = p.sync("0", cid, vec![]).await?;
    let (_, key) = p.sync(&key, cid, vec![]).await?;
    let add = Element::new(Page::AirSync, "Add")
        .with_child(t(Page::AirSync, "ClientId", &format!("probe-{label}")))
        .with_child(data);
    let (coll, key) = p.sync(&key, cid, vec![add]).await?;
    let resp = coll
        .as_ref()
        .and_then(|c| c.child(Page::AirSync, "Responses"))
        .and_then(|r| r.child(Page::AirSync, "Add"))
        .ok_or_else(|| format!("{label}: no Add response"))?;
    let status = resp.child_text(Page::AirSync, "Status").unwrap_or_default();
    let sid = resp
        .child_text(Page::AirSync, "ServerId")
        .filter(|_| status == "1")
        .ok_or_else(|| format!("{label}: add failed with status {status}"))?;
    let del = Element::new(Page::AirSync, "Delete").with_child(t(Page::AirSync, "ServerId", &sid));
    p.sync(&key, cid, vec![del]).await?;
    println!("Write {label:<9} added {sid}, deleted again");
    Ok(())
}

pub async fn run(args: &[String]) -> Result<(), String> {
    let insecure = args.iter().any(|a| a == "--insecure");
    let full = args.iter().any(|a| a == "--full");
    let pos: Vec<&String> = args.iter().filter(|a| !a.starts_with("--")).collect();
    let [url, user, password] = pos.as_slice() else {
        return Err(USAGE.into());
    };
    let http = reqwest::Client::builder()
        .danger_accept_invalid_certs(insecure)
        .timeout(Duration::from_secs(300))
        .build()
        .map_err(|e| e.to_string())?;
    let mut p = Probe {
        http,
        url: url.trim_end_matches('/').to_string(),
        user: user.to_string(),
        password: password.to_string(),
        device: "EasProbe01".into(),
        policy_key: None,
    };

    let opts = p
        .http
        .request(
            reqwest::Method::OPTIONS,
            format!("{}/Microsoft-Server-ActiveSync", p.url),
        )
        .send()
        .await
        .map_err(|e| format!("OPTIONS: {}", chain(&e)))?;
    let versions = opts
        .headers()
        .get("ms-asprotocolversions")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("(none)")
        .to_string();
    println!("OPTIONS        HTTP {} versions {versions}", opts.status());

    let resp = p
        .send("Provision", &provision(None))
        .await?
        .ok_or("Provision: empty response")?;
    let temp =
        policy_key(&resp).ok_or_else(|| format!("Provision: no key in {}", resp.to_xml()))?;
    let resp = p
        .send("Provision", &provision(Some(&temp)))
        .await?
        .ok_or("Provision: empty response")?;
    let key =
        policy_key(&resp).ok_or_else(|| format!("Provision: no final key in {}", resp.to_xml()))?;
    println!("Provision      ok, policy key {key}");
    p.policy_key = Some(key);

    let resp = p
        .send(
            "FolderSync",
            &Element::new(Page::FolderHierarchy, "FolderSync").with_child(t(
                Page::FolderHierarchy,
                "SyncKey",
                "0",
            )),
        )
        .await?
        .ok_or("FolderSync: empty response")?;
    let status = resp
        .child_text(Page::FolderHierarchy, "Status")
        .unwrap_or_default();
    if status != "1" {
        return Err(format!("FolderSync: status {status}"));
    }
    let folders: Vec<(String, String, String)> = resp
        .child(Page::FolderHierarchy, "Changes")
        .map(|c| {
            c.children_named(Page::FolderHierarchy, "Add")
                .map(|a| {
                    let f = |n: &str| a.child_text(Page::FolderHierarchy, n).unwrap_or_default();
                    (f("ServerId"), f("Type"), f("DisplayName"))
                })
                .collect()
        })
        .unwrap_or_default();
    println!("FolderSync     {} folders", folders.len());
    for (id, ty, name) in &folders {
        println!("  {id:>4}  type {ty:>2}  {name}");
    }
    let folder_of = |ty: &str| {
        folders
            .iter()
            .find(|(_, t, _)| t == ty)
            .map(|f| f.0.clone())
    };

    let mut inbox_key = None;
    for (wanted, label) in [("2", "Inbox"), ("9", "Contacts"), ("8", "Calendar")] {
        let Some(cid) = folder_of(wanted) else {
            println!("Sync {label:<9} no such folder");
            continue;
        };
        let (key, items) = p.sync_all(&cid).await?;
        if wanted == "2" {
            inbox_key = Some((cid.clone(), key));
        }
        println!("Sync {label:<9} {} item(s)", items.len());
        for a in items.iter().take(5) {
            println!("  - {}", describe(a));
        }
    }

    if full {
        let (inbox, key) = inbox_key.ok_or("--full needs an inbox")?;
        push_and_send(&p, &inbox, &key).await?;
        if let Some(cid) = folder_of("9") {
            let data = Element::new(Page::AirSync, "ApplicationData")
                .with_child(t(Page::Contacts, "FirstName", "Probe"))
                .with_child(t(Page::Contacts, "LastName", "Contact"))
                .with_child(t(Page::Contacts, "Email1Address", "probe@example.invalid"));
            add_and_delete(&p, "contact", &cid, data).await?;
        }
        if let Some(cid) = folder_of("8") {
            let start = chrono::Utc::now() + chrono::Duration::days(1);
            let fmt = |d: chrono::DateTime<chrono::Utc>| d.format("%Y%m%dT%H0000Z").to_string();
            let data = Element::new(Page::AirSync, "ApplicationData")
                .with_child(t(Page::Calendar, "Subject", "Probe event"))
                .with_child(t(Page::Calendar, "StartTime", &fmt(start)))
                .with_child(t(
                    Page::Calendar,
                    "EndTime",
                    &fmt(start + chrono::Duration::hours(1)),
                ))
                .with_child(t(Page::Calendar, "BusyStatus", "2"));
            add_and_delete(&p, "event", &cid, data).await?;
        }
    }
    println!("probe finished");
    Ok(())
}

/// Ping on the inbox, send a message to ourselves, and require the Ping to
/// report the inbox and the next Sync to bring the message.
async fn push_and_send(p: &Probe, inbox: &str, key: &str) -> Result<(), String> {
    let subject = format!(
        "Probe {}",
        chrono::Utc::now().format("%Y-%m-%d %H:%M:%S%.3f")
    );
    let ping = Element::new(Page::Ping, "Ping")
        .with_child(t(Page::Ping, "HeartbeatInterval", "120"))
        .with_child(
            Element::new(Page::Ping, "Folders").with_child(
                Element::new(Page::Ping, "Folder")
                    .with_child(t(Page::Ping, "Id", inbox))
                    .with_child(t(Page::Ping, "Class", "Email")),
            ),
        );
    let pinger = p.clone();
    let started = Instant::now();
    let waiting = tokio::spawn(async move { pinger.send("Ping", &ping).await });
    // Let the Ping reach the gateway and subscribe before the mail exists.
    tokio::time::sleep(Duration::from_secs(2)).await;
    if waiting.is_finished() {
        let early = waiting.await.map_err(|e| e.to_string())??;
        return Err(format!(
            "Ping returned before any change: {}",
            early.map(|e| e.to_xml()).unwrap_or_default()
        ));
    }

    let mime = format!(
        "From: {u}\r\nTo: {u}\r\nSubject: {subject}\r\nMIME-Version: 1.0\r\nContent-Type: text/plain; charset=utf-8\r\n\r\nSent by stalwart-activesync probe.\r\n",
        u = p.user
    );
    let send = Element::new(Page::ComposeMail, "SendMail")
        .with_child(t(
            Page::ComposeMail,
            "ClientId",
            &format!("probe-{}", started.elapsed().as_nanos()),
        ))
        .with_child(Element::new(Page::ComposeMail, "SaveInSentItems"))
        .with_child(Element::new(Page::ComposeMail, "Mime").with_opaque(mime.into_bytes()));
    if let Some(resp) = p.send("SendMail", &send).await? {
        return Err(format!("SendMail failed: {}", resp.to_xml()));
    }
    println!("SendMail       ok, \"{subject}\" to {}", p.user);

    let resp = tokio::time::timeout(Duration::from_secs(90), waiting)
        .await
        .map_err(|_| "Ping not woken within 90 s of sending".to_string())?
        .map_err(|e| e.to_string())??
        .ok_or("Ping: empty response")?;
    let status = resp.child_text(Page::Ping, "Status").unwrap_or_default();
    if status != "2" {
        return Err(format!("Ping: status {status}, expected 2"));
    }
    println!(
        "Ping           woken after {:.1} s",
        started.elapsed().as_secs_f32()
    );

    let mut key = key.to_string();
    for _ in 0..10 {
        let (coll, k) = p.sync(&key, inbox, vec![]).await?;
        key = k;
        if coll
            .as_ref()
            .is_some_and(|c| adds(c).iter().any(|a| describe(a) == subject))
        {
            println!("Sync Inbox     new message arrived");
            return Ok(());
        }
        tokio::time::sleep(Duration::from_secs(2)).await;
    }
    Err(format!("\"{subject}\" never arrived in the inbox"))
}
