use super::*;
use serde_json::{Value, json};

// Round-trip the actual generated stream types, not a hand-built JSON substitute.
fn stream_json<S: Message + Default + serde::Serialize>(stream: &S) -> (S, Value) {
    let decoded = S::decode(stream.encode_to_vec().as_slice()).unwrap();
    let json = serde_json::to_value(&decoded).unwrap();
    (decoded, json)
}

macro_rules! pano_compatibility_test {
    ($name:ident, $schema:ident) => {
        #[test]
        fn $name() {
            use super::$schema::*;
            let stream = StreamMeta {
                pano_dewarp_params: Some(PanoDewarpParams {
                    native_refine_master: Some(DewarpParams { fx: 1234.5, fy: 1200.0, cx: 960.0, cy: 960.0, width: 1920.0, height: 1920.0, ..Default::default() }),
                    native_refine_slave: Some(DewarpParams { fx: 1230.0, fy: 1210.0, cx: 950.0, cy: 970.0, width: 1920.0, height: 1920.0, ..Default::default() }),
                    ..Default::default()
                }),
                ..Default::default()
            };
            let (stream, original_stream) = stream_json(&stream);
            let clip = ClipMeta {
                clip_meta_header: Some(ClipMetaHeader { product_name: "DJI compatibility test".into(), ..Default::default() }),
                ..Default::default()
            };
            let original_clip = serde_json::to_value(&clip).unwrap();
            let mut projected = clip_metadata_json(&clip, Some(&stream)).unwrap();
            // This is the root key consumed by rimecut-public's extract_dual_lens.
            assert_eq!(projected.get("pano_dewarp_params"), original_stream.get("pano_dewarp_params"));
            assert_eq!(projected["pano_dewarp_params"]["native_refine_master"]["fx"], json!(1234.5));
            projected.as_object_mut().unwrap().remove("pano_dewarp_params");
            assert_eq!(projected, original_clip, "all existing clip keys must survive");
            assert_eq!(serde_json::to_value(&stream).unwrap(), original_stream);

            let (absent, _) = stream_json(&StreamMeta::default());
            let projected = clip_metadata_json(&clip, Some(&absent)).unwrap();
            assert_eq!(projected, original_clip, "null pano must not be projected");
            assert!(projected.get("pano_dewarp_params").is_none());
            assert_eq!(clip_metadata_json(&clip, None::<&StreamMeta>).unwrap(), original_clip);
        }
    };
}

pano_compatibility_test!(oq101_pano_clip_metadata_compatibility, dvtm_oq101);
pano_compatibility_test!(wa530_pano_clip_metadata_compatibility, dvtm_eagle4_wa530);

#[test]
fn wm169_clip_metadata_without_pano_is_unchanged() {
    let clip = dvtm_wm169::ClipMeta::default();
    let (stream, _) = stream_json(&dvtm_wm169::StreamMeta::default());
    let original = serde_json::to_value(&clip).unwrap();
    assert_eq!(clip_metadata_json(&clip, Some(&stream)).unwrap(), original);
    assert_eq!(clip_metadata_json(&clip, None::<&dvtm_wm169::StreamMeta>).unwrap(), original);
}

#[test]
fn pano_projection_is_wired_to_shared_clip_metadata_path() {
    // No MP4 fixture: pin the shared macro wiring alongside protobuf-level tests.
    let source = include_str!("mod.rs");
    let shared = source.split("macro_rules! handle_parsed").nth(1).unwrap()
        .split("macro_rules! handle_pano").next().unwrap();
    assert!(shared.contains("clip_metadata_json(clip, $parsed.stream_meta.as_ref())"));
    assert!(shared.contains("GroupId::Default, TagId::Metadata"));
    assert!(shared.contains("message_tag(\"StreamMetadata\", \"Stream metadata\", stream)"));
}
