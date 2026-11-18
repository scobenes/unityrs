//! Verify the clocks against the loaded clip and independently observed probes.
#![allow(clippy::cast_possible_truncation)]
use crate::{
    Arc, BoundClip, J, Meter, Result, Target, f32_bits, index, list, number, require, string,
};

#[allow(clippy::too_many_lines)]
pub(crate) fn check(
    spec: &J,
    capture: &J,
    tolerance: &J,
    bound: &BoundClip<'_, '_>,
    targets: &[Target<'_, '_, '_>],
    meter: &Arc<Meter>,
) -> Result<Vec<f64>> {
    let timing = bound.timing().ok_or("missing clip interval")?;
    let start = timing.start();
    let stop = timing.stop();
    require(
        number(&spec["start"])? == start
            && number(&spec["stop"])? == stop
            && number(&spec["sample_rate"])? == timing.rate(),
        "bundle source origin and interval",
    )?;
    let evidence = &spec["evidence"]["clock"];
    let (start_field, stop_field) = bound.clip().endpoint_fields(meter)?;
    require(
        evidence["start"]["field"] == start_field
            && evidence["stop"]["field"] == stop_field
            && number(&evidence["start"]["source"])? == start
            && number(&evidence["stop"]["source"])? == stop,
        "bundle endpoint fields and values",
    )?;

    let mut inventory = vec![start, stop];
    bound.clip().visit_key_times(meter, |time| {
        meter.count_decode(1)?;
        if (start..=stop).contains(&time) {
            inventory.push(time);
        }
        Ok(())
    })?;
    inventory.sort_unstable_by(f64::total_cmp);
    inventory.dedup();
    let recorded = list(&evidence["key_times"])?
        .iter()
        .map(number)
        .collect::<Result<Vec<_>>>()?;
    require(inventory == recorded, "bundle key-time inventory")?;

    let mut boundaries = Vec::new();
    for track in bound.tracks() {
        track.visit_key_times(|time| {
            if start <= time && time <= stop {
                let left = track.raw_at(time, true, meter)?;
                let right = track.raw_at(time, false, meter)?;
                for (axis, (&a, &b)) in left.iter().zip(&right).enumerate().take(track.width()) {
                    if track.segment(axis, time, true, meter)?.is_held() && a != b {
                        boundaries.push(time);
                    }
                }
            }
            Ok(())
        })?;
    }
    boundaries.sort_unstable_by(f64::total_cmp);
    boundaries.dedup();
    let recorded = list(&spec["held_boundaries"])?
        .iter()
        .map(number)
        .collect::<Result<Vec<_>>>()?;
    require(recorded == boundaries, "bundle held boundaries")?;
    let round = |value| f64::from(value as f32);
    let length = round(stop - start);
    let f32_add = spec["source_clock"] == "f32_add";
    if f32_add {
        require(round(start) == start, "bundle f32 source origin")?;
    }
    let local_keys = inventory
        .iter()
        .map(|t| round(t - start))
        .collect::<Vec<_>>();
    let mut effective_keys = inventory
        .iter()
        .map(|&t| if f32_add { round(t) } else { t })
        .collect::<Vec<_>>();
    effective_keys.extend([start, stop]);
    effective_keys.sort_unstable_by(f64::total_cmp);
    let effective_clock = |rounded, f32_add| {
        if rounded == length {
            stop
        } else {
            let time = start + rounded;
            (if f32_add { round(time) } else { time }).clamp(start, stop)
        }
    };
    let off_key = |keys: &[f64], time| {
        let i = keys.partition_point(|&key| key < time);
        i != 0 && keys.get(i).is_some_and(|&key| time < key)
    };
    let samples = list(&capture["samples"])?;
    let schedule = list(&spec["schedule"])?;
    let frozen = list(&tolerance["samples"])?;
    require(
        samples.len() == schedule.len() && samples.len() == frozen.len() && !samples.is_empty(),
        "replay schedule coverage",
    )?;
    require(
        number(&schedule[0]["local"])? == 0.
            && number(&schedule[schedule.len() - 1]["local"])? == stop - start,
        "replay endpoint schedule",
    )?;
    let mut times = Vec::new();
    let mut alternative = Vec::new();
    let mut has_off_key = false;
    for ((point, observed), frozen) in schedule.iter().zip(samples).zip(frozen) {
        let local = number(&point["local"])?;
        let rounded = round(local);
        let source = if local == stop - start {
            stop
        } else {
            start + local
        };
        let effective = effective_clock(rounded, f32_add);
        let other = effective_clock(rounded, !f32_add);
        require(
            f32_bits(&observed["local_f32"])? == rounded
                && number(&observed["source"])? == source
                && number(&observed["effective_source"])? == effective
                && number(&frozen["source"])? == source
                && number(&frozen["effective_source"])? == effective
                && frozen["sides"] == observed["sides"],
            "bundle sample clocks",
        )?;
        let sides = list(&observed["sides"])?;
        require(
            sides.len() == boundaries.len()
                && sides
                    .iter()
                    .zip(&boundaries)
                    .all(|(side, &key)| side == if effective < key { "left" } else { "right" }),
            "bundle held sample sides",
        )?;
        has_off_key |= off_key(&inventory, source)
            && off_key(&local_keys, rounded)
            && off_key(&effective_keys, effective);
        times.push(effective);
        alternative.push(other);
    }
    require(
        stop == start || has_off_key,
        "bundle interior off-key probe",
    )?;
    require(
        times.windows(2).all(|w| w[0] < w[1]),
        "non-increasing effective clock",
    )?;
    let mut distinguished = false;
    for (&boundary, row) in boundaries.iter().zip(list(&evidence["held"])?) {
        require(number(&row["source"])? == boundary, "held probe boundary")?;
        let left = &row["left"];
        let right = &row["right"];
        let target_index = targets
            .iter()
            .position(|t| {
                t.id[0] == right["property"]
                    && t.id[1] == right["target"]
                    && t.id[2] == right["channel"]
            })
            .ok_or("held probe target")?;
        let target = &targets[target_index];
        let component = index(&right["component"])?;
        let held = held_input(target, component, boundary, meter)?;
        let left_value = target.component(component, boundary, true, meter)?;
        let right_value = target.component(component, boundary, false, meter)?;
        require(
            held && left_value != right_value,
            "held probe does not distinguish bundle sides",
        )?;
        let split = times.partition_point(|&time| time < boundary);
        require(
            index(&right["sample"])? == split,
            "held right probe sample index",
        )?;
        if boundary == start {
            require(
                left.is_null() && split == 0,
                "held start probe sample indices",
            )?;
        } else {
            require(
                split != 0 && index(&left["sample"])? == split - 1,
                "held left probe sample index",
            )?;
        }
        for (probe, is_left) in [(left, true), (right, false)] {
            if probe.is_null() {
                continue;
            }
            require(
                string(&probe["property"])? == target.property()
                    && probe["target"] == target.id[1]
                    && probe["channel"] == target.id[2]
                    && index(&probe["component"])? == component,
                "held probe component",
            )?;
            let sample = index(&probe["sample"])?;
            let observed = f32_bits(&probe["value"])?;
            let budget = number(&frozen[sample]["bounds"][target_index])?;
            let predicted = target.component(component, times[sample], false, meter)?;
            require(
                (predicted - observed).abs() <= budget,
                "held probe disagrees with bundle",
            )?;
            if round(start) == start && (alternative[sample] < boundary) != is_left {
                let alternate = target.component(component, alternative[sample], false, meter)?;
                distinguished |= (alternate - observed).abs() > budget;
            }
        }
    }
    require(
        distinguished && tolerance["clock_discriminated"] == true,
        "bundle held probe did not distinguish clocks",
    )?;
    Ok(times)
}

fn held_input(
    target: &Target<'_, '_, '_>,
    component: usize,
    boundary: f64,
    meter: &Meter,
) -> Result<bool> {
    if target.property() == "rotation" {
        for track in &target.tracks {
            let a = track.raw_at(boundary, true, meter)?;
            let b = track.raw_at(boundary, false, meter)?;
            for (axis, (left, right)) in a.into_iter().zip(b).enumerate().take(track.width()) {
                if left != right && track.segment(axis, boundary, true, meter)?.is_held() {
                    return Ok(true);
                }
            }
        }
        return Ok(false);
    }
    let (track, axis) = target
        .track(component)
        .ok_or("held probe has no bundle track")?;
    Ok(track.segment(axis, boundary, true, meter)?.is_held())
}
