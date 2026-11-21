// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) 2026 Audigma Player Project

//! The player's PCM to DSD options as a small `key=value` text (one per line, from the settings
//! screen) turned into audigma's real [`Settings`]. The glue holds no audigma rule: every value is
//! checked and resolved by audigma, which also decides the DSD rate for `rate_family = source`.
//!
//! Keys: `multiplier` (`dsd64`, `dsd128`, `dsd256`), `family` (`44100`, `source`, `48000`), `ntf`
//! (`resampler.modulator_ntf`), `order` (`resampler.modulator_order`), `phase`, `apodization`
//! (`off`, `gentle` or a pass edge in Hz) and `preamp_db` (`gain.preamp_db`). A missing key keeps
//! audigma's default.

use audigma_dsp::config::TargetRateSpec;
use audigma_dsp::rates::PcmRateHz;
use audigma_settings::{ResolvedPipelinePlan, Settings};

/// Settings for `options` (see the module docs) as TOML.
fn toml(options: &str) -> Result<String, String> {
    let mut target = String::new();
    let mut resampler = String::new();
    let mut gain = String::new();
    let mut multiplier = None;
    for line in options.lines().map(str::trim).filter(|l| !l.is_empty()) {
        let (key, value) = line.split_once('=').ok_or(format!("bad option line {line:?}"))?;
        let (key, value) = (key.trim(), value.trim());
        if value.is_empty()
            || !value.bytes().all(|b| b.is_ascii_alphanumeric() || b"._-".contains(&b))
        {
            return Err(format!("bad value for {key}: {value:?}"));
        }
        // Digits only: a TOML integer (modulator order, apodization pass edge in Hz).
        let number = value.bytes().all(|b| b.is_ascii_digit());
        let quoted = format!("\"{value}\"");
        let toml_value = if number { value } else { &quoted };
        match key {
            "multiplier" => multiplier = Some(value.to_owned()),
            "family" => target += &format!("rate_family = {quoted}\n"),
            "ntf" => resampler += &format!("modulator_ntf = {quoted}\n"),
            "order" => resampler += &format!("modulator_order = {toml_value}\n"),
            "phase" => resampler += &format!("phase = {quoted}\n"),
            "apodization" => resampler += &format!("apodization = {toml_value}\n"),
            "preamp_db" if value.parse::<f64>().is_ok() => {
                gain += &format!("preamp_db = {value}\n");
            }
            _ => return Err(format!("unknown option {key}={value}")),
        }
    }
    let multiplier = multiplier.ok_or("missing option multiplier")?;
    Ok(format!(
        "schema_version = 1\n[target]\nformat = \"dsd\"\ndsd_rate = \"{multiplier}\"\n{target}\
         [resampler]\n{resampler}[gain]\n{gain}"
    ))
}

/// audigma's plan for `options` and `input_rate` Hz PCM.
pub fn plan(options: &str, input_rate: u32) -> Result<ResolvedPipelinePlan, String> {
    let pcm_rate = PcmRateHz::new(input_rate).ok_or(format!("unsupported PCM rate {input_rate}"))?;
    Settings::from_toml_str(&toml(options)?)
        .map_err(|e| e.to_string())?
        .explain_for_input_rate(pcm_rate)
        .map_err(|e| e.to_string())
}

/// The DSD bit rate in Hz audigma plays `input_rate` Hz PCM at under `options`.
pub fn dsd_rate(options: &str, input_rate: u32) -> Result<u32, String> {
    match plan(options, input_rate)?.stream_spec.target {
        TargetRateSpec::Dsd { bit_rate } => Ok(bit_rate.get()),
        other => Err(format!("audigma resolved {other:?}, not DSD")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const BASE: &str = "multiplier=dsd256\nfamily=44100\n";

    #[test]
    fn family_decides_the_rate() {
        assert_eq!(dsd_rate(BASE, 96_000), Ok(11_289_600));
        assert_eq!(dsd_rate("multiplier=dsd128\nfamily=48000", 44_100), Ok(6_144_000));
        assert_eq!(dsd_rate("multiplier=dsd64\nfamily=source", 44_100), Ok(2_822_400));
        assert_eq!(dsd_rate("multiplier=dsd64\nfamily=source", 96_000), Ok(3_072_000));
    }

    #[test]
    fn every_ui_value_is_accepted() {
        for ntf in ["reference", "comfortable", "fixed_h_inf_1_4", "fixed_h_inf_1_5"] {
            for order in ["5", "7"] {
                for phase in ["linear", "minimum"] {
                    for apod in ["off", "gentle", "18000", "18500", "20000"] {
                        let o = format!(
                            "{BASE}ntf={ntf}\norder={order}\nphase={phase}\napodization={apod}\n\
                             preamp_db=-3.5\n"
                        );
                        dsd_rate(&o, 44_100).unwrap_or_else(|e| panic!("{o}: {e}"));
                    }
                }
            }
        }
    }

    #[test]
    fn preamp_lowers_the_modulator_gain() {
        let gain = |o: &str| plan(o, 44_100).unwrap().dsp_config.total_configured_gain_db();
        let low = gain(&format!("{BASE}preamp_db=-3.5"));
        let flat = gain(BASE);
        assert!((flat - low - 3.5).abs() < 1e-6, "{flat} {low}");
    }

    #[test]
    fn rejects_garbage() {
        assert!(dsd_rate("multiplier=dsd999", 44_100).is_err());
        assert!(dsd_rate("multiplier=dsd64\nfoo=bar", 44_100).is_err());
        assert!(dsd_rate("multiplier=dsd64\nntf=\"x\ny", 44_100).is_err());
        assert!(dsd_rate("family=44100", 44_100).is_err());
    }
}
