use serde_json::json;
use std::io::Read;
use webdock_archives::diagnostics::Diagnostics;

fn sample() -> serde_json::Value {
    json!({"version":1,"state":"finished","builds":[{"id":"123","outcome":"succeeded","duration_ms":12,"output_bytes":42}],"checks":[{"component":"storage","outcome":"healthy","duration_ms":3}]})
}
#[test]
fn compressed_archive_contains_only_finished_typed_diagnostics() {
    let d = Diagnostics::parse(sample()).unwrap();
    let gz = d.compress().unwrap();
    let mut output = String::new();
    flate2::read::GzDecoder::new(gz.as_slice())
        .read_to_string(&mut output)
        .unwrap();
    let v: serde_json::Value = serde_json::from_str(&output).unwrap();
    assert_eq!(v, sample());
}
#[test]
fn free_text_secret_channels_and_unfinished_data_are_rejected() {
    for key in ["logs", "token", "credentials", "url", "mail_body"] {
        let mut v = sample();
        v[key] = json!("https://u:secret@private.invalid/?token=secret");
        assert!(Diagnostics::parse(v).is_err());
    }
    for path in ["outcome", "id"] {
        let mut v = sample();
        v["builds"][0][path] = json!("secret-token");
        assert!(Diagnostics::parse(v).is_err());
    }
    let mut v = sample();
    v["state"] = json!("running");
    assert!(Diagnostics::parse(v).is_err());
    let mut v = sample();
    v["builds"][0]["logs"] = json!("mail body");
    assert!(Diagnostics::parse(v).is_err());
    let mut v = sample();
    v["checks"][0]["component"] = json!("https://user:pass@host");
    assert!(Diagnostics::parse(v).is_err());
    let mut v = sample();
    v["builds"] = json!(vec![sample()["builds"][0].clone(); 257]);
    assert!(Diagnostics::parse(v).is_err());
}
