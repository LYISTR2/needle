#![deny(unsafe_op_in_unsafe_fn)]

mod agent;
mod server;
mod shared;

use anyhow::{Result, bail};
use clap::{Parser, Subcommand};
use std::path::PathBuf;

#[derive(Parser)]
#[command(version, about = "Needle — a small, outbound-only Linux probe")]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Create panel configuration and a read-only dashboard credential.
    Init {
        #[arg(long)]
        config: PathBuf,
        #[arg(long)]
        origin: String,
        #[arg(long, default_value = "/var/lib/needle-panel/panel.db")]
        database: PathBuf,
        #[arg(long, default_value = "/run/needle-panel/http.sock")]
        socket: PathBuf,
    },
    /// Listen ONLY on a Unix socket. Requires an HTTPS reverse proxy.
    Serve {
        #[arg(long)]
        config: PathBuf,
    },
    /// Register one node and write its credential to a NEW agent config file.
    AddNode {
        #[arg(long)]
        config: PathBuf,
        #[arg(long)]
        id: String,
        #[arg(long)]
        name: String,
        #[arg(long)]
        agent_config: PathBuf,
        #[arg(long)]
        interface: String,
    },
    /// Revoke a node credential and remove its last report.
    RemoveNode {
        #[arg(long)]
        config: PathBuf,
        #[arg(long)]
        id: String,
    },
    /// Collect local metrics and push to the panel; no listening socket.
    Agent {
        #[arg(long)]
        config: PathBuf,
        /// Collect and submit exactly one report (also persists traffic).
        #[arg(long)]
        once: bool,
    },
    /// Replace cumulative traffic in bytes. Stop the agent first.
    SetTraffic {
        #[arg(long)]
        config: PathBuf,
        #[arg(long)]
        rx: u64,
        #[arg(long)]
        tx: u64,
    },
}

#[tokio::main(flavor = "current_thread")]
async fn main() -> Result<()> {
    // Private files by default; the reverse-proxy socket alone is made group-readable.
    unsafe {
        libc::umask(0o077);
    }
    match Cli::parse().command {
        Command::Init {
            config,
            origin,
            database,
            socket,
        } => {
            server::initialize(&config, &origin, database, socket)?;
        }
        Command::Serve { config } => server::serve(&config).await?,
        Command::AddNode {
            config,
            id,
            name,
            agent_config,
            interface,
        } => {
            server::add_node(&config, &id, &name, &agent_config, &interface)?;
        }
        Command::RemoveNode { config, id } => {
            shared::valid_id(&id)?;
            let cfg: server::Config = shared::read_json(&config)?;
            let db = server::open_db(&cfg.database)?;
            if db.execute("DELETE FROM nodes WHERE id = ?1", [&id])? == 0 {
                bail!("node not found");
            }
            println!("Node removed and credential revoked: {id}");
        }
        Command::Agent { config, once } => agent::run(&config, once).await?,
        Command::SetTraffic { config, rx, tx } => agent::set_traffic(&config, rx, tx)?,
    }
    Ok(())
}
