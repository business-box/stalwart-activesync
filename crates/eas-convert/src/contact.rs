//! JMAP `ContactCard` (JSContact, RFC 9553) to and from EAS contacts
//! (MS-ASCNTC).
//!
//! The card is projected onto EAS's fixed fields ([`project`]), which
//! remembers for each field the card entry it came from. A device change
//! is compared field by field with that projection and only differences
//! become patches ([`device_patch`]), so entries EAS has no field for (a
//! fourth email, a second address of a kind) are never touched.
//!
//! Ghosting: a field the device leaves out of a change is cleared only if
//! the device is known to handle it. With a `Supported` list from the
//! initial Sync that list decides; without one, only a conservative set of
//! fields every client shows ([`CLEARABLE`]) is cleared.

use eas_wbxml::{Element, Page};
use serde_json::{json, Map, Value};
use std::collections::{BTreeMap, HashSet};

/// Where a projected field lives on the card.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Slot {
    /// A name component of this JSContact kind.
    Name(&'static str),
    /// `prop/id/field`; `id` is `None` when the card has no entry yet.
    Entry {
        prop: &'static str,
        id: Option<String>,
        field: &'static str,
    },
    /// `organizations/id/units`.
    Department(Option<String>),
    /// An address of this context (`work`, `private` or none).
    Address {
        id: Option<String>,
        context: Option<&'static str>,
    },
    Keywords,
}

/// EAS field names (Contacts page unless noted) in output order.
const ADDRESS_FIELDS: [(&str, &str); 5] = [
    ("Street", "name"),
    ("City", "locality"),
    ("State", "region"),
    ("PostalCode", "postcode"),
    ("Country", "country"),
];
const ADDRESS_KINDS: [(&str, Option<&str>); 3] = [
    ("Business", Some("work")),
    ("Home", Some("private")),
    ("Other", None),
];

/// Fields cleared when a device leaves them out and sent no `Supported`
/// list: the ones every EAS client shows and edits.
pub const CLEARABLE: &[&str] = &[
    "FirstName",
    "LastName",
    "MiddleName",
    "Title",
    "Suffix",
    "CompanyName",
    "Department",
    "JobTitle",
    "Email1Address",
    "Email2Address",
    "Email3Address",
    "MobilePhoneNumber",
    "BusinessPhoneNumber",
    "Business2PhoneNumber",
    "HomePhoneNumber",
    "Home2PhoneNumber",
    "BusinessFaxNumber",
    "HomeFaxNumber",
    "PagerNumber",
    "BusinessAddressStreet",
    "BusinessAddressCity",
    "BusinessAddressState",
    "BusinessAddressPostalCode",
    "BusinessAddressCountry",
    "HomeAddressStreet",
    "HomeAddressCity",
    "HomeAddressState",
    "HomeAddressPostalCode",
    "HomeAddressCountry",
    "OtherAddressStreet",
    "OtherAddressCity",
    "OtherAddressState",
    "OtherAddressPostalCode",
    "OtherAddressCountry",
    "Birthday",
    "WebPage",
];

/// The card seen through EAS's fields.
#[derive(Debug, Default)]
pub struct Projection {
    pub fields: BTreeMap<String, String>,
    slots: BTreeMap<String, Slot>,
}

fn s<'a>(v: &'a Value, k: &str) -> &'a str {
    v.get(k).and_then(Value::as_str).unwrap_or_default()
}

fn flag(v: &Value, map: &str, key: &str) -> bool {
    v.get(map)
        .and_then(|m| m.get(key))
        .and_then(Value::as_bool)
        .unwrap_or(false)
}

/// Entries of a map property sorted by `pref` (1 first, absent last), then
/// ID, so the projection is stable.
fn entries<'a>(card: &'a Value, prop: &str) -> Vec<(&'a str, &'a Value)> {
    let mut v: Vec<(&str, &Value)> = card
        .get(prop)
        .and_then(Value::as_object)
        .map(|m| m.iter().map(|(k, v)| (k.as_str(), v)).collect())
        .unwrap_or_default();
    v.sort_by_key(|(id, e)| {
        (
            e.get("pref").and_then(Value::as_u64).unwrap_or(u64::MAX),
            *id,
        )
    });
    v
}

fn date_to_eas(d: &Value) -> Option<String> {
    match s(d, "@type") {
        "Timestamp" => crate::email::eas_date(s(d, "utc")),
        _ => {
            let day = d.get("day")?.as_u64()?;
            let month = d.get("month")?.as_u64()?;
            // Without a year, use 1604, the convention iOS and others read
            // as "year unknown".
            let year = d.get("year").and_then(Value::as_u64).unwrap_or(1604);
            Some(format!("{year:04}-{month:02}-{day:02}T00:00:00.000Z"))
        }
    }
}

/// EAS birthday to a JSContact partial date. Devices send local midnight
/// converted to UTC, so a time in the afternoon belongs to the next day.
fn eas_to_date(eas: &str) -> Option<Value> {
    let dt = chrono::DateTime::parse_from_rfc3339(eas.trim())
        .ok()?
        .with_timezone(&chrono::Utc);
    use chrono::{Datelike, Timelike};
    let date = if dt.hour() >= 12 {
        dt.date_naive().succ_opt()?
    } else {
        dt.date_naive()
    };
    let mut v = json!({"@type": "PartialDate", "month": date.month(), "day": date.day()});
    if date.year() != 1604 {
        v["year"] = json!(date.year());
    }
    Some(v)
}

fn address_fields(a: &Value) -> [String; 5] {
    let comps: Vec<&Value> = a
        .get("components")
        .and_then(Value::as_array)
        .map(|c| c.iter().collect())
        .unwrap_or_default();
    let get = |kinds: &[&str]| {
        comps
            .iter()
            .filter(|c| kinds.contains(&s(c, "kind")))
            .map(|c| s(c, "value").trim())
            .filter(|v| !v.is_empty())
            .collect::<Vec<_>>()
            .join(" ")
    };
    let mut street = get(&[
        "postOfficeBox",
        "name",
        "number",
        "building",
        "block",
        "apartment",
        "floor",
        "room",
        "direction",
        "landmark",
    ]);
    if street.is_empty() && comps.is_empty() {
        street = s(a, "full").to_string();
    }
    [
        street,
        get(&["locality"]),
        get(&["region"]),
        get(&["postcode"]),
        get(&["country"]),
    ]
}

/// Projects a card onto EAS fields.
pub fn project(card: &Value) -> Projection {
    let mut p = Projection::default();
    let put = |p: &mut Projection, field: &str, value: String, slot: Slot| {
        if !value.is_empty() {
            p.fields.insert(field.to_string(), value);
        }
        p.slots.insert(field.to_string(), slot);
    };

    // Name.
    let comps: Vec<&Value> = card
        .get("name")
        .and_then(|n| n.get("components"))
        .and_then(Value::as_array)
        .map(|c| c.iter().collect())
        .unwrap_or_default();
    for (field, kind) in [
        ("FirstName", "given"),
        ("LastName", "surname"),
        ("MiddleName", "given2"),
        ("Title", "title"),
        ("Suffix", "credential"),
    ] {
        let v = comps
            .iter()
            .filter(|c| s(c, "kind") == kind)
            .map(|c| s(c, "value"))
            .collect::<Vec<_>>()
            .join(" ");
        put(&mut p, field, v, Slot::Name(kind));
    }

    // Emails 1 to 3.
    let emails = entries(card, "emails");
    for (i, field) in ["Email1Address", "Email2Address", "Email3Address"]
        .iter()
        .enumerate()
    {
        let e = emails.get(i);
        put(
            &mut p,
            field,
            e.map(|(_, e)| s(e, "address").to_string())
                .unwrap_or_default(),
            Slot::Entry {
                prop: "emails",
                id: e.map(|(id, _)| id.to_string()),
                field: "address",
            },
        );
    }

    // Phones, each to the first free field that fits.
    let mut phone_slots: BTreeMap<&str, (String, String)> = BTreeMap::new();
    for (id, ph) in entries(card, "phones") {
        let fax = flag(ph, "features", "fax");
        let work = flag(ph, "contexts", "work");
        let private = flag(ph, "contexts", "private");
        let candidates: &[&str] = if fax {
            if work {
                &["BusinessFaxNumber"]
            } else {
                &["HomeFaxNumber"]
            }
        } else if flag(ph, "features", "pager") {
            &["PagerNumber"]
        } else if flag(ph, "features", "mobile") || flag(ph, "features", "cell") {
            &["MobilePhoneNumber"]
        } else if work {
            &["BusinessPhoneNumber", "Business2PhoneNumber"]
        } else if private {
            &["HomePhoneNumber", "Home2PhoneNumber"]
        } else {
            &["MobilePhoneNumber", "HomePhoneNumber", "Home2PhoneNumber"]
        };
        if let Some(f) = candidates.iter().find(|f| !phone_slots.contains_key(*f)) {
            phone_slots.insert(f, (id.to_string(), s(ph, "number").to_string()));
        }
    }
    for field in PHONE_FIELDS {
        let e = phone_slots.get(field);
        put(
            &mut p,
            field,
            e.map(|(_, n)| n.clone()).unwrap_or_default(),
            Slot::Entry {
                prop: "phones",
                id: e.map(|(id, _)| id.clone()),
                field: "number",
            },
        );
    }

    // Organisation and job title.
    let org = entries(card, "organizations").into_iter().next();
    put(
        &mut p,
        "CompanyName",
        org.map(|(_, o)| s(o, "name").to_string())
            .unwrap_or_default(),
        Slot::Entry {
            prop: "organizations",
            id: org.map(|(id, _)| id.to_string()),
            field: "name",
        },
    );
    let dept = org
        .and_then(|(_, o)| o.get("units"))
        .and_then(Value::as_array)
        .and_then(|u| u.first())
        .map(|u| s(u, "name").to_string())
        .unwrap_or_default();
    put(
        &mut p,
        "Department",
        dept,
        Slot::Department(org.map(|(id, _)| id.to_string())),
    );
    let title = entries(card, "titles")
        .into_iter()
        .find(|(_, t)| matches!(s(t, "kind"), "" | "title"));
    put(
        &mut p,
        "JobTitle",
        title
            .map(|(_, t)| s(t, "name").to_string())
            .unwrap_or_default(),
        Slot::Entry {
            prop: "titles",
            id: title.map(|(id, _)| id.to_string()),
            field: "name",
        },
    );

    // Addresses: first work, first private, first without either.
    let addrs = entries(card, "addresses");
    for (prefix, ctx) in ADDRESS_KINDS {
        let a = addrs.iter().find(|(_, a)| match ctx {
            Some(c) => flag(a, "contexts", c),
            None => !flag(a, "contexts", "work") && !flag(a, "contexts", "private"),
        });
        let values = a.map(|(_, a)| address_fields(a)).unwrap_or_default();
        for ((suffix, _), v) in ADDRESS_FIELDS.iter().zip(values) {
            put(
                &mut p,
                &format!("{prefix}Address{suffix}"),
                v,
                Slot::Address {
                    id: a.map(|(id, _)| id.to_string()),
                    context: ctx,
                },
            );
        }
    }

    // Dates.
    let annivs = entries(card, "anniversaries");
    for (field, kind) in [("Birthday", "birth"), ("Anniversary", "wedding")] {
        let a = annivs.iter().find(|(_, a)| s(a, "kind") == kind);
        put(
            &mut p,
            field,
            a.and_then(|(_, a)| a.get("date"))
                .and_then(date_to_eas)
                .unwrap_or_default(),
            Slot::Entry {
                prop: "anniversaries",
                id: a.map(|(id, _)| id.to_string()),
                field: "date",
            },
        );
    }

    // Web page, nickname, notes.
    for (field, prop, inner) in [
        ("WebPage", "links", "uri"),
        ("NickName", "nicknames", "name"),
        ("Body", "notes", "note"),
    ] {
        let e = entries(card, prop).into_iter().next();
        put(
            &mut p,
            field,
            e.map(|(_, e)| s(e, inner).to_string()).unwrap_or_default(),
            Slot::Entry {
                prop,
                id: e.map(|(id, _)| id.to_string()),
                field: inner,
            },
        );
    }

    // Categories.
    let mut cats: Vec<&str> = card
        .get("keywords")
        .and_then(Value::as_object)
        .map(|m| {
            m.iter()
                .filter(|(_, v)| v.as_bool() == Some(true))
                .map(|(k, _)| k.as_str())
                .collect()
        })
        .unwrap_or_default();
    cats.sort();
    put(&mut p, "Categories", cats.join("\u{1f}"), Slot::Keywords);
    p
}

const PHONE_FIELDS: [&str; 9] = [
    "MobilePhoneNumber",
    "BusinessPhoneNumber",
    "Business2PhoneNumber",
    "HomePhoneNumber",
    "Home2PhoneNumber",
    "BusinessFaxNumber",
    "HomeFaxNumber",
    "PagerNumber",
    "CarPhoneNumber",
];

/// Fields on the Contacts2 page rather than Contacts.
fn page_of(field: &str) -> Page {
    match field {
        "NickName" => Page::Contacts2,
        _ => Page::Contacts,
    }
}

fn file_as(f: &BTreeMap<String, String>, card: &Value) -> String {
    let first = f.get("FirstName").map(String::as_str).unwrap_or_default();
    let last = f.get("LastName").map(String::as_str).unwrap_or_default();
    match (first.is_empty(), last.is_empty()) {
        (false, false) => format!("{last}, {first}"),
        (true, false) => last.to_string(),
        (false, true) => first.to_string(),
        (true, true) => {
            let full = card
                .get("name")
                .and_then(|n| n.get("full"))
                .and_then(Value::as_str)
                .unwrap_or_default();
            if full.is_empty() {
                f.get("CompanyName")
                    .or_else(|| f.get("Email1Address"))
                    .cloned()
                    .unwrap_or_default()
            } else {
                full.to_string()
            }
        }
    }
}

/// `ApplicationData` for a card.
pub fn application_data(card: &Value) -> Element {
    let p = project(card);
    let mut d = Element::new(Page::AirSync, "ApplicationData");
    for (field, value) in &p.fields {
        match field.as_str() {
            "Body" | "Categories" => continue,
            _ => d.push(Element::new(page_of(field), field).with_text(value.clone())),
        }
    }
    // A card with only a full name still needs a name on the device.
    if !p.fields.contains_key("FirstName") && !p.fields.contains_key("LastName") {
        let full = card
            .get("name")
            .and_then(|n| n.get("full"))
            .and_then(Value::as_str)
            .unwrap_or_default();
        if !full.is_empty() {
            d.push(Element::new(Page::Contacts, "LastName").with_text(full));
        }
    }
    d.push(Element::new(Page::Contacts, "FileAs").with_text(file_as(&p.fields, card)));
    if let Some(note) = p.fields.get("Body") {
        d.push(
            Element::new(Page::AirSyncBase, "Body")
                .with_text_child("Type", "1")
                .with_text_child("EstimatedDataSize", note.len().to_string())
                .with_text_child("Data", note.clone()),
        );
    }
    if let Some(cats) = p.fields.get("Categories") {
        let mut c = Element::new(Page::Contacts, "Categories");
        for cat in cats.split('\u{1f}') {
            c.push(Element::new(Page::Contacts, "Category").with_text(cat));
        }
        d.push(c);
    }
    d
}

/// The fields a device sent, by EAS name.
fn device_fields(data: &Element) -> BTreeMap<String, String> {
    let mut out = BTreeMap::new();
    for el in data.elements() {
        let name = el.name();
        match (el.page(), name) {
            (Page::AirSyncBase, "Body") => {
                if el.child_text(Page::AirSyncBase, "Type").as_deref() == Some("1") {
                    if let Some(d) = el.child_text(Page::AirSyncBase, "Data") {
                        out.insert("Body".into(), d);
                    }
                }
            }
            (Page::Contacts, "Categories") => {
                let mut cats: Vec<String> = el
                    .children_named(Page::Contacts, "Category")
                    .filter_map(Element::text)
                    .collect();
                cats.sort();
                out.insert("Categories".into(), cats.join("\u{1f}"));
            }
            (Page::Contacts, _) | (Page::Contacts2, _) => {
                let v = el.text().unwrap_or_default();
                let v = if name.starts_with("Email") {
                    bare_address(&v)
                } else {
                    v.trim().to_string()
                };
                out.insert(name.to_string(), v);
            }
            _ => {}
        }
    }
    out
}

/// `"Name" <a@b>` to `a@b`; devices send either form.
fn bare_address(v: &str) -> String {
    match (v.rfind('<'), v.rfind('>')) {
        (Some(a), Some(b)) if a < b => v[a + 1..b].trim().to_string(),
        _ => v.trim().to_string(),
    }
}

fn new_phone(field: &str, number: &str) -> Value {
    let (ctx, feature) = match field {
        "MobilePhoneNumber" => (None, "mobile"),
        "BusinessPhoneNumber" | "Business2PhoneNumber" => (Some("work"), "voice"),
        "HomePhoneNumber" | "Home2PhoneNumber" => (Some("private"), "voice"),
        "BusinessFaxNumber" => (Some("work"), "fax"),
        "HomeFaxNumber" => (Some("private"), "fax"),
        "PagerNumber" => (None, "pager"),
        _ => (None, "voice"),
    };
    let mut v = json!({"@type": "Phone", "number": number, "features": {feature: true}});
    if let Some(c) = ctx {
        v["contexts"] = json!({ c: true });
    }
    v
}

fn new_entry(prop: &str, field: &str, eas_field: &str, value: &str) -> Option<Value> {
    Some(match prop {
        "emails" => json!({"@type": "EmailAddress", "address": value}),
        "phones" => new_phone(eas_field, value),
        "organizations" => json!({"@type": "Organization", "name": value}),
        "titles" => json!({"@type": "Title", "name": value, "kind": "title"}),
        "anniversaries" => json!({
            "@type": "Anniversary",
            "kind": if eas_field == "Birthday" { "birth" } else { "wedding" },
            "date": eas_to_date(value)?,
        }),
        "links" => json!({"@type": "Link", "uri": value}),
        "nicknames" => json!({"@type": "Nickname", "name": value}),
        "notes" => json!({"@type": "Note", "note": value}),
        _ => json!({ field: value }),
    })
}

/// JMAP patch turning `card` into what the device sent. `supported` is the
/// device's `Supported` list for the folder, if it sent one. On an empty
/// card every key is a top-level property, so the result doubles as the
/// body of a new card.
pub fn device_patch(
    card: &Value,
    data: &Element,
    supported: Option<&HashSet<String>>,
) -> Map<String, Value> {
    let p = project(card);
    let dev = device_fields(data);
    let clearable = |f: &str| match supported {
        Some(s) => s.contains(f),
        None => CLEARABLE.contains(&f),
    };
    let mut patch = Map::new();
    // New entries per property; properties the card lacks are set whole.
    let mut created: BTreeMap<&'static str, Map<String, Value>> = BTreeMap::new();
    let mut name_changed = false;
    let mut addresses_changed: BTreeMap<String, (Option<String>, Option<&'static str>)> =
        BTreeMap::new();

    for (field, slot) in &p.slots {
        let cur = p.fields.get(field).map(String::as_str).unwrap_or_default();
        let new = match dev.get(field) {
            Some(v) => v.as_str(),
            None if clearable(field) => "",
            None => continue,
        };
        if new == cur {
            continue;
        }
        match slot {
            Slot::Name(_) => name_changed = true,
            Slot::Address { id, context } => {
                let prefix = field
                    .split("Address")
                    .next()
                    .unwrap_or_default()
                    .to_string();
                addresses_changed.insert(prefix, (id.clone(), *context));
            }
            Slot::Keywords => {
                let kw: Map<String, Value> = new
                    .split('\u{1f}')
                    .filter(|c| !c.is_empty())
                    .map(|c| (c.to_string(), json!(true)))
                    .collect();
                patch.insert("keywords".into(), Value::Object(kw));
            }
            Slot::Department(org) => {
                let units = if new.is_empty() {
                    json!([])
                } else {
                    json!([{"@type": "OrgUnit", "name": new}])
                };
                match org {
                    Some(id) => {
                        patch.insert(format!("organizations/{id}/units"), units);
                    }
                    None if !new.is_empty() => {
                        let org = created
                            .entry("organizations")
                            .or_default()
                            .entry("eas-org".to_string())
                            .or_insert_with(|| json!({"@type": "Organization", "name": ""}));
                        org["units"] = units;
                    }
                    None => {}
                }
            }
            Slot::Entry {
                prop,
                id,
                field: inner,
            } => match (id, new.is_empty()) {
                (Some(id), true) => {
                    patch.insert(format!("{prop}/{id}"), Value::Null);
                }
                (Some(id), false) => {
                    let v = if *prop == "anniversaries" {
                        match eas_to_date(new) {
                            Some(d) => d,
                            None => continue,
                        }
                    } else {
                        json!(new)
                    };
                    patch.insert(format!("{prop}/{id}/{inner}"), v);
                }
                (None, true) => {}
                (None, false) => {
                    let Some(v) = new_entry(prop, inner, field, new) else {
                        continue;
                    };
                    let key = format!("eas-{}", field.to_ascii_lowercase());
                    let m = created.entry(prop).or_default();
                    // CompanyName and Department share the new organisation.
                    if *prop == "organizations" {
                        let org = m
                            .entry("eas-org".to_string())
                            .or_insert_with(|| json!({"@type": "Organization"}));
                        org["name"] = json!(new);
                    } else {
                        m.insert(key, v);
                    }
                }
            },
        }
    }

    if name_changed {
        let get = |f: &str| {
            dev.get(f)
                .cloned()
                .or_else(|| (!clearable(f)).then(|| p.fields.get(f).cloned()).flatten())
                .unwrap_or_default()
        };
        let mut comps: Vec<Value> = card
            .get("name")
            .and_then(|n| n.get("components"))
            .and_then(Value::as_array)
            .map(|c| {
                c.iter()
                    .filter(|c| {
                        !matches!(
                            s(c, "kind"),
                            "given" | "surname" | "given2" | "title" | "credential"
                        )
                    })
                    .cloned()
                    .collect()
            })
            .unwrap_or_default();
        let mut ordered = Vec::new();
        for (f, kind) in [
            ("Title", "title"),
            ("FirstName", "given"),
            ("MiddleName", "given2"),
            ("LastName", "surname"),
            ("Suffix", "credential"),
        ] {
            let v = get(f);
            if !v.is_empty() {
                ordered.push(json!({"@type": "NameComponent", "kind": kind, "value": v}));
            }
        }
        let full = ordered
            .iter()
            .map(|c| s(c, "value"))
            .collect::<Vec<_>>()
            .join(" ");
        ordered.append(&mut comps);
        let mut name = card
            .get("name")
            .and_then(Value::as_object)
            .cloned()
            .unwrap_or_default();
        name.insert("@type".into(), json!("Name"));
        name.insert("components".into(), Value::Array(ordered));
        name.insert("isOrdered".into(), json!(true));
        name.insert("defaultSeparator".into(), json!(" "));
        if full.is_empty() {
            name.remove("full");
        } else {
            name.insert("full".into(), json!(full));
        }
        patch.insert("name".into(), Value::Object(name));
    }

    for (prefix, (id, ctx)) in addresses_changed {
        let mut comps = Vec::new();
        for (suffix, kind) in ADDRESS_FIELDS {
            let f = format!("{prefix}Address{suffix}");
            let v = match dev.get(&f) {
                Some(v) => v.clone(),
                None if clearable(&f) => String::new(),
                None => p.fields.get(&f).cloned().unwrap_or_default(),
            };
            if !v.is_empty() {
                comps.push(json!({"@type": "AddressComponent", "kind": kind, "value": v}));
            }
        }
        let value = if comps.is_empty() {
            Value::Null
        } else {
            let mut a = json!({"@type": "Address", "components": comps});
            if let Some(c) = ctx {
                a["contexts"] = json!({ c: true });
            }
            a
        };
        match id {
            Some(id) => {
                patch.insert(format!("addresses/{id}"), value);
            }
            None if !value.is_null() => {
                created
                    .entry("addresses")
                    .or_default()
                    .insert(format!("eas-{}", prefix.to_ascii_lowercase()), value);
            }
            None => {}
        }
    }

    for (prop, entries) in created {
        if card.get(prop).is_some_and(Value::is_object) {
            for (id, v) in entries {
                patch.insert(format!("{prop}/{id}"), v);
            }
        } else {
            patch.insert(prop.to_string(), Value::Object(entries));
        }
    }
    patch
}

/// A new card from a device `Add`.
pub fn new_card(data: &Element, address_book: &str) -> Value {
    let mut card = device_patch(&json!({}), data, None);
    card.insert("@type".into(), json!("Card"));
    card.insert("version".into(), json!("1.0"));
    card.insert("addressBookIds".into(), json!({ address_book: true }));
    Value::Object(card)
}

/// Stable fingerprint of a card (FNV-1a over its canonical JSON), to spot
/// server-side changes.
pub fn hash(card: &Value) -> String {
    let text = card.to_string();
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in text.bytes() {
        h ^= b as u64;
        h = h.wrapping_mul(0x0000_0100_0000_01b3);
    }
    format!("{h:016x}")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn card() -> Value {
        json!({
            "@type": "Card",
            "addressBookIds": {"ab1": true},
            "name": {"components": [
                {"kind": "given", "value": "Ada"},
                {"kind": "surname", "value": "Lovelace"},
                {"kind": "generation", "value": "II"}
            ]},
            "emails": {
                "e2": {"address": "ada@work.example", "contexts": {"work": true}},
                "e1": {"address": "ada@home.example", "pref": 1},
                "e3": {"address": "a3@x"},
                "e4": {"address": "a4@x"}
            },
            "phones": {
                "p1": {"number": "+1 555 0100", "features": {"mobile": true}},
                "p2": {"number": "+1 555 0200", "contexts": {"work": true}, "features": {"voice": true}}
            },
            "organizations": {"o1": {"name": "Analytical Engines", "units": [{"name": "R&D"}]}},
            "addresses": {"a1": {"contexts": {"private": true}, "components": [
                {"kind": "number", "value": "12"}, {"kind": "name", "value": "St James's Square"},
                {"kind": "locality", "value": "London"}, {"kind": "country", "value": "UK"}
            ]}},
            "anniversaries": {"b": {"kind": "birth", "date": {"@type": "PartialDate", "year": 1815, "month": 12, "day": 10}}},
            "notes": {"n1": {"note": "Notes on the engine"}},
            "keywords": {"vip": true}
        })
    }

    fn t(page: Page, name: &str, v: &str) -> Element {
        Element::new(page, name).with_text(v)
    }

    #[test]
    fn projects_fields() {
        let p = project(&card());
        let f = |k: &str| p.fields.get(k).map(String::as_str);
        assert_eq!(f("FirstName"), Some("Ada"));
        assert_eq!(f("Email1Address"), Some("ada@home.example"));
        assert_eq!(f("Email2Address"), Some("ada@work.example"));
        assert_eq!(f("Email3Address"), Some("a3@x"));
        assert_eq!(f("MobilePhoneNumber"), Some("+1 555 0100"));
        assert_eq!(f("BusinessPhoneNumber"), Some("+1 555 0200"));
        assert_eq!(f("CompanyName"), Some("Analytical Engines"));
        assert_eq!(f("Department"), Some("R&D"));
        assert_eq!(f("HomeAddressStreet"), Some("12 St James's Square"));
        assert_eq!(f("HomeAddressCity"), Some("London"));
        assert_eq!(f("Birthday"), Some("1815-12-10T00:00:00.000Z"));
        let d = application_data(&card());
        assert_eq!(
            d.child_text(Page::Contacts, "FileAs").as_deref(),
            Some("Lovelace, Ada")
        );
        assert!(d.child(Page::AirSyncBase, "Body").is_some());
        let bytes = eas_wbxml::encode(&d);
        assert_eq!(eas_wbxml::decode(&bytes).unwrap(), d);
    }

    /// What the device would send back unchanged: our own output.
    fn echo(card: &Value) -> Element {
        application_data(card)
    }

    #[test]
    fn unchanged_echo_patches_nothing() {
        let c = card();
        assert!(device_patch(&c, &echo(&c), None).is_empty());
    }

    #[test]
    fn edits_patch_only_what_changed() {
        let c = card();
        let mut data = echo(&c);
        // Change the mobile number, drop email 2, add a home phone.
        data.children.retain(|n| {
            !matches!(n, eas_wbxml::Node::Element(e) if e.name() == "MobilePhoneNumber" || e.name() == "Email2Address")
        });
        data.push(t(Page::Contacts, "MobilePhoneNumber", "+1 555 0199"));
        data.push(t(Page::Contacts, "HomePhoneNumber", "+44 20 7946 0000"));
        let p = device_patch(&c, &data, None);
        assert_eq!(p["phones/p1/number"], json!("+1 555 0199"));
        assert_eq!(p["emails/e2"], Value::Null);
        assert_eq!(
            p["phones/eas-homephonenumber"]["contexts"],
            json!({"private": true})
        );
        // Email 4, which EAS can't show, and the name are untouched.
        assert!(!p.keys().any(|k| k.starts_with("emails/e4") || k == "name"));
        assert_eq!(p.len(), 3, "{p:?}");
    }

    #[test]
    fn ghosted_fields_survive() {
        let c = card();
        // A device that sends only the name: everything it doesn't know
        // stays; fields every client shows are cleared.
        let data = Element::new(Page::AirSync, "ApplicationData")
            .with_child(t(Page::Contacts, "FirstName", "Ada"))
            .with_child(t(Page::Contacts, "LastName", "Lovelace"));
        let p = device_patch(&c, &data, None);
        assert!(!p.contains_key("notes/n1"), "notes are ghost-safe");
        assert!(!p.contains_key("keywords"));
        assert_eq!(p["emails/e1"], Value::Null);

        // With a Supported list naming only the name fields, nothing else
        // is cleared.
        let supported: HashSet<String> = ["FirstName", "LastName"]
            .iter()
            .map(|s| s.to_string())
            .collect();
        assert!(device_patch(&c, &data, Some(&supported)).is_empty());
    }

    #[test]
    fn name_change_keeps_other_components() {
        let c = card();
        let mut data = echo(&c);
        data.children
            .retain(|n| !matches!(n, eas_wbxml::Node::Element(e) if e.name() == "FirstName"));
        data.push(t(Page::Contacts, "FirstName", "Augusta Ada"));
        let p = device_patch(&c, &data, None);
        let comps = p["name"]["components"].as_array().unwrap();
        assert!(comps.iter().any(|c| c["kind"] == "generation"));
        assert_eq!(p["name"]["full"], json!("Augusta Ada Lovelace"));
    }

    #[test]
    fn new_card_from_device() {
        let data = Element::new(Page::AirSync, "ApplicationData")
            .with_child(t(Page::Contacts, "FirstName", "Grace"))
            .with_child(t(Page::Contacts, "LastName", "Hopper"))
            .with_child(t(
                Page::Contacts,
                "Email1Address",
                "\"Grace\" <grace@navy.example>",
            ))
            .with_child(t(Page::Contacts, "CompanyName", "US Navy"))
            .with_child(t(Page::Contacts, "Department", "Computing"))
            .with_child(t(Page::Contacts, "Birthday", "1906-12-08T23:00:00.000Z"))
            .with_child(t(Page::Contacts, "BusinessAddressCity", "Arlington"));
        let c = new_card(&data, "ab1");
        assert_eq!(c["addressBookIds"], json!({"ab1": true}));
        assert_eq!(
            c["emails"]["eas-email1address"]["address"],
            json!("grace@navy.example")
        );
        assert_eq!(c["organizations"]["eas-org"]["name"], json!("US Navy"));
        assert_eq!(
            c["organizations"]["eas-org"]["units"][0]["name"],
            json!("Computing")
        );
        // 23:00 UTC is local midnight of the next day.
        assert_eq!(c["anniversaries"]["eas-birthday"]["date"]["day"], json!(9));
        assert_eq!(
            c["addresses"]["eas-business"]["contexts"],
            json!({"work": true})
        );
        // And it projects back to what the device sent.
        let back = project(&c);
        assert_eq!(
            back.fields.get("Department").map(String::as_str),
            Some("Computing")
        );
        assert_eq!(
            back.fields.get("BusinessAddressCity").map(String::as_str),
            Some("Arlington")
        );
    }

    #[test]
    fn hash_is_stable_and_sensitive() {
        let a = card();
        let mut b = card();
        assert_eq!(hash(&a), hash(&b));
        b["notes"]["n1"]["note"] = json!("changed");
        assert_ne!(hash(&a), hash(&b));
    }
}
