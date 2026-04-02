use std::time::{SystemTime, UNIX_EPOCH};

fn main() {
    println!("cargo:rerun-if-changed=build.rs");

    let build_version =
        build_version_utc(SystemTime::now()).unwrap_or_else(|| "unknown".to_owned());
    println!("cargo:rustc-env=BUILD_VERSION={build_version}");
}

fn build_version_utc(now: SystemTime) -> Option<String> {
    let unix_seconds = now.duration_since(UNIX_EPOCH).ok()?.as_secs() as i64;
    let days_since_unix_epoch = unix_seconds.div_euclid(86_400);
    let (year, month, day) = civil_from_days(days_since_unix_epoch);

    Some(format!("{year:04}{month:02}{day:02}"))
}

fn civil_from_days(days_since_unix_epoch: i64) -> (i32, u32, u32) {
    let z = days_since_unix_epoch + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let day_of_era = z - era * 146_097;
    let year_of_era =
        (day_of_era - day_of_era / 1_460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let year = year_of_era + era * 400;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let month_param = (5 * day_of_year + 2) / 153;
    let day = day_of_year - (153 * month_param + 2) / 5 + 1;
    let month = month_param + if month_param < 10 { 3 } else { -9 };
    let adjusted_year = year + if month <= 2 { 1 } else { 0 };

    (adjusted_year as i32, month as u32, day as u32)
}
