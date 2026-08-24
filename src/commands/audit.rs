//! `nod audit` command: read the recorded deployment audit trail (ADR-003
//! observability). Backed by `AuditLogUseCase` over `AuditStorePort`.

use colored::Colorize;
use std::sync::Arc;

use crate::application::context::AppContext;
use crate::application::use_cases::audit_log::AuditLogUseCase;
use crate::domain::errors::NodError;

pub async fn execute(
    ctx: AppContext,
    target: Option<&str>,
    limit: Option<usize>,
    json: bool,
) -> Result<(), NodError> {
    // The audit store is bound by `main` (a single, explicit site) because it
    // is an opt-in service: `production` builds the base graph without it.
    let ctx = Arc::new(ctx);
    let use_case = AuditLogUseCase::new(ctx);
    let entries = use_case.execute(target, limit).await?;

    if json {
        println!("{}", serde_json::to_string(&entries).unwrap());
        return Ok(());
    }

    if entries.is_empty() {
        println!("{}", "No deployment history recorded yet.".dimmed());
        return Ok(());
    }

    println!(
        "\n{:<20} {:<14} {:<24}",
        "HOST".bold(),
        "OUTCOME".bold(),
        "RECORDED (UTC)".bold()
    );
    for entry in entries {
        let outcome = entry.outcome.clone();
        let colored = if outcome == "completed" {
            outcome.green()
        } else {
            outcome.yellow()
        };
        let timestamp_str = format_epoch_utc(entry.recorded_at);
        println!("{:<20} {:<14} {}", entry.host_name, colored, timestamp_str);
    }

    Ok(())
}

/// Converts Unix epoch seconds into a formatted UTC timestamp (`YYYY-MM-DD HH:MM:SS UTC`).
fn format_epoch_utc(secs: u64) -> String {
    let s = secs % 60;
    let m = (secs / 60) % 60;
    let h = (secs / 3600) % 24;
    let mut d = secs / 86400;

    let mut year = 1970;
    loop {
        let leap = if (year % 4 == 0 && year % 100 != 0) || year % 400 == 0 {
            1
        } else {
            0
        };
        let days_in_year = 365 + leap;
        if d < days_in_year {
            let month_days = [31, 28 + leap, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31];
            for (month, &md) in (1..).zip(month_days.iter()) {
                if d < md {
                    let day = d + 1;
                    return format!("{year:04}-{month:02}-{day:02} {h:02}:{m:02}:{s:02} UTC");
                }
                d -= md;
            }
            break;
        }
        d -= days_in_year;
        year += 1;
    }

    format!("{secs}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn format_epoch_utc_formats_known_timestamps() {
        assert_eq!(format_epoch_utc(0), "1970-01-01 00:00:00 UTC");
        assert_eq!(format_epoch_utc(1787600664), "2026-08-24 19:44:24 UTC");
    }
}
