//! `stalwart-activesync probe`: plays a phone against a running gateway, for
//! checking a deployment end to end.
//!
//! OPTIONS, the two Provision round trips, FolderSync, then an initial Sync
//! of the inbox, contacts and calendar, printing what came back. It uses a
//! fixed device ID, so it shows up once in the admin API however often it
//! runs.

use eas_wbxml::{Element, Page};
use std::time::Duration;

const USAGE: &str =
    "usage: stalwart-activesync probe <https://mail.example.org> <user> <password> [--insecure]";

struct Probe {
    http: reqwest::Client,
    url: String,
    user: String,
    password: String,
    policy_key: Option<String>,
}

impl Probe {
    async fn send(&self, cmd: &str, body: &Element) -> Result<Option<Element>, String> {
        let url = format!(
            "{}/Microsoft-Server-ActiveSync?Cmd={cmd}&User={}&DeviceId=EasProbe01&DeviceType=Probe",
            self.url, self.user
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
        let resp = req.send().await.map_err(|e| format!("{cmd}: {e}"))?;
        let status = resp.status();
        let bytes = resp.bytes().await.map_err(|e| format!("{cmd}: {e}"))?;
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
}

fn t(page: Page, name: &str, v: &str) -> Element {
    Element::new(page, name).with_text(v)
}

fn provision(key: Option<&str>) -> Element {
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

fn policy_key(resp: &Element) -> Option<String> {
    resp.child(Page::Provision, "Policies")?
        .child(Page::Provision, "Policy")?
        .child_text(Page::Provision, "PolicyKey")
}

fn sync(key: &str, cid: &str) -> Element {
    let mut c = Element::new(Page::AirSync, "Collection")
        .with_child(t(Page::AirSync, "SyncKey", key))
        .with_child(t(Page::AirSync, "CollectionId", cid));
    if key != "0" {
        c = c.with_child(t(Page::AirSync, "WindowSize", "5"));
    }
    Element::new(Page::AirSync, "Sync")
        .with_child(Element::new(Page::AirSync, "Collections").with_child(c))
}

fn collection(resp: &Element) -> Option<&Element> {
    resp.child(Page::AirSync, "Collections")?
        .child(Page::AirSync, "Collection")
}

/// A one-line description of an item for the report.
fn describe(data: &Element) -> String {
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

pub async fn run(args: &[String]) -> Result<(), String> {
    let insecure = args.iter().any(|a| a == "--insecure");
    let pos: Vec<&String> = args.iter().filter(|a| !a.starts_with("--")).collect();
    let [url, user, password] = pos.as_slice() else {
        return Err(USAGE.into());
    };
    let http = reqwest::Client::builder()
        .danger_accept_invalid_certs(insecure)
        .timeout(Duration::from_secs(60))
        .build()
        .map_err(|e| e.to_string())?;
    let mut p = Probe {
        http,
        url: url.trim_end_matches('/').to_string(),
        user: user.to_string(),
        password: password.to_string(),
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
        .map_err(|e| format!("OPTIONS: {e}"))?;
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

    for (wanted, label) in [("2", "Inbox"), ("9", "Contacts"), ("8", "Calendar")] {
        let Some((cid, _, name)) = folders.iter().find(|(_, ty, _)| ty == wanted) else {
            println!("Sync {label:<9} no such folder");
            continue;
        };
        let resp = p
            .send("Sync", &sync("0", cid))
            .await?
            .ok_or("Sync: empty")?;
        let k = collection(&resp)
            .and_then(|c| c.child_text(Page::AirSync, "SyncKey"))
            .ok_or_else(|| format!("Sync {name}: {}", resp.to_xml()))?;
        let resp = p.send("Sync", &sync(&k, cid)).await?;
        let adds: Vec<String> = resp
            .as_ref()
            .and_then(collection)
            .and_then(|c| c.child(Page::AirSync, "Commands"))
            .map(|c| {
                c.children_named(Page::AirSync, "Add")
                    .filter_map(|a| a.child(Page::AirSync, "ApplicationData"))
                    .map(describe)
                    .collect()
            })
            .unwrap_or_default();
        let more = resp
            .as_ref()
            .and_then(collection)
            .is_some_and(|c| c.child(Page::AirSync, "MoreAvailable").is_some());
        println!(
            "Sync {label:<9} {} item(s){}",
            adds.len(),
            if more { ", more available" } else { "" }
        );
        for a in adds {
            println!("  - {a}");
        }
    }
    println!("probe finished");
    Ok(())
}
