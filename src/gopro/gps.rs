// SPDX-License-Identifier: MIT OR Apache-2.0

use crate::tags_impl::*;

fn scalar(v: &Scalar) -> f64 {
    match v {
        Scalar::i8(v) => *v as f64, Scalar::u8(v) => *v as f64,
        Scalar::i16(v) => *v as f64, Scalar::u16(v) => *v as f64,
        Scalar::i32(v) => *v as f64, Scalar::u32(v) => *v as f64,
        Scalar::i64(v) => *v as f64, Scalar::u64(v) => *v as f64,
        Scalar::f32(v) => *v as f64, Scalar::f64(v) => *v,
        _ => f64::NAN,
    }
}

fn scales(value: &TagValue) -> Vec<f64> {
    macro_rules! numeric {
        ($($single:ident, $list:ident),*) => {
            match value {
                $(TagValue::$single(v) => vec![*v.get() as f64],
                  TagValue::$list(v) => v.get().iter().map(|v| *v as f64).collect(),)*
                TagValue::Vec_Scalar(v) => v.get().iter().map(scalar).collect(),
                _ => Vec::new(),
            }
        };
    }
    numeric!(i8, Vec_i8, u8, Vec_u8, i16, Vec_i16, u16, Vec_u16,
             i32, Vec_i32, u32, Vec_u32, f32, Vec_f32, f64, Vec_f64)
}

/// Replace only GPS5/9 Data, leaving the other streams and their clocks alone.
/// Missing GPS5 UTC uses the existing numeric timestamp sentinel (0); it is
/// not evidence of lock. GPS9 keeps its row time even when no fix was acquired.
pub(super) fn normalize(map: &mut GroupedTagMap) {
    let Some(gps) = map.get_mut(&GroupId::GPS) else { return };
    let Some(desc) = gps.get(&TagId::Data) else { return };
    let gps9 = match desc.description.as_str() {
        "GPS9" => true,
        "GPS5" => false,
        _ => return,
    };
    let rows: Vec<Vec<f64>> = match &desc.value {
        TagValue::Vec_Vec_i32(v) => v.get().iter().map(|r| r.iter().map(|v| *v as f64).collect()).collect(),
        TagValue::Vec_Vec_Scalar(v) if gps9 => v.get().iter().map(|r| r.iter().map(scalar).collect()).collect(),
        TagValue::Vec_Scalar(v) if gps9 => vec![v.get().iter().map(scalar).collect()],
        _ => return, // Includes already-normalized data: idempotent.
    };
    let divisors = gps.get(&TagId::Scale).map(|d| scales(&d.value)).unwrap_or_default();
    let defaults = [1e7, 1e7, 1e3, 1e3, 1e3, 1.0, 1e3, 1e2, 1.0];
    let scale = |col: usize| {
        let v = if divisors.len() == 1 { divisors.first() } else { divisors.get(col) };
        v.copied().filter(|v| v.is_finite() && *v > 0.0).unwrap_or(defaults[col])
    };
    let utc: Option<&u64> = gps.get_t(TagId::Unknown(u32::from_be_bytes(*b"GPSU")));
    let frame_fix: Option<&u32> = gps.get_t(TagId::Unknown(u32::from_be_bytes(*b"GPSF")));
    // Only the defined 2D/3D fix values indicate lock; unknown values do not.
    let locked = |fix: f64| fix == 2.0 || fix == 3.0;
    let frame_locked = frame_fix.is_some_and(|fix| locked(*fix as f64));
    let mut out = Vec::with_capacity(rows.len());
    for row in rows {
        if row.len() < if gps9 { 9 } else { 5 } { continue; }
        // Do not coerce malformed heterogeneous cells into real coordinates.
        if row.iter().any(|v| !v.is_finite()) { continue; }
        let timestamp = if gps9 {
            946_684_800.0 + row[5] / scale(5) * 86_400.0 + row[6] / scale(6)
        } else {
            utc.map(|v| *v as f64 / 1000.0).unwrap_or(0.0)
        };
        out.push(GpsData {
            // Fork compatibility: a frame lock can recover a zero row fix.
            // This is intentionally OR, not a statement of protocol precedence.
            is_acquired: frame_locked || (gps9 && locked(row[8] / scale(8))),
            unix_timestamp: timestamp,
            lat: row[0] / scale(0),
            lon: row[1] / scale(1),
            altitude: row[2] / scale(2),
            speed: row[3] / scale(3) * 3.6, // GPMF m/s -> GpsData km/h
            track: 0.0,
        });
    }
    // Preserve provenance (description/native_id) and all companion tags.
    gps.get_mut(&TagId::Data).unwrap().value = TagValue::Vec_GpsData(
        ValueType::new_parsed(|v| format!("{v:?}"), out, Vec::new()));
}

#[cfg(test)]
mod tests {
    use super::super::*;

    fn klv(key: &[u8; 4], ty: u8, size: u8, repeat: u16, data: &[u8]) -> Vec<u8> {
        let mut out = key.to_vec();
        out.extend([ty, size]);
        out.extend(repeat.to_be_bytes());
        out.extend(data);
        while out.len() % 4 != 0 { out.push(0); }
        out
    }
    fn ints(key: &[u8; 4], size: u8, repeat: u16, data: &[i32]) -> Vec<u8> {
        klv(key, b'l', size, repeat, &data.iter().flat_map(|v| v.to_be_bytes()).collect::<Vec<_>>())
    }
    fn wire(gps9: bool, custom: bool, rows: usize, fix: i32, frame_fix: Option<u32>, scale: bool, utc: bool) -> Vec<u8> {
        let mut body = Vec::new();
        if scale {
            let scales = if gps9 { vec![10000000, 10000000, 1000, 1000, 1000, 1, 1000, 100, 1] }
                         else { vec![10000000, 10000000, 1000, 1000, 1000] };
            body.extend(ints(b"SCAL", 4, scales.len() as u16, &scales));
        }
        if let Some(fix) = frame_fix { body.extend(klv(b"GPSF", b'L', 4, 1, &fix.to_be_bytes())); }
        if utc { body.extend(klv(b"GPSU", b'U', 16, 1, b"231114120000.000")); }
        if gps9 { body.extend(klv(b"TYPE", b'c', 1, 9, if custom { b"lllllLLSS" } else { b"lllllllll" })); }
        let mut bytes = Vec::new();
        for i in 0..rows {
            for v in [377749000i32, -1224194000, 10500, 5000, 5200] { bytes.extend(v.to_be_bytes()); }
            if gps9 {
                bytes.extend(8718u32.to_be_bytes());
                bytes.extend((44800000u32 + i as u32 * 100).to_be_bytes());
                if custom { bytes.extend(100u16.to_be_bytes()); bytes.extend((fix as u16).to_be_bytes()); }
                else { bytes.extend(100i32.to_be_bytes()); bytes.extend(fix.to_be_bytes()); }
            }
        }
        body.extend(klv(if gps9 { b"GPS9" } else { b"GPS5" }, if custom { b'?' } else { b'l' },
                        if custom { 32 } else if gps9 { 36 } else { 20 }, rows as u16, &bytes));
        klv(b"DEVC", 0, 1, (body.len() + 8) as u16, &klv(b"STRM", 0, 1, body.len() as u16, &body))
    }
    fn parse(data: &[u8]) -> Vec<SampleInfo> {
        let options = crate::InputOptions::default();
        let map = GoPro::parse_metadata_inner(data, GroupId::Default, false, &options).unwrap();
        vec![SampleInfo { tag_map: Some(map), ..Default::default() }]
    }
    fn normalize(samples: &mut Vec<SampleInfo>) {
        GoPro::default().process_samples(samples, None, &crate::InputOptions::default());
    }
    fn points(samples: &[SampleInfo]) -> &Vec<GpsData> {
        samples[0].tag_map.as_ref().unwrap().get(&GroupId::GPS).expect("GPS group")
            .get_t(TagId::Data).expect("canonical Vec_GpsData")
    }
    #[test]
    fn gps_filters_preserve_canonical_data_and_hide_dependencies() {
        use crate::{InputOptions, TagFilter};
        for gps9 in [false, true] {
            let mut body = ints(b"SCAL", 4, if gps9 { 9 } else { 5 },
                if gps9 { &[100, 100, 10, 10, 10, 2, 10, 10, 1] } else { &[100, 100, 10, 10, 10] });
            body.extend(klv(b"GPSU", b'U', 16, 1, b"231114120000.000"));
            body.extend(klv(b"GPSF", b'L', 4, 1, &3u32.to_be_bytes()));
            body.extend(ints(if gps9 { b"GPS9" } else { b"GPS5" }, if gps9 { 36 } else { 20 }, 1,
                if gps9 { &[1000, -2000, 30, 50, 60, 4, 100, 20, 0] } else { &[1000, -2000, 30, 50, 60] }));
            let data = klv(b"STRM", 0, 1, body.len() as u16, &body);
            let run = |options: &InputOptions| {
                let map = GoPro::parse_metadata(&data, GroupId::Default, false, options).unwrap();
                let mut samples = vec![SampleInfo { tag_map: Some(map), ..Default::default() }];
                GoPro::default().process_samples(&mut samples, None, options);
                samples
            };
            let default = run(&InputOptions::default());
            for whitelist in [true, false] {
                let mut options = InputOptions::default();
                if whitelist {
                    options.tag_whitelist.insert(TagFilter::SpecificTag(GroupId::GPS, TagId::Data));
                } else {
                    for id in [TagId::Scale, TagId::Unknown(u32::from_be_bytes(*b"GPSU")), TagId::Unknown(u32::from_be_bytes(*b"GPSF"))] {
                        options.tag_blacklist.insert(TagFilter::SpecificTag(GroupId::GPS, id));
                    }
                }
                let filtered = run(&options);
                assert_eq!(serde_json::to_value(points(&filtered)).unwrap(), serde_json::to_value(points(&default)).unwrap());
                assert_eq!(filtered[0].tag_map.as_ref().unwrap()[&GroupId::GPS].len(), 1);
                let p = &points(&filtered)[0];
                assert_eq!((p.lat, p.lon, p.altitude, p.speed), (10.0, -20.0, 3.0, 18.0));
                assert!(p.is_acquired);
                assert_eq!(p.unix_timestamp, if gps9 { 946684800.0 + 2.0 * 86400.0 + 10.0 } else { 1699963200.0 });
            }
            let mut options = InputOptions::default();
            options.tag_blacklist.insert(TagFilter::EntireTag(TagId::Data));
            let excluded = run(&options);
            assert!(!excluded[0].tag_map.as_ref().unwrap()[&GroupId::GPS].contains_key(&TagId::Data));
            let raw = GoPro::parse_metadata(&data, GroupId::Default, true, &InputOptions::default()).unwrap();
            assert!(matches!(raw[&GroupId::Default][&TagId::Data].value, TagValue::Vec_Vec_i32(_)));
        }
    }
    #[test]
    fn gps_wire_mapping_and_dispatch() {
        for custom in [false, true] {
            for rows in [1, 3] {
                let mut samples = parse(&wire(true, custom, rows, 3, None, true, false));
                let desc = &samples[0].tag_map.as_ref().unwrap()[&GroupId::GPS][&TagId::Data];
                assert_eq!(desc.description, "GPS9");
                assert_eq!(desc.native_id, Some(u32::from_be_bytes(*b"GPS9")));
                if custom {
                    assert!(if rows == 1 { matches!(desc.value, TagValue::Vec_Scalar(_)) }
                            else { matches!(desc.value, TagValue::Vec_Vec_Scalar(_)) });
                } else { assert!(matches!(desc.value, TagValue::Vec_Vec_i32(_))); }
                normalize(&mut samples);
                let out = points(&samples);
                assert_eq!(out.len(), rows);
                assert!((out[0].lat - 37.7749).abs() < 1e-8);
                assert!((out[0].lon + 122.4194).abs() < 1e-8);
                assert_eq!(out[0].altitude, 10.5);
                assert_eq!(out[0].speed, 18.0); // 5 m/s -> canonical km/h
                assert_eq!(out[0].unix_timestamp, 1699964800.0);
                assert!(out[0].is_acquired);
                if rows > 1 { assert!((out[1].unix_timestamp - out[0].unix_timestamp - 0.1).abs() < 1e-6); }
                let before = serde_json::to_value(out).unwrap();
                normalize(&mut samples);
                assert_eq!(serde_json::to_value(points(&samples)).unwrap(), before);
            }
        }
    }
    #[test]
    fn gps5_time_fix_and_default_scales() {
        for scale in [false, true] {
            for (fix, utc, acquired) in [(Some(3), true, true), (None, true, false), (None, false, false), (Some(0), true, false), (Some(2), false, true)] {
                let mut samples = parse(&wire(false, false, 2, 0, fix, scale, utc));
                normalize(&mut samples);
                let out = points(&samples);
                assert_eq!(out.len(), 2);
                assert_eq!(out[0].is_acquired, acquired);
                assert_eq!(out[0].unix_timestamp, if utc { 1699963200.0 } else { 0.0 });
                assert_eq!(out[1].unix_timestamp, out[0].unix_timestamp); // no invented subpacket time
                assert_eq!(out[0].speed, 18.0);
            }
        }
    }
    #[test]
    fn gps9_frame_fix_compatibility_is_bounded() {
        for (row, frame, acquired) in [(0, None, false), (0, Some(3), true), (3, Some(0), true), (1, Some(1), false), (99, Some(99), false)] {
            let mut samples = parse(&wire(true, false, 1, row, frame, false, false));
            normalize(&mut samples);
            assert_eq!(points(&samples)[0].is_acquired, acquired);
        }
    }
    #[test]
    fn gps_nondefault_scaling() {
        let mut body = ints(b"SCAL", 4, 9, &[100, 100, 10, 10, 10, 2, 10, 10, 2]);
        body.extend(ints(b"GPS9", 36, 1, &[1000, -2000, 30, 50, 60, 4, 100, 20, 6]));
        let mut samples = parse(&klv(b"STRM", 0, 1, body.len() as u16, &body));
        normalize(&mut samples);
        let p = &points(&samples)[0];
        assert_eq!((p.lat, p.lon, p.altitude, p.speed), (10.0, -20.0, 3.0, 18.0));
        assert_eq!(p.unix_timestamp, 946684800.0 + 2.0 * 86400.0 + 10.0);
        assert!(p.is_acquired);
    }
    #[test]
    fn gps_custom_floats_and_invalid_scale_fallback() {
        let mut body = ints(b"SCAL", 4, 9, &[0, -1, 0, 0, 0, 0, 0, 0, 0]);
        body.extend(klv(b"TYPE", b'c', 1, 9, b"lllllLfSS"));
        let mut row = Vec::new();
        for v in [377749000i32, -1224194000, 10500, 0, 0, 8718] { row.extend(v.to_be_bytes()); }
        row.extend(1250.5f32.to_be_bytes());
        row.extend(100u16.to_be_bytes());
        row.extend(0u16.to_be_bytes());
        body.extend(klv(b"GPS9", b'?', 32, 1, &row));
        let mut samples = parse(&klv(b"STRM", 0, 1, body.len() as u16, &body));
        normalize(&mut samples);
        let p = &points(&samples)[0];
        assert!((p.unix_timestamp - (946684800.0 + 8718.0 * 86400.0 + 1.2505)).abs() < 1e-6);
        assert_eq!(p.speed, 0.0);
        assert!(!p.is_acquired);
        assert!((p.lat - 37.7749).abs() < 1e-8);
    }
    #[test]
    fn gps_raw_parse_runs_normalization() {
        let data = wire(true, true, 1, 3, None, true, false);
        let options = crate::InputOptions::default();
        let mut parser = GoPro::detect(&data, "test.gpmf", &options).unwrap();
        let samples = parser.parse(&mut Cursor::new(&data), data.len() as u64, |_| {},
            Arc::new(AtomicBool::new(false)), options).unwrap();
        // Detection retains a force-grouped Default metadata sample first.
        assert!(samples.iter().skip(1).all(|sample| {
            let gps = &sample.tag_map.as_ref().unwrap()[&GroupId::GPS];
            let out: &Vec<GpsData> = gps.get_t(TagId::Data).unwrap();
            out.len() == 1 && out[0].speed == 18.0
        }));
        assert_eq!(samples.len(), 2);
    }
    #[test]
    fn gps_noop_unknown_short_and_missing_data() {
        for data in [vec![], vec![1, 2, 3], klv(b"STRM", 0, 1, 12, &klv(b"GPS9", b'x', 1, 1, &[0]))] {
            let mut samples = parse(&data);
            let before = format!("{:?}", samples[0].tag_map);
            normalize(&mut samples);
            assert_eq!(format!("{:?}", samples[0].tag_map), before);
        }
        let short = ints(b"GPS9", 20, 1, &[1, 2, 3, 4, 5]);
        let mut samples = parse(&klv(b"STRM", 0, 1, short.len() as u16, &short));
        normalize(&mut samples);
        assert!(points(&samples).is_empty());
        let mut samples = vec![SampleInfo::default()];
        normalize(&mut samples);
        assert!(samples[0].tag_map.is_none());
        assert!(GoPro::parse_metadata(b"GPS9l$\0\x01", GroupId::GPS, false, &crate::InputOptions::default()).is_err());
    }
}
