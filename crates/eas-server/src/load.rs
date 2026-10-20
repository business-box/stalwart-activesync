//! `stalwart-activesync load`: many phones holding `Ping`s at once.
//!
//! Each of `--accounts` users (`<prefix><n>@<domain>`, all with one password)
//! gets `--devices` phones. Every phone provisions, syncs its inbox and then
//! holds a Ping. Once all are waiting, one message is sent to each account;
//! every Ping of that account must wake with status 2. The report gives
//! setup time, wake-up latency percentiles and failures. Exit status 1 when
//! any phone failed.

use crate::probe::{chain, policy_key, provision, t, Probe};
use eas_wbxml::{Element, Page};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::Semaphore;

const USAGE: &str = "usage: stalwart-activesync load <url> <password> --accounts N --devices M [--prefix load] [--domain example.test] [--heartbeat 300] [--setup-concurrency 25] [--insecure]";

struct Opts {
    url: String,
    password: String,
    accounts: usize,
    devices: usize,
    prefix: String,
    domain: String,
    heartbeat: u64,
    concurrency: usize,
    insecure: bool,
}

fn parse(args: &[String]) -> Result<Opts, String> {
    let mut pos = Vec::new();
    let mut o = Opts {
        url: String::new(),
        password: String::new(),
        accounts: 10,
        devices: 1,
        prefix: "load".into(),
        domain: "example.test".into(),
        heartbeat: 300,
        concurrency: 25,
        insecure: false,
    };
    let mut it = args.iter();
    while let Some(a) = it.next() {
        let mut val = || {
            it.next()
                .cloned()
                .ok_or_else(|| format!("{a} needs a value"))
        };
        let num = |v: String| v.parse::<usize>().map_err(|e| format!("{a}: {e}"));
        match a.as_str() {
            "--accounts" => o.accounts = num(val()?)?,
            "--devices" => o.devices = num(val()?)?,
            "--prefix" => o.prefix = val()?,
            "--domain" => o.domain = val()?,
            "--heartbeat" => o.heartbeat = num(val()?)? as u64,
            "--setup-concurrency" => o.concurrency = num(val()?)?.max(1),
            "--insecure" => o.insecure = true,
            _ if a.starts_with("--") => return Err(format!("unknown option {a}\n{USAGE}")),
            _ => pos.push(a.clone()),
        }
    }
    let [url, password] = pos.as_slice() else {
        return Err(USAGE.into());
    };
    o.url = url.trim_end_matches('/').into();
    o.password = password.clone();
    Ok(o)
}

/// A phone ready to Ping: its probe and inbox ID.
struct Phone {
    probe: Probe,
    inbox: String,
}

async fn setup(mut p: Probe) -> Result<Phone, String> {
    let resp = p
        .send("Provision", &provision(None))
        .await?
        .ok_or("Provision: empty")?;
    let temp = policy_key(&resp).ok_or("Provision: no key")?;
    let resp = p
        .send("Provision", &provision(Some(&temp)))
        .await?
        .ok_or("Provision: empty")?;
    p.policy_key = Some(policy_key(&resp).ok_or("Provision: no final key")?);
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
        .ok_or("FolderSync: empty")?;
    let inbox = resp
        .child(Page::FolderHierarchy, "Changes")
        .and_then(|c| {
            c.children_named(Page::FolderHierarchy, "Add")
                .find(|a| a.child_text(Page::FolderHierarchy, "Type").as_deref() == Some("2"))
                .and_then(|a| a.child_text(Page::FolderHierarchy, "ServerId"))
        })
        .ok_or("FolderSync: no inbox")?;
    p.sync_all(&inbox).await?;
    Ok(Phone { probe: p, inbox })
}

fn ping_body(inbox: &str, heartbeat: u64) -> Element {
    Element::new(Page::Ping, "Ping")
        .with_child(t(Page::Ping, "HeartbeatInterval", &heartbeat.to_string()))
        .with_child(
            Element::new(Page::Ping, "Folders").with_child(
                Element::new(Page::Ping, "Folder")
                    .with_child(t(Page::Ping, "Id", inbox))
                    .with_child(t(Page::Ping, "Class", "Email")),
            ),
        )
}

fn percentile(sorted: &[f64], q: f64) -> f64 {
    if sorted.is_empty() {
        return 0.0;
    }
    let i = ((sorted.len() as f64 - 1.0) * q).round() as usize;
    sorted[i.min(sorted.len() - 1)]
}

pub async fn run(args: &[String]) -> Result<(), String> {
    let o = parse(args)?;
    let total = o.accounts * o.devices;
    let http = reqwest::Client::builder()
        .danger_accept_invalid_certs(o.insecure)
        .timeout(Duration::from_secs(o.heartbeat + 120))
        .pool_max_idle_per_host(total + 10)
        .build()
        .map_err(|e| chain(&e))?;
    println!(
        "load: {} accounts x {} devices = {total} phones against {}",
        o.accounts, o.devices, o.url
    );

    // Set up every phone, a bounded number at a time.
    let started = Instant::now();
    let sem = Arc::new(Semaphore::new(o.concurrency));
    let mut tasks = Vec::with_capacity(total);
    for a in 1..=o.accounts {
        for d in 1..=o.devices {
            let probe = Probe {
                http: http.clone(),
                url: o.url.clone(),
                user: format!("{}{a}@{}", o.prefix, o.domain),
                password: o.password.clone(),
                device: format!("Load{a:04}x{d:02}"),
                policy_key: None,
            };
            let sem = sem.clone();
            tasks.push(tokio::spawn(async move {
                let _permit = sem.acquire_owned().await.map_err(|e| e.to_string())?;
                setup(probe).await
            }));
        }
    }
    let mut phones = Vec::with_capacity(total);
    let mut setup_errors = Vec::new();
    for task in tasks {
        match task.await.map_err(|e| e.to_string())? {
            Ok(p) => phones.push(p),
            Err(e) => setup_errors.push(e),
        }
    }
    println!(
        "setup: {} ready, {} failed in {:.1} s",
        phones.len(),
        setup_errors.len(),
        started.elapsed().as_secs_f32()
    );
    for e in setup_errors.iter().take(5) {
        println!("  setup error: {e}");
    }
    if phones.is_empty() {
        return Err("no phone could be set up".into());
    }

    // Every phone holds a Ping.
    let ping_start = Instant::now();
    let mut pings = Vec::with_capacity(phones.len());
    for ph in &phones {
        let probe = ph.probe.clone();
        let body = ping_body(&ph.inbox, o.heartbeat);
        pings.push(tokio::spawn(async move {
            let r = probe.send("Ping", &body).await;
            (r, Instant::now())
        }));
    }
    // Give the Pings time to reach the gateway and subscribe.
    tokio::time::sleep(Duration::from_secs(5 + total as u64 / 100)).await;
    let early = pings.iter().filter(|p| p.is_finished()).count();
    println!(
        "ping: {} waiting, {early} returned early",
        pings.len() - early
    );

    // One message per account, sent from that account's first phone.
    let send_at = Instant::now();
    let mut sends = Vec::new();
    for ph in phones.iter().filter(|p| p.probe.device.ends_with("x01")) {
        let probe = ph.probe.clone();
        sends.push(tokio::spawn(async move {
            let mime = format!(
                "From: {u}\r\nTo: {u}\r\nSubject: load test\r\nMIME-Version: 1.0\r\nContent-Type: text/plain\r\n\r\nload\r\n",
                u = probe.user
            );
            let body = Element::new(Page::ComposeMail, "SendMail")
                .with_child(t(Page::ComposeMail, "ClientId", &format!("load-{}", probe.device)))
                .with_child(Element::new(Page::ComposeMail, "Mime").with_opaque(mime.into_bytes()));
            match probe.send("SendMail", &body).await {
                Ok(None) => Ok(()),
                Ok(Some(r)) => Err(format!("SendMail: {}", r.to_xml())),
                Err(e) => Err(e),
            }
        }));
    }
    let mut send_errors = 0;
    for s in sends {
        let err = match s.await {
            Ok(Ok(())) => continue,
            Ok(Err(e)) => e,
            Err(e) => e.to_string(),
        };
        send_errors += 1;
        if send_errors <= 3 {
            println!("  send error: {err}");
        }
    }
    println!(
        "send: {} messages in {:.1} s, {send_errors} failed",
        o.accounts,
        send_at.elapsed().as_secs_f32()
    );

    // Collect the Pings.
    let mut latencies = Vec::new();
    let mut wrong = 0;
    let mut failed = Vec::new();
    for p in pings {
        let (r, at) = p.await.map_err(|e| e.to_string())?;
        match r {
            Ok(Some(el)) if el.child_text(Page::Ping, "Status").as_deref() == Some("2") => {
                latencies.push(at.saturating_duration_since(send_at).as_secs_f64());
            }
            Ok(other) => {
                wrong += 1;
                if wrong <= 3 {
                    println!(
                        "  ping ended without changes: {}",
                        other.map(|e| e.to_xml()).unwrap_or_default().trim()
                    );
                }
            }
            Err(e) => failed.push(e),
        }
    }
    latencies.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    println!(
        "wake: {} woken, {wrong} without changes, {} failed, held {:.1} s in total",
        latencies.len(),
        failed.len(),
        ping_start.elapsed().as_secs_f32()
    );
    for e in failed.iter().take(5) {
        println!("  ping error: {e}");
    }
    println!(
        "latency after send: p50 {:.2} s, p90 {:.2} s, p99 {:.2} s, max {:.2} s",
        percentile(&latencies, 0.5),
        percentile(&latencies, 0.9),
        percentile(&latencies, 0.99),
        latencies.last().copied().unwrap_or(0.0)
    );
    if !setup_errors.is_empty() || send_errors > 0 || wrong > 0 || !failed.is_empty() {
        return Err("some phones failed".into());
    }
    Ok(())
}
