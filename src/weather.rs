//! blx-weather — fetch weather from wttr.in.
//!
//! Usage: blx-weather [city]
//! No arguments: uses IP geolocation.

use std::os::unix::process::CommandExt;
use std::process::Command;

fn main() {
    let city = std::env::args().nth(1).unwrap_or_default();
    let url = format!("https://wttr.in/{city}?format=3");

    let err = Command::new("curl").args(["-s", &url]).exec();
    eprintln!("blx-weather: failed to exec curl: {err}");
    std::process::exit(1);
}
