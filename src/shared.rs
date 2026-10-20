use anyhow::{Context, Result, bail, ensure};
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use sha2::{Digest, Sha256};
use std::{
    fs::{self, File, OpenOptions},
    io::Write,
    path::Path,
    time::{SystemTime, UNIX_EPOCH},
};

pub fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

pub fn valid_id(id: &str) -> Result<()> {
    ensure!(
        !id.is_empty()
            && id.len() <= 64
            && id
                .bytes()
                .all(|c| c.is_ascii_alphanumeric() || c == b'-' || c == b'_'),
        "id must be 1–64 ASCII letters, digits, '-' or '_'"
    );
    Ok(())
}

pub fn digest(value: &str) -> String {
    hex::encode(Sha256::digest(value.as_bytes()))
}

pub fn token() -> Result<String> {
    let mut bytes = [0u8; 32];
    getrandom::fill(&mut bytes).map_err(|e| anyhow::anyhow!("OS randomness failed: {e}"))?;
    Ok(hex::encode(bytes))
}

pub fn origin(value: &str) -> Result<url::Url> {
    let url = url::Url::parse(value)?;
    ensure!(
        url.scheme() == "https"
            && url.host_str().is_some()
            && url.username().is_empty()
            && url.password().is_none()
            && url.path() == "/"
            && url.query().is_none()
            && url.fragment().is_none(),
        "origin must be https://domain[:port], without a path, query or credentials"
    );
    match url.host() {
        Some(url::Host::Domain(domain)) if domain.contains('.') && !domain.ends_with('.') => {}
        _ => bail!("a DNS domain is required; IP origins are forbidden"),
    }
    Ok(url)
}

pub fn read_json<T: DeserializeOwned>(path: &Path) -> Result<T> {
    let data = fs::read(path).with_context(|| format!("read {}", path.display()))?;
    ensure!(
        data.len() <= 1024 * 1024,
        "configuration/state exceeds 1 MiB"
    );
    serde_json::from_slice(&data).with_context(|| format!("parse {}", path.display()))
}

pub fn create_json<T: Serialize>(path: &Path, value: &T) -> Result<()> {
    let bytes = serde_json::to_vec_pretty(value)?;
    let mut f = OpenOptions::new().write(true).create_new(true).open(path)?;
    f.write_all(&bytes)?;
    f.sync_all()?;
    sync_parent(path)?;
    Ok(())
}

pub fn atomic_json<T: Serialize>(path: &Path, value: &T) -> Result<()> {
    let tmp = path.with_extension("pending");
    let bytes = serde_json::to_vec(value)?;
    // A private, locked state directory is required. Never silently reset corrupt state.
    let mut f = OpenOptions::new()
        .create(true)
        .truncate(true)
        .write(true)
        .open(&tmp)?;
    f.write_all(&bytes)?;
    f.sync_all()?;
    fs::rename(&tmp, path)?;
    sync_parent(path)
}

fn sync_parent(path: &Path) -> Result<()> {
    let parent = path
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    File::open(parent)?.sync_all()?;
    Ok(())
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Report {
    pub sequence: u64,
    pub hostname: String,
    pub os: String,
    pub arch: String,
    pub cpu_model: String,
    pub cpu_cores: u32,
    pub cpu_percent: f64,
    pub memory_used: u64,
    pub memory_total: u64,
    pub disk_used: u64,
    pub disk_total: u64,
    pub uptime_seconds: u64,
    pub load_1: f64,
    pub rx_bytes: u64,
    pub tx_bytes: u64,
    pub rx_bytes_per_second: f64,
    pub tx_bytes_per_second: f64,
    pub latency: Vec<Latency>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Latency {
    pub name: String,
    pub tcp_ms: Option<f64>,
}

impl Report {
    pub fn validate(&self) -> Result<()> {
        for s in [&self.hostname, &self.os, &self.arch, &self.cpu_model] {
            ensure!(
                s.len() <= 256 && !s.chars().any(char::is_control),
                "invalid metadata"
            );
        }
        ensure!(
            self.cpu_cores > 0 && self.cpu_cores <= 65536,
            "invalid CPU count"
        );
        ensure!(
            self.cpu_percent.is_finite() && (0.0..=100.0).contains(&self.cpu_percent),
            "invalid CPU usage"
        );
        ensure!(
            self.memory_used <= self.memory_total && self.disk_used <= self.disk_total,
            "invalid capacity"
        );
        for n in [
            self.load_1,
            self.rx_bytes_per_second,
            self.tx_bytes_per_second,
        ] {
            ensure!(
                n.is_finite() && (0.0..=1e15).contains(&n),
                "invalid rate/load"
            );
        }
        ensure!(
            self.sequence > 0 && self.sequence <= i64::MAX as u64,
            "invalid sequence"
        );
        ensure!(self.latency.len() <= 8, "at most 8 latency targets");
        for p in &self.latency {
            ensure!(
                !p.name.is_empty() && p.name.len() <= 64 && !p.name.chars().any(char::is_control),
                "invalid target name"
            );
            if let Some(ms) = p.tcp_ms {
                ensure!(
                    ms.is_finite() && (0.0..=10000.0).contains(&ms),
                    "invalid latency"
                );
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn origins_require_https_dns_and_no_credentials() {
        for bad in [
            "http://a.example",
            "https://127.0.0.1",
            "https://[::1]",
            "https://u:p@a.example",
            "https://a.example/path",
            "https://a.example?x=1",
            "https://localhost",
        ] {
            assert!(origin(bad).is_err(), "{bad}");
        }
        assert!(origin("https://probe.example:443").is_ok());
    }
    #[test]
    fn strict_ids() {
        for bad in ["", "../etc", "a/b", "a b", "节点"] {
            assert!(valid_id(bad).is_err());
        }
        assert!(valid_id("vps_hk-01").is_ok());
    }
    #[test]
    fn random_tokens_are_distinct_and_256_bit() {
        let a = token().unwrap();
        assert_eq!(a.len(), 64);
        assert_ne!(a, token().unwrap());
        assert_eq!(digest(&a).len(), 64);
    }
}
