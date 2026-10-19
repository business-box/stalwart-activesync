//! The `mobilesync` autodiscover exchange.
//!
//! Phones POST a small XML request naming the user's address and the
//! response schema they accept. The gateway answers `mobilesync` requests
//! itself and hands every other schema (Outlook's) to Stalwart unchanged.

/// Response schema EAS clients ask for.
pub const MOBILESYNC_SCHEMA: &str =
    "http://schemas.microsoft.com/exchange/autodiscover/mobilesync/responseschema/2006";

/// The parts of an autodiscover request the gateway needs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AutodiscoverRequest {
    pub email: String,
    pub response_schema: String,
}

impl AutodiscoverRequest {
    /// Extracts `EMailAddress` and `AcceptableResponseSchema` from the
    /// request body. This is deliberately a tolerant text scan, not an XML
    /// parser: the body is tiny, clients vary namespaces and prefixes, and
    /// nothing else in it matters.
    pub fn parse(body: &str) -> Option<AutodiscoverRequest> {
        Some(AutodiscoverRequest {
            email: element_text(body, "EMailAddress")?,
            response_schema: element_text(body, "AcceptableResponseSchema")?,
        })
    }

    pub fn is_mobilesync(&self) -> bool {
        self.response_schema.eq_ignore_ascii_case(MOBILESYNC_SCHEMA)
    }
}

/// Text of the first element whose local name is `local`, with any prefix.
fn element_text(body: &str, local: &str) -> Option<String> {
    let mut rest = body;
    while let Some(lt) = rest.find('<') {
        rest = &rest[lt + 1..];
        let end = rest.find('>')?;
        let tag = &rest[..end];
        let name = tag.split_whitespace().next().unwrap_or("");
        let name = name.rsplit(':').next().unwrap_or(name);
        if name == local && !tag.starts_with('/') && !tag.ends_with('/') {
            let content = &rest[end + 1..];
            let close = content.find('<')?;
            let text = content[..close].trim();
            return (!text.is_empty()).then(|| unescape(text));
        }
        rest = &rest[end + 1..];
    }
    None
}

fn unescape(s: &str) -> String {
    s.replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&apos;", "'")
        .replace("&amp;", "&")
}

fn escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

/// The `mobilesync` response pointing the device at `eas_url`
/// (`https://mail.example.org/Microsoft-Server-ActiveSync`).
pub fn mobilesync_response(email: &str, display_name: &str, eas_url: &str) -> String {
    let (email, name, url) = (escape(email), escape(display_name), escape(eas_url));
    format!(
        r#"<?xml version="1.0" encoding="utf-8"?>
<Autodiscover xmlns="http://schemas.microsoft.com/exchange/autodiscover/responseschema/2006">
  <Response xmlns="{MOBILESYNC_SCHEMA}">
    <Culture>en:us</Culture>
    <User>
      <DisplayName>{name}</DisplayName>
      <EMailAddress>{email}</EMailAddress>
    </User>
    <Action>
      <Settings>
        <Server>
          <Type>MobileSync</Type>
          <Url>{url}</Url>
          <Name>{url}</Name>
        </Server>
      </Settings>
    </Action>
  </Response>
</Autodiscover>
"#
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    const IOS_REQUEST: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<Autodiscover xmlns="http://schemas.microsoft.com/exchange/autodiscover/mobilesync/requestschema/2006">
  <Request>
    <EMailAddress>alice@example.org</EMailAddress>
    <AcceptableResponseSchema>http://schemas.microsoft.com/exchange/autodiscover/mobilesync/responseschema/2006</AcceptableResponseSchema>
  </Request>
</Autodiscover>"#;

    #[test]
    fn parses_mobilesync_request() {
        let r = AutodiscoverRequest::parse(IOS_REQUEST).unwrap();
        assert_eq!(r.email, "alice@example.org");
        assert!(r.is_mobilesync());
    }

    #[test]
    fn outlook_schema_is_not_mobilesync() {
        let body = IOS_REQUEST.replace("mobilesync/responseschema", "outlook/responseschema");
        let body = body
            .replace("<EMailAddress>", "<a:EMailAddress xmlns:a=\"x\">")
            .replace("</EMailAddress>", "</a:EMailAddress>");
        let r = AutodiscoverRequest::parse(&body).unwrap();
        assert_eq!(r.email, "alice@example.org");
        assert!(!r.is_mobilesync());
    }

    #[test]
    fn response_is_escaped() {
        let x = mobilesync_response(
            "a&b@example.org",
            "A <B>",
            "https://mail.example.org/Microsoft-Server-ActiveSync",
        );
        assert!(x.contains("<EMailAddress>a&amp;b@example.org</EMailAddress>"));
        assert!(x.contains("<DisplayName>A &lt;B&gt;</DisplayName>"));
        assert!(x.contains("<Type>MobileSync</Type>"));
    }

    #[test]
    fn garbage_is_rejected() {
        assert_eq!(AutodiscoverRequest::parse("not xml"), None);
        assert_eq!(AutodiscoverRequest::parse("<EMailAddress/>"), None);
    }
}
