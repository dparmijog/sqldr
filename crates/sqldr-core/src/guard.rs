//! Guardrails shared by every frontend (CLI and TUI) that need to refuse
//! mutating statements on a `read_only` connection before it ever reaches
//! the network.

/// Flags statements that mutate data or schema based on their leading
/// keyword. Deliberately conservative: false positives (blocking a
/// harmless statement) are far cheaper than false negatives on a
/// production, read-only connection.
pub fn is_mutating(sql: &str) -> bool {
    let head = sql.trim_start().to_ascii_uppercase();
    const MUTATING: &[&str] = &[
        "INSERT", "UPDATE", "DELETE", "DROP", "ALTER", "CREATE", "TRUNCATE", "REPLACE", "GRANT",
        "REVOKE",
    ];
    MUTATING.iter().any(|kw| head.starts_with(kw))
}
