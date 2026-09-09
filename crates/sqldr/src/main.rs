mod app;
mod clipboard;
mod config;
mod favorites;
mod theme;
mod tui;
mod ui;

use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use futures::StreamExt;
use sqldr_core::{is_mutating, ConnConfig, Driver, MySqlDriver, Row};
use tokio_util::sync::CancellationToken;

#[derive(Parser)]
#[command(name = "sqldr", version, about = "TUI + CLI for managing databases")]
struct Cli {
    #[command(subcommand)]
    command: Option<Commands>,
}

#[derive(Subcommand)]
enum Commands {
    /// Runs a query against a configured connection and prints the rows.
    Query {
        #[arg(short = 'c', long = "conn")]
        connection: String,
        sql: String,
    },
    /// Manages connection credentials.
    Conn {
        #[command(subcommand)]
        action: ConnAction,
    },
}

#[derive(Subcommand)]
enum ConnAction {
    /// Saves a connection's password in the system keyring.
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
        Some(Commands::Query { connection, sql }) => run_query(&connection, &sql).await,
        Some(Commands::Conn { action }) => run_conn(action).await,
        None => tui::run(config::load()?).await,
    }
}

async fn run_query(connection: &str, sql: &str) -> Result<()> {
    let cfg = config::load()?;
    let entry = cfg.find(connection)?;
    let url = config::resolve_url(entry)?;

    if entry.read_only && is_mutating(sql) {
        anyhow::bail!(
            "connection '{connection}' is read-only; '{sql}' looks like a write"
        );
    }

    if sqldr_core::needs_where_confirmation(sql) && !confirm_where(sql)? {
        anyhow::bail!("cancelled by user");
    }

    let conn_cfg = ConnConfig {
        name: entry.name.clone(),
        url,
        read_only: entry.read_only,
    };
    let driver = MySqlDriver::connect(&conn_cfg)
        .await
        .with_context(|| format!("connecting to '{connection}'"))?;

    if let Ok(path) = config::history_path(&entry.name) {
        if let Ok(mut history) = sqldr_core::History::load(path) {
            let _ = history.push(sql);
        }
    }

    let cancel = CancellationToken::new();
    let mut stream = driver.query(sql, cancel);

    let mut rows: Vec<Row> = Vec::new();
    while let Some(row) = stream.next().await {
        rows.push(row?);
    }

    print_rows(&rows);
    Ok(())
}

/// Prompts on stderr for a `y`/`N` confirmation before running an
/// `UPDATE`/`DELETE` without a `WHERE` clause. Returns `false` on anything
/// but an explicit "y" (including EOF, e.g. non-interactive stdin).
fn confirm_where(sql: &str) -> Result<bool> {
    use std::io::Write;
    eprintln!("No WHERE clause — run anyway?\n{sql}");
    eprint!("Type 'y' to confirm: ");
    std::io::stderr().flush()?;
    let mut answer = String::new();
    std::io::stdin().read_line(&mut answer)?;
    Ok(answer.trim().eq_ignore_ascii_case("y"))
}

async fn run_conn(action: ConnAction) -> Result<()> {
    match action {
        ConnAction::SetPassword { name, password } => {
            let password = match password {
                Some(p) => p,
                None => rpassword::prompt_password(format!("Password for '{name}': "))?,
            };
            config::set_password(&name, &password)?;
            println!("Password saved for '{name}'.");
            Ok(())
        }
    }
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
