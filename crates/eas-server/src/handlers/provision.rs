//! `Provision` (MS-ASPROV): the two round trips that give a device its
//! policy key, and remote wipe.
//!
//! 1. The device asks for the policy; the gateway answers with the policy
//!    document and a temporary key.
//! 2. The device acknowledges with that key; the gateway answers with the
//!    final key, which the device then sends on every request.

use super::{random_u32, wbxml, Ctx, HandlerError, HandlerResult};
use eas_proto::provision::{status, POLICY_TYPE};
use eas_state::WipeStatus;
use eas_wbxml::{Element, Page};

pub async fn handle(ctx: &Ctx<'_>, request: Option<Element>) -> HandlerResult {
    let req = request.ok_or(HandlerError::BadRequest("empty Provision"))?;
    if !req.is(Page::Provision, "Provision") {
        return Err(HandlerError::BadRequest("not a Provision request"));
    }
    let store = &ctx.state.store;
    let device = &ctx.device;
    let mut resp = Element::new(Page::Provision, "Provision");

    if let Some(info) = req.child(Page::Settings, "DeviceInformation") {
        if let Some(set) = info.child(Page::Settings, "Set") {
            store
                .set_device_info(
                    device.id,
                    set.child_text(Page::Settings, "Model").as_deref(),
                    set.child_text(Page::Settings, "FriendlyName").as_deref(),
                    set.child_text(Page::Settings, "OS").as_deref(),
                )
                .await?;
        }
        resp.push(Element::new(Page::Settings, "DeviceInformation").with_text_child("Status", "1"));
    }

    // Wipe acknowledgement: the device reports it wiped itself.
    if let Some(wipe) = req.child(Page::Provision, "RemoteWipe") {
        if device.wipe != WipeStatus::None {
            let st = wipe.child_text(Page::Provision, "Status");
            tracing::warn!(user = %device.username, device = %device.device_id, status = ?st, "device acknowledged remote wipe");
            store
                .set_wipe_status(device.id, WipeStatus::Acknowledged)
                .await?;
        }
        resp.push(Element::new(Page::Provision, "Status").with_text(status::SUCCESS.to_string()));
        return Ok(wbxml(&resp));
    }

    // A pending wipe beats everything else.
    if device.wipe != WipeStatus::None {
        resp.push(Element::new(Page::Provision, "Status").with_text(status::SUCCESS.to_string()));
        resp.push(Element::new(Page::Provision, "RemoteWipe"));
        return Ok(wbxml(&resp));
    }

    let policy = req
        .child(Page::Provision, "Policies")
        .and_then(|p| p.child(Page::Provision, "Policy"))
        .ok_or(HandlerError::BadRequest("Provision without Policy"))?;
    let policy_type = policy
        .child_text(Page::Provision, "PolicyType")
        .unwrap_or_default();
    let mut out =
        Element::new(Page::Provision, "Policy").with_text_child("PolicyType", policy_type.clone());

    if policy_type != POLICY_TYPE {
        resp.push(Element::new(Page::Provision, "Status").with_text(status::SUCCESS.to_string()));
        out = out.with_text_child("Status", status::UNKNOWN_POLICY_TYPE.to_string());
        resp.push(Element::new(Page::Provision, "Policies").with_child(out));
        return Ok(wbxml(&resp));
    }

    let acked_key = policy
        .child_text(Page::Provision, "PolicyKey")
        .and_then(|k| k.trim().parse::<u32>().ok());
    match acked_key {
        // Phase 1: hand out the document and a temporary key.
        None => {
            let temp = random_u32();
            store.set_pending_policy_key(device.id, temp).await?;
            out = out
                .with_text_child("Status", status::SUCCESS.to_string())
                .with_text_child("PolicyKey", temp.to_string())
                .with_child(
                    Element::new(Page::Provision, "Data").with_child(ctx.state.policy.document()),
                );
        }
        // Phase 2: the device acknowledges. Our policy asks nothing the
        // device could fail to apply, so "partially applied" (2) is accepted.
        Some(key) => {
            let applied = policy
                .child_text(Page::Provision, "Status")
                .is_some_and(|s| matches!(s.trim(), "1" | "2"));
            if Some(key) == device.pending_policy_key && applied {
                let final_key = random_u32();
                store.confirm_policy_key(device.id, final_key).await?;
                tracing::info!(user = %device.username, device = %device.device_id, "device provisioned");
                out = out
                    .with_text_child("Status", status::SUCCESS.to_string())
                    .with_text_child("PolicyKey", final_key.to_string());
            } else {
                out = out.with_text_child("Status", status::INVALID_POLICY_KEY.to_string());
            }
        }
    }
    resp.push(Element::new(Page::Provision, "Status").with_text(status::SUCCESS.to_string()));
    resp.push(Element::new(Page::Provision, "Policies").with_child(out));
    Ok(wbxml(&resp))
}
