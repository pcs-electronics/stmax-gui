use std::fmt;

pub const DEVICE_BAUD_RATE: u32 = 115_200;
pub const QUIET_TIMEOUT_MS: u64 = 350;
pub const STARTUP_DELAY_MS: u64 = 200;

const POWER_MIN: f64 = 0.0;
const POWER_MAX: f64 = 100.0;
const FREQUENCY_MIN_MHZ: f64 = 87.5;
const FREQUENCY_MAX_MHZ: f64 = 108.0;
const ALARM_TEMP_MIN_C: f64 = 40.0;
const ALARM_TEMP_MAX_C: f64 = 100.0;
const RDS_PS_MAX_BYTES: usize = 8;
const RDS_RT_MAX_BYTES: usize = 64;
const RDS_AFS_MAX_COUNT: usize = 25;
const RDS_AF_MIN_TENTHS: i32 = 876;
const RDS_AF_MAX_TENTHS: i32 = 1_079;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DeviceForm {
    pub power_percent: String,
    pub frequency_mhz: String,
    pub stereo_mode: bool,
    pub digital_audio_input: bool,
    pub audio_gain: u8,
    pub preemphasis_50us: bool,
    pub alarm_temp_c: String,
    pub rds_enabled: bool,
    pub rds_pi_hex: String,
    pub rds_ecc: String,
    pub rds_ps: String,
    pub rds_rt: String,
    pub rds_tp: bool,
    pub rds_ta: bool,
    pub rds_ms: bool,
    pub rds_di: String,
    pub rds_pty: String,
    pub rds_afs: String,
}

impl Default for DeviceForm {
    fn default() -> Self {
        Self {
            power_percent: "0".to_owned(),
            frequency_mhz: String::new(),
            stereo_mode: true,
            digital_audio_input: false,
            audio_gain: 1,
            preemphasis_50us: true,
            alarm_temp_c: "80".to_owned(),
            rds_enabled: false,
            rds_pi_hex: "0000".to_owned(),
            rds_ecc: "0".to_owned(),
            rds_ps: String::new(),
            rds_rt: String::new(),
            rds_tp: false,
            rds_ta: false,
            rds_ms: false,
            rds_di: "1".to_owned(),
            rds_pty: "0".to_owned(),
            rds_afs: String::new(),
        }
    }
}

impl DeviceForm {
    pub fn from_info_response(response: &str) -> Result<Self, String> {
        let block = current_settings_block(response).ok_or_else(|| {
            "device response did not include a `Current settings:` block".to_owned()
        })?;
        let mut form = DeviceForm::default();

        for raw_line in block.lines() {
            let line = raw_line.trim();
            if line.is_empty() || line == "Current settings:" {
                continue;
            }

            if let Some(value) = line.strip_prefix("Power: ") {
                form.power_percent = normalize_percent(value)?;
                continue;
            }

            if let Some(value) = line.strip_prefix("Frequency: ") {
                form.frequency_mhz = normalize_frequency(value)?;
                continue;
            }

            if line.starts_with("Stereo: ") {
                parse_audio_line(line, &mut form)?;
                continue;
            }

            if let Some(value) = line.strip_prefix("Alarm temp: ") {
                form.alarm_temp_c = normalize_alarm_temp(value)?;
                continue;
            }

            if line.starts_with("RDS: ") {
                parse_rds_flags_line(line, &mut form)?;
                continue;
            }

            if line.starts_with("PI: ") {
                parse_pi_line(line, &mut form)?;
                continue;
            }

            if let Some(value) = line.strip_prefix("RT: ") {
                form.rds_rt = value.to_owned();
                continue;
            }

            if let Some(value) = line.strip_prefix("AFs: ") {
                form.rds_afs = normalize_afs_line(value);
            }
        }

        Ok(form)
    }

    pub fn build_save_commands(&self) -> Result<Vec<String>, String> {
        let power = parse_f64_range(&self.power_percent, "Power", POWER_MIN, POWER_MAX)?;
        let alarm_temp = parse_f64_range(
            &self.alarm_temp_c,
            "Alarm temperature",
            ALARM_TEMP_MIN_C,
            ALARM_TEMP_MAX_C,
        )?;
        let rds_pi = parse_hex_u16(&self.rds_pi_hex)?;
        let rds_ecc = parse_u8_range(&self.rds_ecc, "RDS ECC", 0, 255)?;
        let rds_di = parse_u8_range(&self.rds_di, "RDS DI", 0, 15)?;
        let rds_pty = parse_u8_range(&self.rds_pty, "RDS PTY", 0, 31)?;
        let rds_ps = validate_text(&self.rds_ps, "RDS PS", RDS_PS_MAX_BYTES)?;
        let rds_rt = validate_text(&self.rds_rt, "RDS RT", RDS_RT_MAX_BYTES)?;
        let afs_payload = build_afs_payload(&self.rds_afs)?;

        let mut commands = Vec::new();
        commands.push(format!("config-power:{}", format_decimal(power)));

        let frequency = self.frequency_mhz.trim();
        if !frequency.is_empty() {
            let frequency_value =
                parse_f64_range(frequency, "Frequency", FREQUENCY_MIN_MHZ, FREQUENCY_MAX_MHZ)?;
            commands.push(format!("config-fq:{}", format_decimal(frequency_value)));
        }

        commands.push(format!(
            "config-audio-stereo:{}",
            bool_digit(self.stereo_mode)
        ));
        commands.push(format!(
            "config-audio-input:{}",
            bool_digit(self.digital_audio_input)
        ));
        commands.push(format!(
            "config-audio-gain:{}",
            match self.audio_gain {
                0..=2 => self.audio_gain,
                other => {
                    return Err(format!("Audio gain must be 0, 1, or 2, got {other}"));
                }
            }
        ));
        commands.push(format!(
            "config-audio-pre:{}",
            bool_digit(self.preemphasis_50us)
        ));
        commands.push(format!("config-alarm-temp:{}", format_decimal(alarm_temp)));

        commands.push(format!("config-rds:{}", bool_digit(self.rds_enabled)));
        commands.push(format!("config-rds-pi:{rds_pi:04X}"));
        commands.push(format!("config-rds-ecc:{rds_ecc}"));
        commands.push(format!("config-rds-ps:{rds_ps}"));
        commands.push(format!("config-rds-rt:{rds_rt}"));
        commands.push(format!("config-rds-tp:{}", bool_digit(self.rds_tp)));
        commands.push(format!("config-rds-ta:{}", bool_digit(self.rds_ta)));
        commands.push(format!("config-rds-ms:{}", bool_digit(self.rds_ms)));
        commands.push(format!("config-rds-di:{rds_di}"));
        commands.push(format!("config-rds-pty:{rds_pty}"));
        commands.push(format!("config-rds-afs:{afs_payload}"));
        commands.push("config-save".to_owned());

        Ok(commands)
    }
}

pub fn current_settings_block(response: &str) -> Option<&str> {
    let marker = "Current settings:";
    response.find(marker).map(|index| &response[index..])
}

pub fn normalize_response_for_display(response: &str) -> String {
    current_settings_block(response)
        .unwrap_or(response)
        .trim()
        .to_owned()
}

pub fn first_nonempty_line(response: &str) -> Option<&str> {
    response
        .lines()
        .map(str::trim)
        .find(|line| !line.is_empty())
}

pub fn response_is_err(response: &str) -> bool {
    matches!(first_nonempty_line(response), Some("ERR"))
}

fn parse_audio_line(line: &str, form: &mut DeviceForm) -> Result<(), String> {
    let stereo = segment_between(line, "Stereo: ", ", Input: ")?;
    let input = segment_between(line, ", Input: ", ", Gain: ")?;
    let gain = segment_between(line, ", Gain: ", ", Preemphasis: ")?;
    let preemphasis = segment_after(line, ", Preemphasis: ")?;

    form.stereo_mode = parse_bool_token(stereo, "Stereo")?;
    form.digital_audio_input = parse_bool_token(input, "Audio input")?;
    form.audio_gain = parse_u8_range(first_token(gain), "Audio gain", 0, 2)?;
    form.preemphasis_50us = parse_bool_token(preemphasis, "Preemphasis")?;

    Ok(())
}

fn parse_rds_flags_line(line: &str, form: &mut DeviceForm) -> Result<(), String> {
    let rds = segment_between(line, "RDS: ", ", TP: ")?;
    let tp = segment_between(line, ", TP: ", ", TA: ")?;
    let ta = segment_between(line, ", TA: ", ", MS: ")?;
    let ms = segment_between(line, ", MS: ", ", DI: ")?;
    let di = segment_between(line, ", DI: ", ", PTY: ")?;
    let pty = segment_after(line, ", PTY: ")?;

    form.rds_enabled = parse_bool_token(rds, "RDS")?;
    form.rds_tp = parse_bool_token(tp, "RDS TP")?;
    form.rds_ta = parse_bool_token(ta, "RDS TA")?;
    form.rds_ms = parse_bool_token(ms, "RDS MS")?;
    form.rds_di = parse_u8_range(first_token(di), "RDS DI", 0, 15)?.to_string();
    form.rds_pty = parse_u8_range(first_token(pty), "RDS PTY", 0, 31)?.to_string();

    Ok(())
}

fn parse_pi_line(line: &str, form: &mut DeviceForm) -> Result<(), String> {
    let after_prefix = line
        .strip_prefix("PI: ")
        .ok_or_else(|| "device response is missing `PI: `".to_owned())?;
    let (pi, remainder) = after_prefix
        .split_once(", ECC: ")
        .ok_or_else(|| "device response is missing `, ECC: ` after `PI: `".to_owned())?;
    let (ecc, ps) = match remainder.split_once(", PS:") {
        Some((ecc, ps)) => (ecc, ps.trim_start()),
        None => (remainder, ""),
    };

    form.rds_pi_hex = format!("{:04X}", parse_hex_u16(first_token(pi))?);
    form.rds_ecc = parse_u8_range(first_token(ecc), "RDS ECC", 0, 255)?.to_string();
    form.rds_ps = ps.to_owned();

    Ok(())
}

fn normalize_percent(value: &str) -> Result<String, String> {
    let without_suffix = value.trim().trim_end_matches('%').trim();
    Ok(format_decimal(parse_f64_range(
        without_suffix,
        "Power",
        POWER_MIN,
        POWER_MAX,
    )?))
}

fn normalize_frequency(value: &str) -> Result<String, String> {
    if value.trim().eq_ignore_ascii_case("not set") {
        return Ok(String::new());
    }

    let numeric = value
        .split_once("MHz")
        .map(|(prefix, _)| prefix.trim())
        .unwrap_or_else(|| first_token(value));

    Ok(format_decimal(parse_f64_range(
        numeric,
        "Frequency",
        FREQUENCY_MIN_MHZ,
        FREQUENCY_MAX_MHZ,
    )?))
}

fn normalize_alarm_temp(value: &str) -> Result<String, String> {
    let numeric = value
        .strip_suffix('C')
        .map(str::trim)
        .unwrap_or_else(|| first_token(value));

    Ok(format_decimal(parse_f64_range(
        numeric,
        "Alarm temperature",
        ALARM_TEMP_MIN_C,
        ALARM_TEMP_MAX_C,
    )?))
}

fn normalize_afs_line(value: &str) -> String {
    if value.trim().eq_ignore_ascii_case("not set") {
        return String::new();
    }

    let without_unit = value
        .strip_suffix("MHz")
        .map(str::trim)
        .unwrap_or_else(|| value.trim());

    without_unit
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(", ")
}

fn build_afs_payload(raw: &str) -> Result<String, String> {
    let tokens = raw
        .split(|character: char| character.is_whitespace() || character == ',' || character == ';')
        .filter(|token| !token.trim().is_empty())
        .collect::<Vec<_>>();

    if tokens.is_empty() {
        return Ok(String::new());
    }

    if tokens.len() > RDS_AFS_MAX_COUNT {
        return Err(format!(
            "RDS AFs accepts at most {RDS_AFS_MAX_COUNT} entries, got {}",
            tokens.len()
        ));
    }

    let mut normalized = Vec::with_capacity(tokens.len());
    for token in tokens {
        let tenths = parse_af_tenths(token)?;
        normalized.push(format!("{:.1}", tenths as f64 / 10.0));
    }

    Ok(normalized.join(" "))
}

fn parse_af_tenths(raw: &str) -> Result<i32, String> {
    let value = raw.trim();
    let float_value = value
        .parse::<f64>()
        .map_err(|_| format!("RDS AF `{value}` is not a valid frequency"))?;
    let tenths = (float_value * 10.0).round() as i32;
    if ((tenths as f64) / 10.0 - float_value).abs() > 0.000_1 {
        return Err(format!(
            "RDS AF `{value}` must use 0.1 MHz steps between 87.6 and 107.9"
        ));
    }
    if !(RDS_AF_MIN_TENTHS..=RDS_AF_MAX_TENTHS).contains(&tenths) {
        return Err(format!(
            "RDS AF `{value}` must be between 87.6 and 107.9 MHz"
        ));
    }

    Ok(tenths)
}

fn validate_text(raw: &str, label: &str, max_bytes: usize) -> Result<String, String> {
    if raw.contains('\n') || raw.contains('\r') {
        return Err(format!("{label} cannot contain newline characters"));
    }
    let bytes = raw.as_bytes().len();
    if bytes > max_bytes {
        return Err(format!(
            "{label} must be at most {max_bytes} bytes, got {bytes}"
        ));
    }

    Ok(raw.to_owned())
}

fn parse_f64_range(raw: &str, label: &str, min_value: f64, max_value: f64) -> Result<f64, String> {
    let value = raw
        .trim()
        .parse::<f64>()
        .map_err(|_| format!("{label} must be numeric, got `{raw}`"))?;

    if !(min_value..=max_value).contains(&value) {
        return Err(format!(
            "{label} must be between {} and {}, got {}",
            PrettyFloat(min_value),
            PrettyFloat(max_value),
            PrettyFloat(value)
        ));
    }

    Ok(value)
}

fn parse_u8_range(raw: &str, label: &str, min_value: u8, max_value: u8) -> Result<u8, String> {
    let value = raw
        .trim()
        .parse::<u8>()
        .map_err(|_| format!("{label} must be an integer, got `{raw}`"))?;

    if !(min_value..=max_value).contains(&value) {
        return Err(format!(
            "{label} must be between {min_value} and {max_value}, got {value}"
        ));
    }

    Ok(value)
}

fn parse_hex_u16(raw: &str) -> Result<u16, String> {
    let trimmed = raw.trim().trim_start_matches("0x").trim_start_matches("0X");
    if trimmed.is_empty()
        || trimmed.len() > 4
        || !trimmed
            .chars()
            .all(|character| character.is_ascii_hexdigit())
    {
        return Err(format!(
            "RDS PI must be a 1 to 4 digit hexadecimal value, got `{raw}`"
        ));
    }

    u16::from_str_radix(trimmed, 16)
        .map_err(|_| format!("RDS PI must be a valid hexadecimal number, got `{raw}`"))
}

fn parse_bool_token(raw: &str, label: &str) -> Result<bool, String> {
    match parse_u8_range(first_token(raw), label, 0, 1)? {
        0 => Ok(false),
        1 => Ok(true),
        _ => unreachable!(),
    }
}

fn bool_digit(value: bool) -> u8 {
    if value {
        1
    } else {
        0
    }
}

fn format_decimal(value: f64) -> String {
    let mut text = format!("{value:.3}");
    while text.contains('.') && text.ends_with('0') {
        text.pop();
    }
    if text.ends_with('.') {
        text.pop();
    }
    text
}

fn segment_between<'a>(line: &'a str, prefix: &str, suffix: &str) -> Result<&'a str, String> {
    let after_prefix = line
        .find(prefix)
        .map(|index| &line[index + prefix.len()..])
        .ok_or_else(|| format!("device response is missing `{prefix}`"))?;
    let (segment, _) = after_prefix
        .split_once(suffix)
        .ok_or_else(|| format!("device response is missing `{suffix}` after `{prefix}`"))?;

    Ok(segment.trim())
}

fn segment_after<'a>(line: &'a str, prefix: &str) -> Result<&'a str, String> {
    line.find(prefix)
        .map(|index| line[index + prefix.len()..].trim())
        .ok_or_else(|| format!("device response is missing `{prefix}`"))
}

fn first_token(value: &str) -> &str {
    value.split_whitespace().next().unwrap_or("").trim()
}

struct PrettyFloat(f64);

impl fmt::Display for PrettyFloat {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&format_decimal(self.0))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_current_settings_block() {
        let response = "\
OK
Help:
config-power:n
Current settings:
Power: 55%
Frequency: 99.50 MHz
Stereo: 1 (stereo), Input: 0 (analog), Gain: 2, Preemphasis: 1 (50 uS)
Alarm temp: 82 C
RDS: 1 (on), TP: 0 (off), TA: 1 (on), MS: 1 (music), DI: 7, PTY: 10
PI: 1a2b (hex), ECC: 12, PS: STMAX
RT: Studio link active
AFs: 99.5 100.1 101.3 MHz
";

        let form = DeviceForm::from_info_response(response).expect("response should parse");
        assert_eq!(form.power_percent, "55");
        assert_eq!(form.frequency_mhz, "99.5");
        assert!(form.stereo_mode);
        assert!(!form.digital_audio_input);
        assert_eq!(form.audio_gain, 2);
        assert!(form.preemphasis_50us);
        assert_eq!(form.alarm_temp_c, "82");
        assert!(form.rds_enabled);
        assert_eq!(form.rds_pi_hex, "1A2B");
        assert_eq!(form.rds_ecc, "12");
        assert_eq!(form.rds_ps, "STMAX");
        assert_eq!(form.rds_rt, "Studio link active");
        assert_eq!(form.rds_afs, "99.5, 100.1, 101.3");
    }

    #[test]
    fn builds_save_command_sequence() {
        let form = DeviceForm {
            power_percent: "50".to_owned(),
            frequency_mhz: "99.5".to_owned(),
            stereo_mode: true,
            digital_audio_input: false,
            audio_gain: 1,
            preemphasis_50us: true,
            alarm_temp_c: "80".to_owned(),
            rds_enabled: true,
            rds_pi_hex: "1234".to_owned(),
            rds_ecc: "10".to_owned(),
            rds_ps: "STMAX".to_owned(),
            rds_rt: "Studio link active".to_owned(),
            rds_tp: true,
            rds_ta: false,
            rds_ms: true,
            rds_di: "1".to_owned(),
            rds_pty: "5".to_owned(),
            rds_afs: "99.5, 101.2 104.7".to_owned(),
        };

        let commands = form.build_save_commands().expect("form should validate");
        assert_eq!(
            commands.first().map(String::as_str),
            Some("config-power:50")
        );
        assert!(commands.contains(&"config-fq:99.5".to_owned()));
        assert!(commands.contains(&"config-rds-pi:1234".to_owned()));
        assert!(commands.contains(&"config-rds-afs:99.5 101.2 104.7".to_owned()));
        assert_eq!(commands.last().map(String::as_str), Some("config-save"));
    }

    #[test]
    fn rejects_invalid_af_step() {
        let error = build_afs_payload("99.55").expect_err("step should be rejected");
        assert!(error.contains("0.1 MHz"));
    }

    #[test]
    fn parses_empty_ps_without_trailing_space() {
        let response = "\
OK
Current settings:
Power: 100%
Frequency: 87.80 MHz
Stereo: 1 (stereo), Input: 0 (analog), Gain: 1, Preemphasis: 1 (50 uS)
Alarm temp: 80 C
RDS: 0 (off), TP: 0 (off), TA: 0 (off), MS: 0 (speech), DI: 1, PTY: 0
PI: 0 (hex), ECC: 0, PS:
RT:
AFs: not set
";

        let form = DeviceForm::from_info_response(response).expect("response should parse");
        assert_eq!(form.rds_pi_hex, "0000");
        assert_eq!(form.rds_ecc, "0");
        assert_eq!(form.rds_ps, "");
        assert_eq!(form.rds_rt, "");
    }
}
