//! blx-backup — copy a file with a timestamped suffix.
//!
//! Usage: blx-backup <file>
//! Creates: <file>.backup.20260309_143022

use std::time::SystemTime;

fn main() {
    let path = match std::env::args().nth(1) {
        Some(p) => p,
        None => {
            eprintln!("usage: blx-backup <file>");
            std::process::exit(1);
        }
    };

    let now = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .expect("time went backwards");
    let secs = now.as_secs();

    // Manual UTC timestamp formatting (no chrono dependency)
    let days = secs / 86400;
    let time_of_day = secs % 86400;
    let hours = time_of_day / 3600;
    let minutes = (time_of_day % 3600) / 60;
    let seconds = time_of_day % 60;

    // Days since epoch to Y/M/D (simplified Gregorian)
    let (year, month, day) = epoch_days_to_ymd(days);

    let timestamp = format!(
        "{year:04}{month:02}{day:02}_{hours:02}{minutes:02}{seconds:02}"
    );
    let dest = format!("{path}.backup.{timestamp}");

    match std::fs::copy(&path, &dest) {
        Ok(_) => println!("{dest}"),
        Err(e) => {
            eprintln!("blx-backup: {e}");
            std::process::exit(1);
        }
    }
}

fn epoch_days_to_ymd(mut days: u64) -> (u64, u64, u64) {
    // Algorithm from http://howardhinnant.github.io/date_algorithms.html
    days += 719_468;
    let era = days / 146_097;
    let doe = days - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = if m <= 2 { y + 1 } else { y };
    (year, m, d)
}
