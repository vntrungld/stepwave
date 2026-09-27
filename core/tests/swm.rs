use stepwave_core::swm::{SwmModel, GATES, HIDDEN};
use stepwave_core::CoreError;

const FIXTURE: &[u8] = include_bytes!("fixtures/model_random.swm");

fn header_len(bytes: &[u8]) -> usize {
    u32::from_le_bytes(bytes[8..12].try_into().unwrap()) as usize
}

/// Re-encode the fixture with its JSON header transformed by `edit`.
fn with_header(edit: impl FnOnce(&mut serde_json::Value)) -> Vec<u8> {
    let n = header_len(FIXTURE);
    let mut header: serde_json::Value = serde_json::from_slice(&FIXTURE[12..12 + n]).unwrap();
    edit(&mut header);
    let head = serde_json::to_vec(&header).unwrap();
    let mut out = FIXTURE[..8].to_vec();
    out.extend_from_slice(&(head.len() as u32).to_le_bytes());
    out.extend_from_slice(&head);
    out.extend_from_slice(&FIXTURE[12 + n..]);
    out
}

fn err_text(bytes: &[u8]) -> String {
    match SwmModel::from_bytes(bytes) {
        Err(CoreError::InvalidModel(msg)) => msg,
        Err(other) => panic!("expected InvalidModel, got {other:?}"),
        Ok(_) => panic!("expected an error"),
    }
}

#[test]
fn loads_fixture() {
    let m = SwmModel::from_bytes(FIXTURE).unwrap();
    assert_eq!(m.strength_db(), 6.0);
    assert_eq!(m.gain_db_range(), (-12.0, 12.0));
    assert_eq!(m.inp_weight().len(), 64 * 64);
    assert_eq!(m.gru1().w_ih.len(), GATES * 64);
    assert_eq!(m.gru2().w_hh.len(), GATES * HIDDEN);
    assert_eq!(m.out_weight().len(), 32 * HIDDEN);
    assert!(m.std().iter().all(|s| *s > 0.0));
}

#[test]
fn load_reads_a_file_and_reports_io_errors() {
    let path = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/fixtures/model_random.swm"
    );
    assert!(SwmModel::load(std::path::Path::new(path)).is_ok());
    let missing = SwmModel::load(std::path::Path::new("/nonexistent/x.swm"));
    assert!(matches!(missing, Err(CoreError::Io(_))));
}

#[test]
fn accepts_unknown_header_keys() {
    let bytes = with_header(|h| {
        h["target"] = serde_json::json!({"strength_db": 6.0});
        h["future_key"] = serde_json::json!([1, 2, 3]);
    });
    assert!(SwmModel::from_bytes(&bytes).is_ok());
}

#[test]
fn rejects_bad_container() {
    let mut bad_magic = FIXTURE.to_vec();
    bad_magic[..4].copy_from_slice(b"XXXX");
    assert!(err_text(&bad_magic).contains("magic"));

    let mut v2 = FIXTURE.to_vec();
    v2[4..8].copy_from_slice(&2u32.to_le_bytes());
    assert!(err_text(&v2).contains("version"));

    assert!(err_text(&FIXTURE[..FIXTURE.len() - 8]).contains("truncated"));
    let mut trailing = FIXTURE.to_vec();
    trailing.extend_from_slice(&[0, 0, 0, 0]);
    assert!(err_text(&trailing).contains("trailing"));
    assert!(err_text(&FIXTURE[..6]).contains("truncated"));

    let mut not_json = FIXTURE.to_vec();
    not_json[12] = b'#';
    assert!(err_text(&not_json).contains("header"));
}

#[test]
fn rejects_nan_weight() {
    let mut bytes = FIXTURE.to_vec();
    let last = bytes.len() - 4;
    bytes[last..].copy_from_slice(&f32::NAN.to_le_bytes());
    assert!(err_text(&bytes).contains("out.bias"));
}

#[test]
#[allow(clippy::type_complexity)]
fn rejects_bad_header_fields() {
    let cases: Vec<(&str, Box<dyn Fn(&mut serde_json::Value)>)> = vec![
        ("arch", Box::new(|h| h["arch"] = "lstm".into())),
        ("gate_order", Box::new(|h| h["gate_order"] = "zrn".into())),
        ("sizes", Box::new(|h| h["sizes"]["hidden"] = 128.into())),
        (
            "sample_rate",
            Box::new(|h| h["sample_rate"] = 44_100.into()),
        ),
        ("hop", Box::new(|h| h["hop"] = 512.into())),
        ("num_bands", Box::new(|h| h["num_bands"] = 24.into())),
        ("std", Box::new(|h| h["feature"]["std"][3] = 0.0.into())),
        (
            "mean",
            Box::new(|h| h["feature"]["mean"] = serde_json::json!([0.0])),
        ),
        (
            "gain_db_range",
            Box::new(|h| h["gain_db_range"] = serde_json::json!([12.0, -12.0])),
        ),
        (
            "gain_db_range",
            Box::new(|h| h["gain_db_range"] = serde_json::json!([1.0, 12.0])),
        ),
        ("strength_db", Box::new(|h| h["strength_db"] = 0.0.into())),
        (
            "tensors",
            Box::new(|h| h["tensors"][0]["name"] = "dense.weight".into()),
        ),
        (
            "feature.delta",
            Box::new(|h| h["feature"]["delta"] = "raw_diff".into()),
        ),
    ];
    for (field, edit) in cases {
        let msg = err_text(&with_header(edit));
        assert!(msg.contains(field), "{field}: message was {msg:?}");
    }
}

#[test]
fn accepts_missing_feature_delta() {
    let bytes = with_header(|h| {
        h["feature"].as_object_mut().unwrap().remove("delta");
    });
    assert!(SwmModel::from_bytes(&bytes).is_ok());
}

#[test]
fn rejects_declared_header_length_exceeding_file_size() {
    let mut bytes = FIXTURE.to_vec();
    let too_big = bytes.len() as u32 + 1;
    bytes[8..12].copy_from_slice(&too_big.to_le_bytes());
    assert!(err_text(&bytes).contains("truncated"));
}

#[test]
fn rejects_header_length_of_u32_max_without_panicking() {
    let mut bytes = FIXTURE.to_vec();
    bytes[8..12].copy_from_slice(&0xFFFF_FFFFu32.to_le_bytes());
    assert!(err_text(&bytes).contains("truncated"));
}

#[test]
fn load_rejects_oversized_file_without_reading_it_fully() {
    let path = std::env::temp_dir().join(format!(
        "stepwave-swm-oversize-{}-{:?}.swm",
        std::process::id(),
        std::thread::current().id()
    ));
    {
        let file = std::fs::File::create(&path).unwrap();
        file.set_len(32 * 1024 * 1024).unwrap(); // sparse: does not actually use 32 MiB on disk
    }
    let result = SwmModel::load(&path);
    std::fs::remove_file(&path).ok();
    match result {
        Err(CoreError::InvalidModel(msg)) => assert!(
            msg.contains("16") && msg.to_lowercase().contains("mib"),
            "message was {msg:?}"
        ),
        other => panic!("expected InvalidModel, got {other:?}"),
    }
}
