// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright © 2021 Adrian <adrian.eddy at gmail>

pub mod klv;

mod lens_profile;

use std::io::*;
use std::sync::{ Arc, atomic::AtomicBool };
use byteorder::{ ReadBytesExt, BigEndian };

use crate::tags_impl::*;
use crate::*;
use klv::KLV;
use memchr::memmem;

#[derive(Default)]
pub struct GoPro {
    pub model: Option<String>,
    extra_gpmf: Option<GroupedTagMap>,
    frame_readout_time: Option<f64>,
    has_cori: bool,
    is_raw_gpmf: bool,
}

impl GoPro {
    pub fn camera_type(&self) -> String {
        "GoPro".to_owned()
    }
    pub fn has_accurate_timestamps(&self) -> bool {
        self.has_cori
    }
    pub fn possible_extensions() -> Vec<&'static str> {
        vec!["mp4", "mov", "360", "gpmf", "lrv"]
    }
    pub fn frame_readout_time(&self) -> Option<f64> {
        self.frame_readout_time
    }
    pub fn normalize_imu_orientation(v: String) -> String {
        v
    }

    pub fn detect<P: AsRef<std::path::Path>>(buffer: &[u8], _filepath: P, options: &crate::InputOptions) -> Option<Self> {
        let mut ret = None;

        if buffer.len() > 8 && &buffer[0..4] == b"DEVC" {
            if let Ok(map) = Self::parse_metadata(buffer, GroupId::Default, true, options) {
                let mut obj = Self::default();
                for v in map.values() {
                    if let Some(v) = v.get_t(TagId::Unknown(0x4D494E46/*MINF*/)) as Option<&String> {
                        obj.model = Some(v.clone());
                    }
                    if let Some(v) = v.get_t(TagId::Unknown(0x53524F54/*SROT*/)) as Option<&f32> {
                        obj.frame_readout_time = Some(*v as f64);
                    }
                    if obj.model.is_some() && obj.frame_readout_time.is_some() { break; }
                }
                obj.extra_gpmf = Some(map);
                obj.is_raw_gpmf = true;
                ret = Some(obj);
            }
        }

        if let Some(pos) = memmem::find(buffer, b"GPMFDEVC") {
            let mut obj = Self::default();
            let mut buf = &buffer[pos-4..];
            let len = buf.read_u32::<BigEndian>().unwrap() as usize;
            let gpmf_box = &buf[..len];

            if let Ok(map) = Self::parse_metadata(&gpmf_box[8+8..], GroupId::Default, true, &crate::InputOptions::default()) {
                for v in map.values() {
                    if let Some(v) = v.get_t(TagId::Unknown(0x4D494E46/*MINF*/)) as Option<&String> {
                        obj.model = Some(v.clone());
                    }
                    if let Some(v) = v.get_t(TagId::Unknown(0x53524F54/*SROT*/)) as Option<&f32> {
                        obj.frame_readout_time = Some(*v as f64);
                    }
                    if obj.model.is_some() && obj.frame_readout_time.is_some() { break; }
                }
                obj.extra_gpmf = Some(map);
            }
            ret = Some(obj);
        } else if memmem::find(buffer, b"GoPro MET").is_some() {
            ret = Some(Self::default());
        }

        if ret.is_none() || ret.as_ref().unwrap().model.is_none() {
            // Find model name in GPRO section in `mdat` at the beginning of the file
            if let Some(p1) = memmem::find(buffer, b"GPRO") {
                if let Some(model) = util::find_between_with_offset(&buffer[p1..(p1+1024).min(buffer.len())], b"HERO", b'\0', -4) {
                    if let Some(obj) = &mut ret {
                        obj.model = Some(model);
                    } else {
                        ret = Some(Self { model: Some(model), ..Default::default() });
                    }
                }
            }
        }
        ret
    }

    pub fn parse<T: Read + Seek, F: Fn(f64)>(&mut self, stream: &mut T, size: usize, progress_cb: F, cancel_flag: Arc<AtomicBool>, options: crate::InputOptions) -> Result<Vec<SampleInfo>> {
        let mut samples = Vec::new();
        if let Some(extra) = &self.extra_gpmf {
            samples.push(SampleInfo { tag_map: Some(extra.clone()), ..Default::default() });
        }

        let mut fps = None;

        if self.is_raw_gpmf {
            let mut data = Vec::with_capacity(size);
            stream.read_to_end(&mut data)?;
            for pos in memmem::find_iter(&data, b"DEVC") {
                let chunk = &data[pos..];
                if Self::detect_metadata(chunk) {
                    let next = memmem::find(&chunk[8..], b"DEVC").unwrap_or(chunk.len() - 8) + 8;
                    let res = GoPro::parse_metadata(&chunk[8..next], GroupId::Default, false, &options);
                    if let Ok(mut map) = res {
                        self.process_map(&mut map);
                        samples.push(SampleInfo { tag_map: Some(map), ..Default::default() });
                        if options.probe_only {
                            break;
                        }
                    }
                }
            }
        } else {
            let cancel_flag2 = cancel_flag.clone();
            let ctx = util::get_metadata_track_samples(stream, size, true, |mut info: SampleInfo, data: &[u8], file_position: u64, _video_md: Option<&VideoMetadata>| {
                if size > 0 {
                    progress_cb(file_position as f64 / size as f64);
                }
                if Self::detect_metadata(data) {
                    if let Ok(mut map) = GoPro::parse_metadata(&data[8..], GroupId::Default, false, &options) {
                        self.process_map(&mut map);
                        info.tag_map = Some(map);
                        samples.push(info);
                    }
                }
                if options.probe_only {
                    cancel_flag2.store(true, std::sync::atomic::Ordering::Relaxed);
                }
            }, cancel_flag)?;
            if !ctx.tracks.is_empty() {
                fps = util::get_fps_from_track(&ctx.tracks[0]);
            }
        }
        self.process_samples(&mut samples, fps, &options);

        if self.model.as_ref().map(|x| x.contains("HERO5")).unwrap_or_default() {
            if samples.is_empty() {
                samples.push(SampleInfo { tag_map: Some(GroupedTagMap::default()), ..Default::default() });
            }
            let first_sample = samples.first_mut().unwrap();
            if let Some(ref mut first_map) = first_sample.tag_map {
                if let Ok(_) = stream.seek(SeekFrom::Start(0)) {
                    while let Ok((typ, _offs, size, header_size)) = util::read_box(stream) {
                        if size == 0 || typ == 0 { break; }
                        let org_pos = stream.stream_position()?;
                        if typ == fourcc("moov") || typ == fourcc("udta") {
                            continue; // go inside these boxes
                        } else {
                            if typ == fourcc("SETT") {
                                let mut buf = vec![0u8; size as usize - header_size as usize];
                                stream.read_exact(&mut buf)?;
                                if buf.len() > 8 {
                                    match buf[5] {
                                        0x00 => util::insert_tag(first_map, tag!(parsed GroupId::Default, TagId::Unknown(0x45495341), "EISA", String, |v| v.clone(), "N".into(), vec![]), &options),
                                        0x02 => util::insert_tag(first_map, tag!(parsed GroupId::Default, TagId::Unknown(0x45495341), "EISA", String, |v| v.clone(), "N".into(), vec![]), &options),
                                        0x10 => util::insert_tag(first_map, tag!(parsed GroupId::Default, TagId::Unknown(0x45495341), "EISA", String, |v| v.clone(), "Y".into(), vec![]), &options),
                                        _ => log::debug!("Unknown stab byte {}", util::to_hex(&buf)),
                                    }
                                    match buf[7] {
                                        0x00 => util::insert_tag(first_map, tag!(parsed GroupId::Default, TagId::Unknown(0x56464f56), "VFOV", String, |v| v.clone(), "W".into(), vec![]), &options),
                                        0x40 => util::insert_tag(first_map, tag!(parsed GroupId::Default, TagId::Unknown(0x56464f56), "VFOV", String, |v| v.clone(), "W".into(), vec![]), &options),
                                        0x41 => util::insert_tag(first_map, tag!(parsed GroupId::Default, TagId::Unknown(0x56464f56), "VFOV", String, |v| v.clone(), "M".into(), vec![]), &options),
                                        0x42 => util::insert_tag(first_map, tag!(parsed GroupId::Default, TagId::Unknown(0x56464f56), "VFOV", String, |v| v.clone(), "N".into(), vec![]), &options),
                                        0x44 => util::insert_tag(first_map, tag!(parsed GroupId::Default, TagId::Unknown(0x56464f56), "VFOV", String, |v| v.clone(), "L".into(), vec![]), &options),
                                        0x63 => util::insert_tag(first_map, tag!(parsed GroupId::Default, TagId::Unknown(0x56464f56), "VFOV", String, |v| v.clone(), "S".into(), vec![]), &options),
                                        _ => log::debug!("Unknown lens byte {}", util::to_hex(&buf)),
                                    }
                                }
                            }
                            stream.seek(SeekFrom::Start(org_pos + size - header_size as u64))?;
                        }
                    }
                }
            }
        }

        // Build a native lens profile from the in-camera POLY calibration, if present.
        lens_profile::insert_lens_profile(&mut samples, self.model.as_deref(), &options);

        Ok(samples)
    }

    fn detect_metadata(data: &[u8]) -> bool {
        data.len() > 8 && &data[0..4] == b"DEVC"
    }

    pub fn parse_metadata(data: &[u8], group_id: GroupId, force_group: bool, options: &crate::InputOptions) -> Result<GroupedTagMap> {
        let mut slice = Cursor::new(data);
        let datalen = data.len() as u64;
        let mut map = GroupedTagMap::new();

        let mut last_type = None;

        while slice.position() < datalen {
            let start_pos = slice.position() as usize;
            if datalen as i64 - start_pos as i64 >= 8 {
                let mut klv = KLV::parse_header(&mut slice)?;
                let pos = slice.position() as usize;

                let len = klv.data_len();
                if len == 0 { continue; }

                let full_tag_data = &data[start_pos..(pos + len)];
                let tag_data      = &data[pos      ..(pos + len)];

                slice.seek(SeekFrom::Current(klv.aligned_data_len() as i64))?;

                if klv.data_type == 0 { // Container
                    let container_group = if force_group { group_id.clone() } else { KLV::group_from_key(GoPro::get_last_klv(tag_data)?) };
                    for (g, v) in GoPro::parse_metadata(tag_data, container_group, force_group, options)? {
                        let group_map = map.entry(g).or_insert_with(TagMap::new);
                        group_map.extend(v);
                    }
                    continue;
                }

                if &klv.key == b"TYPE" {
                    if let TagValue::String(typedef) = klv.parse_data(full_tag_data) {
                        last_type = Some(typedef.get().clone());
                        continue;
                    }
                }
                if klv.data_type == b'?' && last_type.is_some() {
                    klv.custom_type = last_type.clone();
                }

                util::insert_tag(&mut map, TagDescription {
                    group:       group_id.clone(),
                    id:          klv.tag_id(),
                    description: klv.key_as_string(),
                    value:       klv.parse_data(full_tag_data),
                    native_id:   Some((&klv.key[..]).read_u32::<BigEndian>()?)
                }, options);
            } else {
                break;
            }
        }

        Ok(map)
    }

    fn process_map(&self, tag_map: &mut GroupedTagMap) {
        for (g, v) in tag_map.iter_mut() {
            // If we have ORIN and ORIO but not MTRX, construct MTRX from ORIN and ORIO and insert to the map
            if v.contains_key(&TagId::OrientationIn) && v.contains_key(&TagId::OrientationOut) && !v.contains_key(&TagId::Matrix) {
                crate::try_block!({
                    let m = KLV::orientations_to_matrix(
                        (v.get_t(TagId::OrientationIn)  as Option<&String>)?,
                        (v.get_t(TagId::OrientationOut) as Option<&String>)?
                    )?;
                    v.insert(TagId::Matrix,
                        crate::tag!(parsed g.clone(), TagId::Matrix, "MTRX", Vec_Vec_f32, |v| format!("{:?}", v), vec![m], Vec::new())
                    );
                });
            }

            // Convert MTRX to Orientation tag
            if g == &GroupId::Gyroscope || g == &GroupId::Accelerometer || g == &GroupId::Magnetometer {
                let mut imu_orientation = None;
                if let Some(m) = v.get_t(TagId::Matrix) as Option<&Vec<Vec<f32>>> {
                    if !m.is_empty() && !m[0].is_empty() {
                        imu_orientation = Some(GoPro::mtrx_to_orientation(&m[0]));
                    }
                } else if let Some(m) = &self.model {
                    if m.contains("HERO6") { imu_orientation = Some("ZyX".to_string()); }
                    if m.contains("HERO7 Silver") { imu_orientation = Some("YXz".to_string()); }
                }
                if let Some(o) = imu_orientation {
                    v.insert(TagId::Orientation, crate::tag!(parsed g.clone(), TagId::Orientation, "IMUO", String, |v| v.to_string(), o, Vec::new()));
                }
            }
        }
    }

    fn get_timestamp(info: &util::SampleInfo, group_id: &GroupId) -> Option<i64> {
        if let Some(ref grouped_tag_map) = info.tag_map {
            for (group, map) in grouped_tag_map {
                if group == group_id {
                    let mut tick = 0i64;
                    if let Some(t) = map.get_t(TagId::Unknown(0x5449434b /*TICK*/)) as Option<&u32> {
                        tick = (*t as i64) * 1000;
                    }
                    let timestamp_us = (map.get_t(TagId::TimestampUs) as Option<&u64>).map(|x| *x as i64).unwrap_or(tick);
                    return Some(timestamp_us);
                }
            }
        }
        None
    }

    fn process_samples(&mut self, samples: &mut Vec<SampleInfo>, fps: Option<f64>, options: &crate::InputOptions) {
        // Normalize quaternions
        let mut prev_increment = 0;
        let mut start_timestamp_us = None;
        let mut global_ts_cori: f64 = 0.0;
        let mut global_ts_iori: f64 = 0.0;
        let global_increment = fps.map(|x| 1000.0 / x);
        for i in 0..samples.len() {
            let info = &samples[i];
            if info.tag_map.is_none() { continue; }

            let grouped_tag_map = info.tag_map.as_ref().unwrap();

            let mut cori = Vec::new();
            let mut iori = Vec::new();
            for (group, map) in grouped_tag_map.iter() {
                if group == &GroupId::CameraOrientation || group == &GroupId::ImageOrientation {
                    let scale = *(map.get_t(TagId::Scale) as Option<&i16>).unwrap_or(&32767) as f64;
                    let mut timestamp_us = *(map.get_t(TagId::TimestampUs) as Option<&u64>).unwrap_or(&0) as i64;
                    // let start_count = *(map.get_t(TagId::Count) as Option<&u32>).unwrap_or(&0);
                    let next_timestamp_us = samples.get(i + 1).map(|x | Self::get_timestamp(x, &group)).unwrap_or(None);
                    if start_timestamp_us.is_none() {
                        start_timestamp_us = Some(timestamp_us);
                    }
                    // TODO https://github.com/gopro/gpmf-parser/blob/master/GPMF_utils.c
                    if let Some(arr) = map.get_t(TagId::Data) as Option<&Vec<Quaternion<i16>>> {
                        self.has_cori = true;
                        let sample_count = arr.len() as i64;
                        let increment = next_timestamp_us.map(|x| (x - timestamp_us) / sample_count).unwrap_or(prev_increment);
                        prev_increment = increment;
                        for v in arr.iter() {
                            let mut ts = timestamp_us - start_timestamp_us.unwrap();
                            if let Some(global_inc) = global_increment {
                                if group == &GroupId::CameraOrientation {
                                    ts = (global_ts_cori * 1000.0).round() as i64;
                                    global_ts_cori += global_inc;
                                } else {
                                    ts = (global_ts_iori * 1000.0).round() as i64;
                                    global_ts_iori += global_inc;
                                }
                            }
                            let aout = if group == &GroupId::CameraOrientation { &mut cori } else { &mut iori };
                            aout.push((
                                ts,
                                Quaternion {
                                    w: v.w as f64 / scale,
                                    x: -v.x as f64 / scale,
                                    y: v.y as f64 / scale,
                                    z: v.z as f64 / scale
                                }
                            ));
                            timestamp_us += increment;
                        }
                    }
                }
            }
            if !cori.is_empty() && cori.len() == iori.len() {
                // Multiply CORI * IORI
                let quat = cori.into_iter().zip(iori.into_iter()).map(|(c, i)| TimeQuaternion {
                    t: c.0 as f64 / 1000.0,
                    v: c.1 * i.1
                }).collect();

                let grouped_tag_map = samples[i].tag_map.as_mut().unwrap();
                util::insert_tag(grouped_tag_map, tag!(parsed GroupId::Quaternion, TagId::Data, "Quaternion data",  Vec_TimeQuaternion_f64, |v| format!("{:?}", v), quat, vec![]), options);
            }

            // Fold raw GPS5 / GPS9 rows into Vec<GpsData> under
            // GroupId::GPS / TagId::Data, matching what CAMM (mod.rs:176),
            // Insta360 (record.rs:206), and gyroflow (binary.rs:937) emit.
            // Downstream telemetry consumers see a single uniform shape
            // regardless of GoPro model.
            Self::process_gps_samples(samples, i, options);
        }
    }

    /// Post-process the GPS group in one sample: reads the raw
    /// `Vec_Vec_i32` rows (GPS5 = 5 columns, GPS9 = 9 columns) plus SCAL /
    /// GPSU / GPSF, and replaces `GroupId::GPS / TagId::Data` with a
    /// pre-scaled `Vec<GpsData>` — the same shape emitted by CAMM /
    /// Insta360 / gyroflow. Non-GoPro brands are untouched.
    fn process_gps_samples(samples: &mut Vec<SampleInfo>, i: usize, options: &crate::InputOptions) {
        let info = &samples[i];
        let tag_map = match info.tag_map.as_ref() {
            Some(m) => m,
            None => return,
        };
        let gps_map = match tag_map.get(&GroupId::GPS) {
            Some(m) => m,
            None => return,
        };
        // Find the raw GPS5 / GPS9 descriptor (both land under TagId::Data
        // per klv.rs, disambiguated by the FourCC in `description`).
        let data_desc = match gps_map.get(&TagId::Data) {
            Some(d) => d,
            None => return,
        };
        let (rows, is_gps9) = match (&data_desc.value, data_desc.description.as_str()) {
            (TagValue::Vec_Vec_i32(v), "GPS5") => (v.get(), false),
            (TagValue::Vec_Vec_i32(v), "GPS9") => (v.get(), true),
            // Already post-processed (Vec_GpsData) or a different producer.
            _ => return,
        };

        // SCAL divisors — GoPro authoritative layout:
        //   GPS5: [1e7, 1e7, 1e3, 1e3, 1e2]   (lat, lon, alt, spd2d, spd3d)
        //   GPS9: [1e7, 1e7, 1e3, 1e3, 1e3, 1, 1e3, 1e2, 1]
        //         (lat, lon, alt, spd2d, spd3d, days, secs*1e3, DOP, fix)
        // Read whatever the file shipped and fall back to spec defaults so
        // synthetic tests don't need to author SCAL.
        let scal: Vec<f64> = match &gps_map.get(&TagId::Scale).map(|d| &d.value) {
            Some(TagValue::Vec_i32(v)) => v.get().iter().map(|x| *x as f64).collect(),
            Some(TagValue::Vec_u32(v)) => v.get().iter().map(|x| *x as f64).collect(),
            _ => Vec::new(),
        };
        let scale = |col: usize, default: f64| -> f64 {
            scal.get(col).copied().filter(|v| *v > 0.0).unwrap_or(default)
        };

        // GPSU (frame-level UTC, ms since epoch) — GPS5 only. GPS9 carries
        // days + secs-since-midnight per row.
        const GPSU_FOURCC: u32 = u32::from_be_bytes(*b"GPSU");
        const GPSF_FOURCC: u32 = u32::from_be_bytes(*b"GPSF");
        let gpsu_ms: Option<u64> = gps_map
            .get_t(TagId::Unknown(GPSU_FOURCC))
            .copied();
        // GPSF: 0 = no lock, 2 = 2D lock, 3 = 3D lock (u32 in the file).
        let gpsf: Option<u32> = gps_map
            .get_t(TagId::Unknown(GPSF_FOURCC))
            .copied();

        // 2000-01-01T00:00:00Z in Unix epoch seconds — GPS9 origin.
        const EPOCH_2000_UNIX_S: i64 = 946_684_800;

        let mut gps = Vec::with_capacity(rows.len());
        for row in rows {
            let (needed, is_2d_only) = if is_gps9 { (9, false) } else { (5, true) };
            if row.len() < needed {
                continue;
            }

            let lat = row[0] as f64 / scale(0, 1e7);
            let lon = row[1] as f64 / scale(1, 1e7);
            let alt = row[2] as f64 / scale(2, 1e3);
            // 2D speed for GpsData.speed; 3D goes unused.
            let speed = row[3] as f64 / scale(3, 1e3);

            let (unix_ts_s, acquired) = if is_gps9 {
                let days = row[5] as i64;
                let secs_ms = row[6] as f64 / scale(6, 1e3);
                let fix = row[8];
                let t = EPOCH_2000_UNIX_S as f64 + days as f64 * 86_400.0 + secs_ms;
                (t, fix >= 2)
            } else {
                // GPS5: no per-row fix in-band; use frame-level GPSF and
                // GPSU. All fixes in this frame share the timestamp.
                let t = gpsu_ms.map(|ms| ms as f64 / 1000.0).unwrap_or(0.0);
                (t, gpsf.map(|f| f >= 2).unwrap_or(t > 0.0))
            };

            let _ = is_2d_only; // reserved for future 2D/3D distinction

            gps.push(GpsData {
                is_acquired: acquired,
                unix_timestamp: unix_ts_s,
                lat,
                lon,
                speed,
                track: 0.0,
                altitude: alt,
            });
        }

        // Convert km/h ↔ m/s? tags_impl.rs:352 GpsData::speed is documented
        // as "in km/h", but Insta360 (record.rs) stores raw NMEA knots and
        // CAMM stores m/s — the unit is currently producer-defined. GoPro
        // SCAL yields m/s (1e3), which matches CAMM. Leaving as-is; a
        // consumer aggregating across producers is already responsible
        // for unit reconciliation.

        let grouped_tag_map = samples[i].tag_map.as_mut().unwrap();
        util::insert_tag(
            grouped_tag_map,
            tag!(parsed GroupId::GPS, TagId::Data, "GPS data", Vec_GpsData, |v| format!("{:?}", v), gps, vec![]),
            options,
        );
    }
    pub fn get_avg_sample_duration(samples: &Vec<SampleInfo>, group_id: &GroupId) -> Option<f64> {
        let mut total_duration_ms = 0.0;

        let mut first_tsus = None;
        let mut last_tsus = None;
        let mut count = 0;
        let mut last_len = 0;
        for info in samples {
            total_duration_ms += info.duration_ms;
            if info.tag_map.is_none() { continue; }
            for (group, map) in info.tag_map.as_ref().unwrap() {
                if group == group_id {
                    if let Some(t) = map.get_t(TagId::TimestampUs) as Option<&u64> {
                        if first_tsus.is_none() { first_tsus = Some(*t as i64); }
                        last_tsus = Some(*t as i64);
                    } else if let Some(t) = map.get_t(TagId::TimestampMs) as Option<&u64> {
                        if first_tsus.is_none() { first_tsus = Some((*t as i64) * 1000); }
                        last_tsus = Some((*t as i64) * 1000);
                    } else if let Some(t) = map.get_t(TagId::Unknown(0x5449434b /*TICK*/)) as Option<&u32> {
                        if first_tsus.is_none() { first_tsus = Some((*t as i64) * 1000); }
                        last_tsus = Some((*t as i64) * 1000);
                    }
                    if let Some(t) = map.get_t(TagId::Data) as Option<&Vec<Vector3<i16>>> {
                        count += t.len();
                        last_len = t.len();
                    } else if let Some(t) = map.get_t(TagId::Data) as Option<&Vec<Quaternion<i16>>> {
                        count += t.len();
                        last_len = t.len();
                    }
                }
            }
        }
        if first_tsus.is_some() && last_tsus.is_some() && count > 0 && last_tsus.unwrap() > first_tsus.unwrap() {
            Some((last_tsus.unwrap() as f64 - first_tsus.unwrap() as f64) / (count - last_len).max(1) as f64 / 1000.0)
        } else if count > 0 {
            Some(total_duration_ms / count as f64)
        } else {
            None
        }
    }

    pub fn get_last_klv(data: &[u8]) -> Result<&[u8]> {
        let mut slice = Cursor::new(data);

        let mut offset = 0;
        while slice.position() < data.len() as u64 {
            offset = slice.position() as usize;
            let klv = KLV::parse_header(&mut slice)?;
            slice.seek(SeekFrom::Current(klv.aligned_data_len() as i64))?;
        }
        Ok(&data[offset..])
    }

    fn mtrx_to_orientation(mtrx: &[f32]) -> String {
        assert!(mtrx.len() == 9);

        (0..3).map(|x| {
                 if mtrx[x * 3 + 0] > 0.5 { 'X' } else if mtrx[x * 3 + 0] < -0.5 { 'x' }
            else if mtrx[x * 3 + 1] > 0.5 { 'Y' } else if mtrx[x * 3 + 1] < -0.5 { 'y' }
            else if mtrx[x * 3 + 2] > 0.5 { 'Z' } else if mtrx[x * 3 + 2] < -0.5 { 'z' }
            else { panic!("Invalid MTRX {:?}", mtrx) }
        }).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::util;
    use std::collections::BTreeMap;

    fn fmt_vec_vec_i32(v: &Vec<Vec<i32>>) -> String { format!("{:?}", v) }
    fn fmt_vec_i32(v: &Vec<i32>) -> String { format!("{:?}", v) }
    fn fmt_u64(v: &u64) -> String { format!("{}", v) }
    fn fmt_u32(v: &u32) -> String { format!("{}", v) }

    fn make_sample(tags: Vec<(GroupId, TagId, String, TagValue)>) -> util::SampleInfo {
        let mut map: GroupedTagMap = BTreeMap::new();
        for (g, id, description, value) in tags {
            let desc = TagDescription {
                group: g.clone(),
                id: id.clone(),
                native_id: None,
                description,
                value,
            };
            map.entry(g).or_default().insert(id, desc);
        }
        util::SampleInfo {
            tag_map: Some(map),
            ..Default::default()
        }
    }

    #[test]
    fn process_gps_samples_gps9_hero11() {
        // Layout per gopro/gpmf-parser: 9 int32 columns
        //   [lat, lon, alt, spd2d, spd3d, days_since_2000, ms_since_midnight, DOP, fix]
        // Row 1: acquired 3D fix at 2023-11-14T12:26:40Z
        //   days_since_2000 = 8718
        //   secs_since_midnight = 44_800 ⇒ ms = 44_800_000
        //   → unix_ts_s = 946_684_800 + 8718 * 86_400 + 44_800 = 1_700_000_000
        // Row 2: fix=0 must be dropped
        // Row 3: acquired 3D fix at +0.2 s
        let rows = vec![
            vec![312_304_000, 1_214_737_000, 10_000, 5_000, 5_100, 8718, 44_800_000, 100, 3],
            vec![0, 0, 0, 0, 0, 8718, 44_800_100, 100, 0],
            vec![312_305_000, 1_214_738_000, 11_000, 4_500, 4_600, 8718, 44_800_200, 100, 3],
        ];
        const GPS9_FOURCC: u32 = u32::from_be_bytes(*b"GPS9");
        let mut samples = vec![make_sample(vec![
            (GroupId::GPS, TagId::Data, "GPS9".into(),
                TagValue::Vec_Vec_i32(ValueType::new_parsed(fmt_vec_vec_i32, rows, vec![]))),
            (GroupId::GPS, TagId::Scale, "SCAL".into(),
                TagValue::Vec_i32(ValueType::new_parsed(fmt_vec_i32,
                    vec![10_000_000, 10_000_000, 1_000, 1_000, 1_000, 1, 1_000, 100, 1],
                    vec![]))),
        ])];

        let _ = GPS9_FOURCC; // reserved for future direct-tag lookups
        GoPro::process_gps_samples(&mut samples, 0, &crate::InputOptions::default());

        let map = samples[0].tag_map.as_ref().unwrap();
        let gps_map = map.get(&GroupId::GPS).unwrap();
        let out: &Vec<GpsData> = gps_map.get_t(TagId::Data).expect("GPS Vec_GpsData missing");
        assert_eq!(out.len(), 3, "process_gps_samples keeps every row incl. unlocked");
        assert!(out[0].is_acquired);
        assert!(!out[1].is_acquired, "fix=0 → not acquired");
        assert!(out[2].is_acquired);
        assert!((out[0].lat - 31.2304).abs() < 1e-9);
        assert!((out[0].lon - 121.4737).abs() < 1e-9);
        assert!((out[0].altitude - 10.0).abs() < 1e-9);
        assert!((out[0].speed - 5.0).abs() < 1e-9);
        // 2023-11-14T12:26:40Z: 8718 days since 2000-01-01 + 44_800 s.
        // Unix = 946_684_800 + 8718 * 86_400 + 44_800 = 1_699_964_800.
        assert!((out[0].unix_timestamp - 1_699_964_800.0).abs() < 1e-3);
        assert!((out[2].unix_timestamp - 1_699_964_800.200).abs() < 1e-3);
    }

    #[test]
    fn process_gps_samples_gps5_legacy() {
        // Legacy GPS5 (HERO5–HERO10): 5 int32 columns
        //   [lat, lon, alt, spd2d, spd3d]
        //   frame-level timestamp from GPSU (ms), fix from GPSF.
        let rows = vec![
            vec![312_304_000, 1_214_737_000, 10_000, 5_000, 51_000],
            vec![312_305_000, 1_214_738_000, 11_000, 4_500, 46_000],
        ];
        const GPSU_FOURCC: u32 = u32::from_be_bytes(*b"GPSU");
        const GPSF_FOURCC: u32 = u32::from_be_bytes(*b"GPSF");
        let mut samples = vec![make_sample(vec![
            (GroupId::GPS, TagId::Data, "GPS5".into(),
                TagValue::Vec_Vec_i32(ValueType::new_parsed(fmt_vec_vec_i32, rows, vec![]))),
            (GroupId::GPS, TagId::Scale, "SCAL".into(),
                TagValue::Vec_i32(ValueType::new_parsed(fmt_vec_i32,
                    vec![10_000_000, 10_000_000, 1_000, 1_000, 100],
                    vec![]))),
            (GroupId::GPS, TagId::Unknown(GPSU_FOURCC), "GPSU".into(),
                TagValue::u64(ValueType::new_parsed(fmt_u64, 1_700_000_000_000_u64, vec![]))),
            (GroupId::GPS, TagId::Unknown(GPSF_FOURCC), "GPSF".into(),
                TagValue::u32(ValueType::new_parsed(fmt_u32, 3_u32, vec![]))),
        ])];

        GoPro::process_gps_samples(&mut samples, 0, &crate::InputOptions::default());

        let map = samples[0].tag_map.as_ref().unwrap();
        let gps_map = map.get(&GroupId::GPS).unwrap();
        let out: &Vec<GpsData> = gps_map.get_t(TagId::Data).expect("GPS Vec_GpsData missing");
        assert_eq!(out.len(), 2);
        assert!(out.iter().all(|g| g.is_acquired), "GPSF=3 => all locked");
        assert!((out[0].lat - 31.2304).abs() < 1e-9);
        assert!((out[0].lon - 121.4737).abs() < 1e-9);
        assert!((out[0].altitude - 10.0).abs() < 1e-9);
        assert!((out[0].speed - 5.0).abs() < 1e-9);
        assert!((out[0].unix_timestamp - 1_700_000_000.0).abs() < 1e-3);
    }

    #[test]
    fn process_gps_samples_no_gps_group_noop() {
        let mut samples = vec![make_sample(vec![])];
        GoPro::process_gps_samples(&mut samples, 0, &crate::InputOptions::default());
        let map = samples[0].tag_map.as_ref().unwrap();
        assert!(map.get(&GroupId::GPS).is_none(), "no GPS group should stay absent");
    }
}
