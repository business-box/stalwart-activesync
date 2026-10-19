use std::net::SocketAddr;

/// Runtime configuration, read from the environment.
#[derive(Debug, Clone)]
pub struct Config {
    /// `EAS_LISTEN`, default `0.0.0.0:8080`.
    pub listen: SocketAddr,
    /// `STALWART_URL`: Stalwart's HTTP root as seen from the gateway, for
    /// example `http://stalwart:8080`. Required.
    pub stalwart_url: String,
    /// `EAS_PUBLIC_URL`: the public origin phones use, for example
    /// `https://mail.example.org`. Used in autodiscover answers. Required.
    pub public_url: String,
    /// `EAS_TRUST_FORWARDED`, default `true`: take the client IP from
    /// `X-Forwarded-For`. Only safe when the gateway is reachable through
    /// the reverse proxy alone.
    pub trust_forwarded: bool,
    /// `EAS_MAX_BODY_MB`, default 50: largest request body accepted, which
    /// bounds outgoing attachments in `SendMail`.
    pub max_body_bytes: usize,
    /// `EAS_AUTH_CACHE_SECS`, default 300: how long a successful login is
    /// reused before Stalwart is asked again.
    pub auth_cache_secs: u64,
    /// `EAS_DB_URL`, default `sqlite://eas.db`: the state database.
    pub db_url: String,
    /// `EAS_REQUIRE_PROVISIONING`, default `true`: refuse commands from
    /// devices without a policy key. Remote wipe only works with it on.
    pub require_provisioning: bool,
    /// `EAS_ADMIN_TOKEN`: bearer token for `/admin`; the admin API is off
    /// without it.
    pub admin_token: Option<String>,
}

impl Config {
    pub fn from_env() -> Result<Config, String> {
        let var = |k: &str| std::env::var(k).ok().filter(|v| !v.trim().is_empty());
        let listen = var("EAS_LISTEN")
            .unwrap_or_else(|| "0.0.0.0:8080".into())
            .parse()
            .map_err(|e| format!("EAS_LISTEN: {e}"))?;
        let stalwart_url = var("STALWART_URL").ok_or("STALWART_URL is required")?;
        let public_url = var("EAS_PUBLIC_URL")
            .ok_or("EAS_PUBLIC_URL is required")?
            .trim_end_matches('/')
            .to_string();
        let trust_forwarded = var("EAS_TRUST_FORWARDED")
            .map(|v| !matches!(v.to_ascii_lowercase().as_str(), "0" | "false" | "no"))
            .unwrap_or(true);
        let max_body_bytes = var("EAS_MAX_BODY_MB")
            .map(|v| {
                v.parse::<usize>()
                    .map_err(|e| format!("EAS_MAX_BODY_MB: {e}"))
            })
            .transpose()?
            .unwrap_or(50)
            * 1024
            * 1024;
        let auth_cache_secs = var("EAS_AUTH_CACHE_SECS")
            .map(|v| v.parse().map_err(|e| format!("EAS_AUTH_CACHE_SECS: {e}")))
            .transpose()?
            .unwrap_or(300);
        let db_url = var("EAS_DB_URL").unwrap_or_else(|| "sqlite://eas.db".into());
        let require_provisioning = var("EAS_REQUIRE_PROVISIONING")
            .map(|v| !matches!(v.to_ascii_lowercase().as_str(), "0" | "false" | "no"))
            .unwrap_or(true);
        let admin_token = var("EAS_ADMIN_TOKEN");
        Ok(Config {
            listen,
            stalwart_url,
            public_url,
            trust_forwarded,
            max_body_bytes,
            auth_cache_secs,
            db_url,
            require_provisioning,
            admin_token,
        })
    }

    /// Full EAS endpoint URL advertised by autodiscover.
    pub fn eas_url(&self) -> String {
        format!("{}{}", self.public_url, eas_proto::ENDPOINT_PATH)
    }
}
