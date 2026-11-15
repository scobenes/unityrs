//! Reusable decision-10 comparison for synthetic and future Editor WAVs.
//! Channel metrics are independent: a loud channel cannot hide a quiet one.
#[derive(Debug)]
pub struct Metric {
    pub snr_db: f64,
    pub max_lsb: i32,
}
pub fn compare(
    reference: &[u8],
    actual: &[u8],
    channels: usize,
    min_snr_db: f64,
    max_lsb: i32,
) -> Result<Vec<Metric>, String> {
    if channels == 0
        || reference.len() != actual.len()
        || !reference.len().is_multiple_of(channels * 2)
    {
        return Err("sample count or channel layout differs".into());
    }
    let mut signal = vec![0f64; channels];
    let mut noise = vec![0f64; channels];
    let mut peak = vec![0; channels];
    for (i, (r, a)) in reference
        .as_chunks::<2>()
        .0
        .iter()
        .zip(actual.as_chunks::<2>().0)
        .enumerate()
    {
        let c = i % channels;
        let r = i32::from(i16::from_le_bytes(*r));
        let a = i32::from(i16::from_le_bytes(*a));
        let d = r - a;
        signal[c] += f64::from(r).powi(2);
        noise[c] += f64::from(d).powi(2);
        peak[c] = peak[c].max(d.abs());
    }
    let mut metrics = Vec::new();
    for c in 0..channels {
        let snr_db = if noise[c] == 0.0 {
            f64::INFINITY
        } else if signal[c] == 0.0 {
            f64::NEG_INFINITY
        } else {
            10.0 * (signal[c] / noise[c]).log10()
        };
        if snr_db < min_snr_db || peak[c] > max_lsb {
            return Err(format!(
                "channel {c}: SNR {snr_db} dB (minimum {min_snr_db}), peak {} LSB (maximum {max_lsb})",
                peak[c]
            ));
        }
        metrics.push(Metric {
            snr_db,
            max_lsb: peak[c],
        });
    }
    Ok(metrics)
}
