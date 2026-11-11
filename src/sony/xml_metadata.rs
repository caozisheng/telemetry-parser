// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright © 2021 Adrian <adrian.eddy at gmail>

use quick_xml::events::Event;
use quick_xml::Reader;

#[allow(dead_code)]
pub fn parse_from_file(stream: &mut std::fs::File) -> std::io::Result<Metadata> {
    let ctx = mp4parse::read_mp4(stream, mp4parse::ParseStrictness::Permissive)?;

    if let Some(Ok(md)) = ctx.metadata {
        if let Some(mp4parse::XmlBox::StringXmlBox(x)) = md.xml {
            let parsed_md = parse(&x[..]);
            log::info!("Model: {} {}, frames: {}", parsed_md.manufacturer, parsed_md.model, parsed_md.frame_count);
            return Ok(parsed_md);
        }
    }
    Err(std::io::ErrorKind::NotFound.into())
}

pub struct Metadata {
    pub manufacturer: String,
    pub model: String,
    pub frame_count: usize,
}

pub fn parse(data: &[u8]) -> Metadata {
    let mut reader = Reader::from_reader(data);
    let mut buf = Vec::new();

    let mut frame_count = 0usize;
    let mut model = String::new();
    let mut manufacturer = String::new();

    loop {
        match reader.read_event_into(&mut buf) {
            Ok(Event::Empty(ref e)) => {
                if e.name().as_ref() == "Duration" || e.name().as_ref() == "Device" {
                    for ox in e.attributes() {
                        if let Ok(x) = ox {
                            if x.key.as_ref() == "value"        { frame_count = x.value.parse::<usize>().unwrap(); }
                            if x.key.as_ref() == "modelName"    { model = x.value.to_string(); }
                            if x.key.as_ref() == "manufacturer" { manufacturer = x.value.to_string(); }
                        }
                    }
                }
            },
            Ok(Event::Eof) => break, // exits the loop when reaching end of file
            Err(e) => panic!("Error at position {}: {:?}", reader.buffer_position(), e),
            _ => (), // There are several other `Event`s we do not consider here
        }
        buf.clear();
    }
    Metadata { manufacturer, model, frame_count }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_device_and_duration_attributes() {
        let metadata = parse(br#"<NonRealTimeMeta><Duration value="120"/><Device manufacturer="Sony" modelName="ILCE-7SM3"/></NonRealTimeMeta>"#);
        assert_eq!(metadata.frame_count, 120);
        assert_eq!(metadata.manufacturer, "Sony");
        assert_eq!(metadata.model, "ILCE-7SM3");
    }
}
