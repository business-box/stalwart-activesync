use crate::command::Command;
use crate::version::ProtocolVersion;
use base64::Engine as _;

/// What an EAS request carries in its URI.
///
/// Clients use one of two forms (MS-ASHTTP 2.2.1.1.1):
/// - plain: `?Cmd=Sync&User=alice&DeviceId=ABC123&DeviceType=iPhone`
/// - base64: the whole query string is one base64 blob with a binary
///   layout; the protocol version and policy key then travel inside it
///   instead of in headers.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct RequestLine {
    pub command: Option<Command>,
    pub user: Option<String>,
    pub device_id: String,
    pub device_type: String,
    /// Only set by the base64 form; otherwise read `X-MS-PolicyKey`.
    pub policy_key: Option<u32>,
    /// Only set by the base64 form; otherwise read `MS-ASProtocolVersion`.
    pub protocol_version: Option<ProtocolVersion>,
    pub locale: Option<u16>,
    pub attachment_name: Option<String>,
    pub collection_id: Option<String>,
    pub item_id: Option<String>,
    pub long_id: Option<String>,
    pub occurrence: Option<String>,
    pub save_in_sent: bool,
    pub accept_multipart: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum QueryError {
    #[error("missing Cmd parameter")]
    MissingCommand,
    #[error("unknown command {0:?}")]
    UnknownCommand(String),
    #[error("missing DeviceId")]
    MissingDeviceId,
    #[error("missing DeviceType")]
    MissingDeviceType,
    #[error("invalid {0}")]
    Invalid(&'static str),
    #[error("base64 request line is not valid base64")]
    Base64,
    #[error("base64 request line truncated")]
    Truncated,
}

/// Device IDs and types end up in database keys and log lines, so only a
/// conservative character set is accepted.
fn clean_id(s: &str, what: &'static str, max: usize) -> Result<String, QueryError> {
    if s.is_empty() || s.len() > max {
        return Err(QueryError::Invalid(what));
    }
    if !s
        .bytes()
        .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.'))
    {
        return Err(QueryError::Invalid(what));
    }
    Ok(s.to_string())
}

impl RequestLine {
    /// Parses the query string of an EAS request URI (without the `?`).
    /// The command is optional only for `OPTIONS`; POST handlers should call
    /// [`RequestLine::command`].
    pub fn parse(query: &str) -> Result<RequestLine, QueryError> {
        // Base64 never contains '&', and a plain line always has several parameters.
        if query.contains('&') || query.starts_with("Cmd=") {
            RequestLine::parse_plain(query)
        } else {
            RequestLine::parse_base64(query)
        }
    }

    /// The command, or an error if the request line has none.
    pub fn command(&self) -> Result<Command, QueryError> {
        self.command.ok_or(QueryError::MissingCommand)
    }

    fn parse_plain(query: &str) -> Result<RequestLine, QueryError> {
        let mut line = RequestLine::default();
        let mut device_id = None;
        let mut device_type = None;
        for (k, v) in form_urlencoded::parse(query.as_bytes()) {
            match k.as_ref() {
                "Cmd" => {
                    line.command = Some(
                        Command::from_name(&v)
                            .ok_or_else(|| QueryError::UnknownCommand(v.to_string()))?,
                    )
                }
                "User" => line.user = Some(v.into_owned()),
                "DeviceId" => device_id = Some(clean_id(&v, "DeviceId", 64)?),
                "DeviceType" => device_type = Some(clean_id(&v, "DeviceType", 64)?),
                "AttachmentName" => line.attachment_name = Some(v.into_owned()),
                "CollectionId" => line.collection_id = Some(v.into_owned()),
                "ItemId" => line.item_id = Some(v.into_owned()),
                "LongId" => line.long_id = Some(v.into_owned()),
                "Occurrence" => line.occurrence = Some(v.into_owned()),
                "SaveInSent" => line.save_in_sent = v.eq_ignore_ascii_case("T"),
                _ => {}
            }
        }
        line.device_id = device_id.ok_or(QueryError::MissingDeviceId)?;
        line.device_type = device_type.ok_or(QueryError::MissingDeviceType)?;
        Ok(line)
    }

    fn parse_base64(query: &str) -> Result<RequestLine, QueryError> {
        let raw = query.trim();
        let raw = percent_decode(raw);
        let bytes = base64::engine::general_purpose::STANDARD
            .decode(raw.as_bytes())
            .or_else(|_| base64::engine::general_purpose::STANDARD_NO_PAD.decode(raw.as_bytes()))
            .or_else(|_| base64::engine::general_purpose::URL_SAFE_NO_PAD.decode(raw.as_bytes()))
            .map_err(|_| QueryError::Base64)?;
        let mut r = Cursor { b: &bytes, pos: 0 };

        let version = r.u8()?;
        let cmd = r.u8()?;
        let locale = u16::from_le_bytes([r.u8()?, r.u8()?]);
        let dev_id_len = r.u8()? as usize;
        let dev_id = r.take(dev_id_len)?;
        let pol_len = r.u8()? as usize;
        let policy_key = match pol_len {
            0 => None,
            4 => {
                let k = r.take(4)?;
                Some(u32::from_le_bytes([k[0], k[1], k[2], k[3]]))
            }
            _ => return Err(QueryError::Invalid("policy key length")),
        };
        let type_len = r.u8()? as usize;
        let dev_type = r.take(type_len)?;

        let mut line = RequestLine {
            command: Some(
                Command::from_code(cmd).ok_or(QueryError::UnknownCommand(cmd.to_string()))?,
            ),
            protocol_version: ProtocolVersion::from_code(version),
            locale: Some(locale),
            policy_key,
            // The binary device ID is sent raw; hex keeps it a stable,
            // printable key (the same convention Z-Push uses).
            device_id: if dev_id.is_empty() {
                return Err(QueryError::MissingDeviceId);
            } else {
                dev_id.iter().map(|b| format!("{b:02x}")).collect()
            },
            device_type: clean_id(
                std::str::from_utf8(dev_type).map_err(|_| QueryError::Invalid("DeviceType"))?,
                "DeviceType",
                64,
            )
            .map_err(|e| match e {
                QueryError::Invalid(_) if dev_type.is_empty() => QueryError::MissingDeviceType,
                e => e,
            })?,
            ..RequestLine::default()
        };

        while r.pos < r.b.len() {
            let tag = r.u8()?;
            let len = r.u8()? as usize;
            let val = r.take(len)?;
            let text = || String::from_utf8_lossy(val).into_owned();
            match tag {
                0 => line.attachment_name = Some(text()),
                1 => line.collection_id = Some(text()),
                3 => line.item_id = Some(text()),
                4 => line.long_id = Some(text()),
                6 => line.occurrence = Some(text()),
                7 => {
                    let flags = val.first().copied().unwrap_or(0);
                    line.save_in_sent = flags & 0x01 != 0;
                    line.accept_multipart = flags & 0x02 != 0;
                }
                8 => line.user = Some(text()),
                _ => {}
            }
        }
        Ok(line)
    }
}

fn percent_decode(s: &str) -> String {
    if !s.contains('%') {
        return s.to_string();
    }
    // `form_urlencoded` would also turn '+' into a space, which corrupts
    // base64, so decode %XX by hand.
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        // Hex digits are read as bytes: slicing the string here could cut a
        // multi-byte character in half.
        if b[i] == b'%' && i + 2 < b.len() {
            let hex = |c: u8| (c as char).to_digit(16);
            if let (Some(h), Some(l)) = (hex(b[i + 1]), hex(b[i + 2])) {
                out.push((h * 16 + l) as u8);
                i += 3;
                continue;
            }
        }
        out.push(b[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

struct Cursor<'a> {
    b: &'a [u8],
    pos: usize,
}

impl<'a> Cursor<'a> {
    fn u8(&mut self) -> Result<u8, QueryError> {
        let v = *self.b.get(self.pos).ok_or(QueryError::Truncated)?;
        self.pos += 1;
        Ok(v)
    }

    fn take(&mut self, n: usize) -> Result<&'a [u8], QueryError> {
        let end = self.pos.checked_add(n).ok_or(QueryError::Truncated)?;
        let s = self.b.get(self.pos..end).ok_or(QueryError::Truncated)?;
        self.pos = end;
        Ok(s)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn percent_followed_by_multibyte_text_does_not_panic() {
        // Found by the request_line fuzz target.
        assert_eq!(percent_decode("a%\u{e9}b"), "a%\u{e9}b");
        assert_eq!(percent_decode("%\u{1F600}"), "%\u{1F600}");
        assert_eq!(percent_decode("x%41%4"), "xA%4");
        assert!(RequestLine::parse("Cmd=Sync&DeviceId=%\u{e9}&DeviceType=x").is_err());
    }

    #[test]
    fn plain_form() {
        let l = RequestLine::parse(
            "Cmd=SmartReply&User=alice%40example.org&DeviceId=ApplF17XYZ&DeviceType=iPhone&ItemId=m%3A42&SaveInSent=T",
        )
        .unwrap();
        assert_eq!(l.command, Some(Command::SmartReply));
        assert_eq!(l.user.as_deref(), Some("alice@example.org"));
        assert_eq!(l.device_id, "ApplF17XYZ");
        assert_eq!(l.device_type, "iPhone");
        assert_eq!(l.item_id.as_deref(), Some("m:42"));
        assert!(l.save_in_sent);
        assert_eq!(l.policy_key, None);
    }

    #[test]
    fn plain_form_rejects_bad_ids() {
        assert_eq!(
            RequestLine::parse("Cmd=Sync&DeviceId=a%27%3B--&DeviceType=x"),
            Err(QueryError::Invalid("DeviceId"))
        );
        assert_eq!(
            RequestLine::parse("Cmd=Sync&DeviceType=x&User=a"),
            Err(QueryError::MissingDeviceId)
        );
        assert_eq!(
            RequestLine::parse("Cmd=Nope&DeviceId=a&DeviceType=b"),
            Err(QueryError::UnknownCommand("Nope".into()))
        );
    }

    fn base64_line(cmd: u8, policy: Option<u32>, params: &[(u8, &[u8])]) -> String {
        let mut b = vec![141u8, cmd, 0x09, 0x04];
        let dev = [0xDE, 0xAD, 0xBE, 0xEF];
        b.push(dev.len() as u8);
        b.extend_from_slice(&dev);
        match policy {
            Some(k) => {
                b.push(4);
                b.extend_from_slice(&k.to_le_bytes());
            }
            None => b.push(0),
        }
        b.push(7);
        b.extend_from_slice(b"Android");
        for (tag, val) in params {
            b.push(*tag);
            b.push(val.len() as u8);
            b.extend_from_slice(val);
        }
        base64::engine::general_purpose::STANDARD.encode(b)
    }

    #[test]
    fn base64_form() {
        let q = base64_line(
            3,
            Some(1_234_567_890),
            &[(8, b"bob"), (3, b"m:7"), (7, &[0x01])],
        );
        let l = RequestLine::parse(&q).unwrap();
        assert_eq!(l.command, Some(Command::SmartReply));
        assert_eq!(l.protocol_version, Some(ProtocolVersion::V14_1));
        assert_eq!(l.locale, Some(0x0409));
        assert_eq!(l.device_id, "deadbeef");
        assert_eq!(l.device_type, "Android");
        assert_eq!(l.policy_key, Some(1_234_567_890));
        assert_eq!(l.user.as_deref(), Some("bob"));
        assert_eq!(l.item_id.as_deref(), Some("m:7"));
        assert!(l.save_in_sent && !l.accept_multipart);
    }

    #[test]
    fn base64_form_url_escaped_and_truncated() {
        let q = base64_line(0, None, &[]);
        let escaped = q
            .replace('+', "%2B")
            .replace('/', "%2F")
            .replace('=', "%3D");
        assert_eq!(
            RequestLine::parse(&escaped).unwrap().command,
            Some(Command::Sync)
        );
        let mut raw = base64::engine::general_purpose::STANDARD
            .decode(&q)
            .unwrap();
        raw.truncate(8);
        let short = base64::engine::general_purpose::STANDARD.encode(raw);
        assert_eq!(RequestLine::parse(&short), Err(QueryError::Truncated));
    }

    #[test]
    fn options_request_has_no_command() {
        let l = RequestLine::parse("User=alice&DeviceId=abc&DeviceType=iPhone").unwrap();
        assert_eq!(l.command(), Err(QueryError::MissingCommand));
    }
}
