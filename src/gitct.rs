//! git-ct — git commit with auto-appended timestamp.
//!
//! Usage: git-ct <message words...>
//! Creates commit: "<message> - 2026-03-09 14:30:22"

use std::process::{Command, exit};
use std::time::SystemTime;

fn main() {
    let msg: String = std::env::args().skip(1).collect::<Vec<_>>().join(" ");
    if msg.is_empty() {
        eprintln!("usage: git-ct <commit message>");
        exit(1);
    }

    let now = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .expect("time went backwards");
    let secs = now.as_secs();

    let time_of_day = secs % 86400;
    let hours = time_of_day / 3600;
    let minutes = (time_of_day % 3600) / 60;
    let seconds = time_of_day % 60;

    let days = secs / 86400;
    let (year, month, day) = epoch_days_to_ymd(days);

    let full_msg = format!(
        "{msg} - {year:04}-{month:02}-{day:02} {hours:02}:{minutes:02}:{seconds:02}"
    );

    let commit = Command::new("git").args(["commit", "-m", &full_msg]).status();
    if !commit.map_or(false, |s| s.success()) {
        exit(1);
    }
}

fn epoch_days_to_ymd(mut days: u64) -> (u64, u64, u64) {
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
