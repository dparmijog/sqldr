mod config;

use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use futures::StreamExt;
use sqldr_core::{ConnConfig, Driver, MySqlDriver, Row};
use tokio_util::sync::CancellationToken;

#[derive(Parser)]
#[command(name = "sqldr", about = "TUI + CLI para administrar bases de datos")]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Ejecuta una query contra una conexión configurada e imprime las filas.
    Query {
        #[arg(short = 'c', long = "conn")]
        connection: String,
        sql: String,
    },
    /// Administra credenciales de conexiones.
    Conn {
        #[command(subcommand)]
        action: ConnAction,
    },
}

#[derive(Subcommand)]
enum ConnAction {
    /// Guarda la contraseña de una conexión en el keyring del sistema.
    SetPassword {
        name: String,
        #[arg(long)]
        password: Option<String>,
    },
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();
    match cli.command {
        Commands::Query { connection, sql } => run_query(&connection, &sql).await,
        Commands::Conn { action } => run_conn(action).await,
    }
}

async fn run_query(connection: &str, sql: &str) -> Result<()> {
    let cfg = config::load()?;
    let entry = cfg.find(connection)?;
    let url = config::resolve_url(entry)?;

    if entry.read_only && is_mutating(sql) {
        anyhow::bail!(
            "connection '{connection}' es read-only; '{sql}' parece una escritura"
        );
    }

    let conn_cfg = ConnConfig {
        name: entry.name.clone(),
        url,
        read_only: entry.read_only,
    };
    let driver = MySqlDriver::connect(&conn_cfg)
        .await
        .with_context(|| format!("connecting to '{connection}'"))?;

    let cancel = CancellationToken::new();
    let mut stream = driver.query(sql, cancel);

    let mut rows: Vec<Row> = Vec::new();
    while let Some(row) = stream.next().await {
        rows.push(row?);
    }

    print_rows(&rows);
    Ok(())
}

async fn run_conn(action: ConnAction) -> Result<()> {
    match action {
        ConnAction::SetPassword { name, password } => {
            let password = match password {
                Some(p) => p,
                None => rpassword::prompt_password(format!("Password for '{name}': "))?,
            };
            config::set_password(&name, &password)?;
            println!("Contraseña guardada para '{name}'.");
            Ok(())
        }
    }
}

/// Rough guardrail: flags statements that mutate data/schema so read-only
/// connections refuse them before ever hitting the network.
fn is_mutating(sql: &str) -> bool {
    let head = sql.trim_start().to_ascii_uppercase();
    const MUTATING: &[&str] = &[
        "INSERT", "UPDATE", "DELETE", "DROP", "ALTER", "CREATE", "TRUNCATE", "REPLACE", "GRANT",
        "REVOKE",
    ];
    MUTATING.iter().any(|kw| head.starts_with(kw))
}

fn print_rows(rows: &[Row]) {
    let Some(first) = rows.first() else {
        println!("(0 rows)");
        return;
    };

    let mut widths: Vec<usize> = first.cols.iter().map(|c| c.len()).collect();
    for row in rows {
        for (i, v) in row.values.iter().enumerate() {
            widths[i] = widths[i].max(v.to_string().len());
        }
    }

    let print_sep = |widths: &[usize]| {
        let parts: Vec<String> = widths.iter().map(|w| "-".repeat(w + 2)).collect();
        println!("+{}+", parts.join("+"));
    };

    print_sep(&widths);
    let header: Vec<String> = first
        .cols
        .iter()
        .enumerate()
        .map(|(i, c)| format!(" {:width$} ", c, width = widths[i]))
        .collect();
    println!("|{}|", header.join("|"));
    print_sep(&widths);

    for row in rows {
        let cells: Vec<String> = row
            .values
            .iter()
            .enumerate()
            .map(|(i, v)| format!(" {:width$} ", v.to_string(), width = widths[i]))
            .collect();
        println!("|{}|", cells.join("|"));
    }
    print_sep(&widths);
    println!("({} row{})", rows.len(), if rows.len() == 1 { "" } else { "s" });
}
