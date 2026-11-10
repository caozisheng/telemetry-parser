// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright © 2024 Tomi Leppikangas

use argh::FromArgs;
use chrono::{ SecondsFormat, TimeZone, Utc };
use std::sync::{ atomic::AtomicBool, Arc };
use telemetry_parser::tags_impl::*;
use telemetry_parser::*;

/**
 * dump GPS data from GoPro mp4 file
 */
#[derive(FromArgs)]
struct Opts {
    /// csv output
    #[argh(switch, short = 'c')]
    csv: bool,
    /// kml output
    #[argh(switch, short = 'k')]
    kml: bool,
    /// gpx output
    #[argh(switch, short = 'g')]
    gpx: bool,
    ///
    /// input file
    #[argh(positional)]
    file: String,
}

// Canonical GPS has 2D speed only. Keep the CSV 3D column empty rather
// than inventing a value that the canonical representation cannot supply.
fn export_point(point: &GpsData, opts: &Opts) -> Option<String> {
    let utc = Utc.timestamp_millis_opt((point.unix_timestamp * 1000.0).round() as i64).single()?;
    let time = utc.to_rfc3339_opts(SecondsFormat::Millis, true);
    let speed_mps = point.speed / 3.6;
    if opts.csv {
        Some(format!("{},{},{},{},{},", time, point.lat, point.lon, point.altitude, speed_mps))
    } else if opts.kml {
        Some(format!("{},{},{}", point.lon, point.lat, point.altitude))
    } else if opts.gpx {
        Some(format!("<trkpt lat=\"{}\" lon=\"{}\">
  <ele>{}</ele>
  <time>{}</time>
  <speed>{}</speed>
</trkpt>",
            point.lat, point.lon, point.altitude, time, speed_mps))
    } else {
        None
    }
}

fn export_gps(map: &TagMap, opts: &Opts) -> Vec<String> {
    match map.get(&TagId::Data).map(|tag| &tag.value) {
        Some(TagValue::Vec_GpsData(points)) => points.get().iter().filter_map(|p| export_point(p, opts)).collect(),
        _ => Vec::new(),
    }
}

const KML_HEAD: &'static str = r#"<?xml version="1.0" encoding="UTF-8"?>
<kml xmlns="http://earth.google.com/kml/2.0">
<Document>
<Placemark>
 <LineString>
  <coordinates>"#;

const KML_END: &'static str = r#"  </coordinates>
 </LineString>
</Placemark>
</Document>
</kml>"#;

const GPX_HEAD: &'static str = r#"<?xml version="1.0" encoding="UTF-8"?>
<gpx  xmlns="http://www.topografix.com/GPX/1/1">"#;
const GPX_END: &str = r#"</gpx>"#;

fn main() {
    let opts: Opts = argh::from_env();

    let mut stream = std::fs::File::open(&opts.file).unwrap();
    let filesize = stream.metadata().unwrap().len();

    match opts {
        Opts { csv: true, .. } => {
            println!("UTC Time,Latitude,Longitude,Altitude,2D Speed,3D Speed");
        }
        Opts { kml: true, .. } => {
            println!("{}", KML_HEAD);
        }
        Opts { gpx: true, .. } => {
            println!("{}", GPX_HEAD);
        }
        _ => {
            eprintln!("Error: select kml, csv or gpx output");
            return;
        }
    }

    let input = Input::from_stream(
        &mut stream,
        filesize,
        &opts.file,
        |_| (),
        Arc::new(AtomicBool::new(false)),
    ).unwrap();
    // println!(
    //     "Detected camera: {} {}",
    //     input.camera_type(),
    //     input.camera_model().unwrap_or(&"".into())
    // );

    let samples = input.samples.as_ref().unwrap();

    for info in samples {
        if info.tag_map.is_none() {
            continue;
        }
        let grouped_tag_map = info.tag_map.as_ref().unwrap();

        if let Some(map) = grouped_tag_map.get(&GroupId::GPS) {
            for line in export_gps(map, &opts) { println!("{}", line); }
        }
    }

    match opts {
        Opts { csv: true, .. } => { }
        Opts { kml: true, .. } => { println!("{}", KML_END); }
        Opts { gpx: true, .. } => { println!("{}", GPX_END); }
        _ => { }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn klv(key: &[u8; 4], ty: u8, size: u8, repeat: u16, data: &[u8]) -> Vec<u8> {
        let mut out = key.to_vec();
        out.extend([ty, size]);
        out.extend(repeat.to_be_bytes());
        out.extend(data);
        while out.len() % 4 != 0 { out.push(0); }
        out
    }

    #[test]
    fn exports_parsed_gps5_and_gps9_canonical_points() {
        for gps9 in [false, true] {
            let mut body = klv(b"GPSU", b'U', 16, 1, b"231114120000.000");
            let mut rows = Vec::new();
            for _ in 0..2 {
                let row: &[i32] = if gps9 { &[10000000, -20000000, 3000, 5000, 6000, 8718, 43200000, 100, 3] }
                                 else { &[10000000, -20000000, 3000, 5000, 6000] };
                rows.extend(row.iter().flat_map(|v| v.to_be_bytes()));
            }
            body.extend(klv(if gps9 { b"GPS9" } else { b"GPS5" }, b'l', if gps9 { 36 } else { 20 }, 2, &rows));
            let stream = klv(b"STRM", 0, 1, body.len() as u16, &body);
            let data = klv(b"DEVC", 0, 1, stream.len() as u16, &stream);
            let input = Input::from_stream(&mut std::io::Cursor::new(&data), data.len() as u64,
                "test.gpmf", |_| (), Arc::new(AtomicBool::new(false))).unwrap();
            let opts = Opts { csv: true, kml: false, gpx: false, file: String::new() };
            let maps: Vec<_> = input.samples.as_ref().unwrap().iter().filter_map(|s| s.tag_map.as_ref()?.get(&GroupId::GPS)).collect();
            assert_eq!(maps.len(), 1);
            let lines = export_gps(maps[0], &opts);
            assert_eq!(lines, vec!["2023-11-14T12:00:00.000Z,1,-2,3,5,"; 2]);
            let gpx = export_gps(maps[0], &Opts { csv: false, gpx: true, ..opts });
            assert_eq!(gpx.len(), 2);
            assert!(gpx[0].contains("<speed>5</speed>"));
            assert!(gpx[0].contains("<time>2023-11-14T12:00:00.000Z</time>"));
            let kml = export_gps(maps[0], &Opts { csv: false, gpx: false, kml: true, file: String::new() });
            assert_eq!(kml, vec!["-2,1,3"; 2]);
        }
    }
}
