use crate::shared::{self, Latency, Report};
use anyhow::{Context, Result, ensure};
use fs2::FileExt;
use serde::{Deserialize, Serialize};
use std::{
    collections::{BTreeMap, BTreeSet},
    fs::{self, File, OpenOptions},
    net::SocketAddr,
    path::{Path, PathBuf},
    time::{Duration, Instant},
};

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Config {
    pub endpoint: String,
    #[serde(default)]
    pub path_prefix: String,
    pub node_id: String,
    pub token: String,
    pub state_dir: PathBuf,
    pub interfaces: Vec<String>,
    pub interval_seconds: u64,
    #[serde(default)]
    pub targets: Vec<Target>,
    /// Optional private PKI certificate. TLS verification is never disabled.
    #[serde(default)]
    pub ca_certificate: Option<PathBuf>,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Target {
    pub name: String,
    pub address: SocketAddr,
}

#[derive(Default, Debug, Serialize, Deserialize, Clone)]
#[serde(deny_unknown_fields)]
struct Counter {
    rx: u64,
    tx: u64,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Ledger {
    version: u8,
    node_id: String,
    boot_id: String,
    sequence: u64,
    rx: u64,
    tx: u64,
    previous: BTreeMap<String, Counter>,
}

pub fn valid_interface(value: &str) -> Result<()> {
    ensure!(
        !value.is_empty()
            && value.len() <= 15
            && value != "lo"
            && value
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b"_-.:".contains(&b)),
        "invalid interface (loopback is excluded)"
    );
    Ok(())
}

fn valid_prefix(prefix: &str) -> Result<()> {
    ensure!(
        prefix.is_empty()
            || (prefix.starts_with('/')
                && prefix.len() <= 80
                && prefix[1..].split('/').all(|part| !part.is_empty()
                    && part
                        .bytes()
                        .all(|b| b.is_ascii_alphanumeric() || b"_-".contains(&b)))),
        "path_prefix must be empty or a safe path such as /needle, without trailing slash"
    );
    Ok(())
}

fn config(path: &Path) -> Result<Config> {
    let cfg: Config = shared::read_json(path)?;
    shared::origin(&cfg.endpoint)?;
    valid_prefix(&cfg.path_prefix)?;
    shared::valid_id(&cfg.node_id)?;
    ensure!(
        cfg.token.len() == 64 && hex::decode(&cfg.token).is_ok(),
        "token must be 256-bit hexadecimal"
    );
    ensure!(cfg.state_dir.is_absolute(), "state_dir must be absolute");
    ensure!(
        (5..=30).contains(&cfg.interval_seconds),
        "interval must be 5–30 seconds"
    );
    ensure!(
        !cfg.interfaces.is_empty() && cfg.interfaces.len() <= 16,
        "select 1–16 external interfaces (avoid bridge/veth double counting)"
    );
    for name in &cfg.interfaces {
        valid_interface(name)?;
    }
    ensure!(
        cfg.interfaces.iter().collect::<BTreeSet<_>>().len() == cfg.interfaces.len(),
        "duplicate interfaces"
    );
    ensure!(cfg.targets.len() <= 8, "at most 8 TCP latency targets");
    for t in &cfg.targets {
        ensure!(
            !t.name.is_empty()
                && t.name.len() <= 64
                && !t.name.chars().any(char::is_control)
                && t.address.port() != 0,
            "invalid latency target"
        );
    }
    Ok(cfg)
}

fn lock_state(cfg: &Config) -> Result<File> {
    fs::create_dir_all(&cfg.state_dir)?;
    let f = OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(cfg.state_dir.join("agent.lock"))?;
    f.try_lock_exclusive()
        .context("agent is running; stop it before editing traffic")?;
    Ok(f)
}

fn boot_id() -> Result<String> {
    Ok(fs::read_to_string("/proc/sys/kernel/random/boot_id")?
        .trim()
        .into())
}

fn raw_network(cfg: &Config) -> Result<BTreeMap<String, Counter>> {
    let data = fs::read_to_string("/proc/net/dev")?;
    let mut counters = BTreeMap::new();
    for name in &cfg.interfaces {
        let line = data
            .lines()
            .find_map(|line| {
                let (iface, values) = line.rsplit_once(':')?;
                (iface.trim() == name).then_some(values)
            })
            .with_context(|| {
                format!("configured interface {name} is absent; refusing silent accounting changes")
            })?;
        let values: Vec<&str> = line.split_whitespace().collect();
        ensure!(values.len() >= 16, "invalid kernel network counters");
        counters.insert(
            name.clone(),
            Counter {
                rx: values[0].parse()?,
                tx: values[8].parse()?,
            },
        );
    }
    Ok(counters)
}

fn ledger(cfg: &Config) -> Result<Ledger> {
    let path = cfg.state_dir.join("traffic.json");
    match fs::metadata(&path) {
        Ok(_) => {
            let l: Ledger = shared::read_json(&path)?;
            ensure!(
                l.version == 1 && l.node_id == cfg.node_id && l.sequence <= i64::MAX as u64,
                "incompatible state or node identity mismatch; do not delete accounting state"
            );
            Ok(l)
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Ledger {
            version: 1,
            node_id: cfg.node_id.clone(),
            boot_id: boot_id()?,
            sequence: 0,
            rx: 0,
            tx: 0,
            previous: BTreeMap::new(),
        }),
        Err(e) => Err(e.into()),
    }
}

fn difference(current: u64, previous: u64, same_boot: bool) -> u64 {
    if same_boot && current >= previous {
        current - previous
    } else {
        current
    }
}

fn account(
    ledger: &mut Ledger,
    boot: String,
    current: BTreeMap<String, Counter>,
) -> Result<(u64, u64)> {
    let mut rx = 0u64;
    let mut tx = 0u64;
    for (name, value) in &current {
        let previous = ledger.previous.get(name).cloned().unwrap_or_default();
        rx = rx
            .checked_add(difference(value.rx, previous.rx, ledger.boot_id == boot))
            .context("RX overflow")?;
        tx = tx
            .checked_add(difference(value.tx, previous.tx, ledger.boot_id == boot))
            .context("TX overflow")?;
    }
    ledger.rx = ledger.rx.checked_add(rx).context("total RX overflow")?;
    ledger.tx = ledger.tx.checked_add(tx).context("total TX overflow")?;
    ledger.previous = current;
    ledger.boot_id = boot;
    Ok((rx, tx))
}

pub fn set_traffic(path: &Path, rx: u64, tx: u64) -> Result<()> {
    let cfg = config(path)?;
    let _lock = lock_state(&cfg)?;
    let mut state = ledger(&cfg)?;
    let file = cfg.state_dir.join("traffic.json");
    if file.exists() {
        let backup = cfg
            .state_dir
            .join(format!("traffic.backup-{}.json", shared::token()?));
        shared::create_json(&backup, &state)?;
        println!("Previous ledger backed up to {}", backup.display());
    }
    state.rx = rx;
    state.tx = tx;
    state.previous = raw_network(&cfg)?;
    state.boot_id = boot_id()?;
    shared::atomic_json(&file, &state)?;
    println!("Cumulative traffic set: RX={rx} bytes, TX={tx} bytes; sequence preserved");
    Ok(())
}

#[derive(Clone, Copy)]
struct Cpu {
    total: u64,
    idle: u64,
}
fn cpu() -> Result<Cpu> {
    let data = fs::read_to_string("/proc/stat")?;
    let values = data
        .lines()
        .next()
        .context("missing CPU counters")?
        .split_whitespace()
        .skip(1)
        .take(8)
        .map(str::parse::<u64>)
        .collect::<std::result::Result<Vec<_>, _>>()?;
    ensure!(values.len() >= 5, "incomplete CPU counters");
    Ok(Cpu {
        total: values.iter().sum(),
        idle: values[3] + values[4],
    })
}

fn cpu_usage(before: Cpu, after: Cpu) -> f64 {
    let total = after.total.saturating_sub(before.total);
    let idle = after.idle.saturating_sub(before.idle);
    if total == 0 {
        0.0
    } else {
        ((total.saturating_sub(idle)) as f64 / total as f64 * 100.0).clamp(0.0, 100.0)
    }
}

fn memory() -> Result<(u64, u64)> {
    let data = fs::read_to_string("/proc/meminfo")?;
    let field = |name: &str| -> Result<u64> {
        let line = data
            .lines()
            .find(|line| line.starts_with(name))
            .context("missing memory field")?;
        Ok(line
            .split_whitespace()
            .nth(1)
            .context("invalid memory field")?
            .parse::<u64>()?
            * 1024)
    };
    let total = field("MemTotal:")?;
    Ok((total.saturating_sub(field("MemAvailable:")?), total))
}

fn disk() -> Result<(u64, u64)> {
    let mut value = std::mem::MaybeUninit::<libc::statvfs>::uninit();
    // SAFETY: the C string is terminated; statvfs initializes the output on success.
    let result = unsafe { libc::statvfs(c"/".as_ptr(), value.as_mut_ptr()) };
    ensure!(
        result == 0,
        "statvfs failed: {}",
        std::io::Error::last_os_error()
    );
    let value = unsafe { value.assume_init() };
    let total = value.f_blocks.saturating_mul(value.f_frsize);
    let free = value.f_bfree.saturating_mul(value.f_frsize);
    Ok((total.saturating_sub(free), total))
}

fn clean(value: &str) -> String {
    value.chars().filter(|c| !c.is_control()).take(64).collect()
}

async fn measure(targets: &[Target]) -> Vec<Latency> {
    let mut tasks = tokio::task::JoinSet::new();
    for t in targets {
        let name = t.name.clone();
        let address = t.address;
        tasks.spawn(async move {
            let start = Instant::now();
            let tcp_ms = match tokio::time::timeout(
                Duration::from_secs(3),
                tokio::net::TcpStream::connect(address),
            )
            .await
            {
                Ok(Ok(stream)) => {
                    drop(stream);
                    Some(start.elapsed().as_secs_f64() * 1000.0)
                }
                _ => None,
            };
            Latency { name, tcp_ms }
        });
    }
    let mut result = Vec::new();
    while let Some(Ok(sample)) = tasks.join_next().await {
        result.push(sample);
    }
    result.sort_by(|a, b| a.name.cmp(&b.name));
    result
}

async fn collect(
    state: &mut Ledger,
    cfg: &Config,
    previous_cpu: &mut Cpu,
    previous_time: &mut Instant,
) -> Result<Report> {
    let current_cpu = cpu()?;
    let elapsed = previous_time.elapsed().as_secs_f64().max(0.001);
    *previous_time = Instant::now();
    let (rx, tx) = account(state, boot_id()?, raw_network(cfg)?)?;
    state.sequence = state
        .sequence
        .checked_add(1)
        .filter(|v| *v <= i64::MAX as u64)
        .context("sequence exhausted")?;
    // Commit before attempting the network; transport errors must not lose accounting.
    shared::atomic_json(&cfg.state_dir.join("traffic.json"), state)?;
    let (memory_used, memory_total) = memory()?;
    let (disk_used, disk_total) = disk()?;
    let info = fs::read_to_string("/proc/cpuinfo")?;
    let cpu_model = info
        .lines()
        .find_map(|line| {
            let (key, value) = line.split_once(':')?;
            matches!(key.trim(), "model name" | "Hardware").then(|| clean(value.trim()))
        })
        .unwrap_or_else(|| std::env::consts::ARCH.into());
    let cpu_cores = fs::read_to_string("/proc/stat")?
        .lines()
        .filter(|line| {
            line.starts_with("cpu") && line.as_bytes().get(3).is_some_and(u8::is_ascii_digit)
        })
        .count() as u32;
    let os = fs::read_to_string("/etc/os-release")
        .ok()
        .and_then(|text| {
            text.lines().find_map(|line| {
                line.strip_prefix("PRETTY_NAME=")
                    .map(|v| clean(v.trim_matches('"')))
            })
        })
        .unwrap_or_else(|| "Linux".into());
    let report = Report {
        sequence: state.sequence,
        hostname: clean(fs::read_to_string("/proc/sys/kernel/hostname")?.trim()),
        os,
        arch: std::env::consts::ARCH.into(),
        cpu_model,
        cpu_cores,
        cpu_percent: cpu_usage(*previous_cpu, current_cpu),
        memory_used,
        memory_total,
        disk_used,
        disk_total,
        uptime_seconds: fs::read_to_string("/proc/uptime")?
            .split_whitespace()
            .next()
            .context("missing uptime")?
            .parse::<f64>()? as u64,
        load_1: fs::read_to_string("/proc/loadavg")?
            .split_whitespace()
            .next()
            .context("missing load")?
            .parse()?,
        rx_bytes: state.rx,
        tx_bytes: state.tx,
        rx_bytes_per_second: rx as f64 / elapsed,
        tx_bytes_per_second: tx as f64 / elapsed,
        latency: measure(&cfg.targets).await,
    };
    *previous_cpu = current_cpu;
    report.validate()?;
    Ok(report)
}

pub async fn run(path: &Path, once: bool) -> Result<()> {
    let cfg = config(path)?;
    let _lock = lock_state(&cfg)?;
    let mut state = ledger(&cfg)?;
    // Initial baseline counts current-boot traffic once, but does not inflate live rate.
    account(&mut state, boot_id()?, raw_network(&cfg)?)?;
    shared::atomic_json(&cfg.state_dir.join("traffic.json"), &state)?;
    let mut builder = reqwest::Client::builder()
        .https_only(true)
        .no_proxy()
        .redirect(reqwest::redirect::Policy::none())
        .connect_timeout(Duration::from_secs(5))
        .timeout(Duration::from_secs(10))
        .pool_max_idle_per_host(1)
        .user_agent(concat!("needle/", env!("CARGO_PKG_VERSION")));
    if let Some(cert) = &cfg.ca_certificate {
        builder = builder.add_root_certificate(reqwest::Certificate::from_pem(&fs::read(cert)?)?);
    }
    let client = builder.build()?;
    let endpoint = format!(
        "{}{}/api/report/{}",
        cfg.endpoint.trim_end_matches('/'),
        cfg.path_prefix,
        cfg.node_id
    );
    let mut previous_cpu = cpu()?;
    let mut previous_time = Instant::now();
    let mut signal = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())?;
    tokio::time::sleep(Duration::from_secs(1)).await;
    loop {
        let report = collect(&mut state, &cfg, &mut previous_cpu, &mut previous_time).await?;
        let result = client
            .post(&endpoint)
            .bearer_auth(&cfg.token)
            .json(&report)
            .send()
            .await;
        let success = match result {
            Ok(response) if response.status() == reqwest::StatusCode::NO_CONTENT => true,
            Ok(response) => {
                eprintln!(
                    "Report rejected: HTTP {} (accounting persisted)",
                    response.status().as_u16()
                );
                false
            }
            Err(_) => {
                eprintln!("Report delivery failed (network/TLS/timeout); accounting persisted");
                false
            }
        };
        if once {
            ensure!(success, "report not accepted");
            println!(
                "Report accepted; cumulative RX={} TX={} bytes",
                state.rx, state.tx
            );
            break;
        }
        tokio::select! {
            _ = tokio::time::sleep(Duration::from_secs(cfg.interval_seconds)) => {},
            _ = tokio::signal::ctrl_c() => { break; },
            _ = signal.recv() => { break; },
        }
    }
    account(&mut state, boot_id()?, raw_network(&cfg)?)?;
    shared::atomic_json(&cfg.state_dir.join("traffic.json"), &state)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    fn counters(rx: u64, tx: u64) -> BTreeMap<String, Counter> {
        BTreeMap::from([("eth0".into(), Counter { rx, tx })])
    }
    fn fresh() -> Ledger {
        Ledger {
            version: 1,
            node_id: "test".into(),
            boot_id: "boot-a".into(),
            sequence: 0,
            rx: 0,
            tx: 0,
            previous: BTreeMap::new(),
        }
    }
    #[test]
    fn persists_across_restart_and_reboot() {
        let mut l = fresh();
        account(&mut l, "boot-a".into(), counters(100, 50)).unwrap();
        let mut l: Ledger = serde_json::from_slice(&serde_json::to_vec(&l).unwrap()).unwrap();
        account(&mut l, "boot-a".into(), counters(150, 80)).unwrap();
        assert_eq!((l.rx, l.tx), (150, 80));
        account(&mut l, "boot-b".into(), counters(20, 10)).unwrap();
        assert_eq!((l.rx, l.tx), (170, 90));
    }
    #[test]
    fn reboot_detected_even_if_new_counters_are_larger() {
        let mut l = fresh();
        account(&mut l, "boot-a".into(), counters(100, 50)).unwrap();
        account(&mut l, "boot-b".into(), counters(200, 80)).unwrap();
        assert_eq!((l.rx, l.tx), (300, 130));
    }
    #[test]
    fn counter_reset_and_no_double_count() {
        let mut l = fresh();
        account(&mut l, "boot-a".into(), counters(100, 50)).unwrap();
        assert_eq!(
            account(&mut l, "boot-a".into(), counters(100, 50)).unwrap(),
            (0, 0)
        );
        account(&mut l, "boot-a".into(), counters(10, 5)).unwrap();
        assert_eq!((l.rx, l.tx), (110, 55));
    }
    #[test]
    fn manual_total_keeps_baseline() {
        let mut l = fresh();
        l.rx = 1000;
        l.tx = 2000;
        l.previous = counters(100, 200);
        account(&mut l, "boot-a".into(), counters(150, 260)).unwrap();
        assert_eq!((l.rx, l.tx), (1050, 2060));
    }
    #[test]
    fn rejects_overflow_and_bad_interfaces() {
        let mut l = fresh();
        l.rx = u64::MAX;
        assert!(account(&mut l, "boot-a".into(), counters(1, 0)).is_err());
        for bad in ["lo", "../eth0", "eth0/foo", ""] {
            assert!(valid_interface(bad).is_err());
        }
        assert!(valid_interface("enp1s0").is_ok());
    }
    #[test]
    fn prefix_cannot_change_destination_or_escape_route() {
        for prefix in ["", "/needle", "/monitor/needle-1"] {
            assert!(valid_prefix(prefix).is_ok());
        }
        for prefix in [
            "//evil.example",
            "/../admin",
            "/needle/",
            "/x?y",
            "/x#y",
            "/%2e%2e",
            "https://evil.example",
        ] {
            assert!(valid_prefix(prefix).is_err());
        }
    }
    #[test]
    fn cpu_uses_delta_not_lifetime_average() {
        assert_eq!(
            cpu_usage(
                Cpu {
                    total: 100,
                    idle: 50
                },
                Cpu {
                    total: 200,
                    idle: 75
                }
            ),
            75.0
        );
        assert_eq!(
            cpu_usage(
                Cpu {
                    total: 100,
                    idle: 50
                },
                Cpu {
                    total: 100,
                    idle: 50
                }
            ),
            0.0
        );
    }
}
