use webdock_agent_protocol::{certificate_identity, transport_identity};
#[test]
fn absent_transport_identity_and_expired_certificates_are_denied() {
    assert!(transport_identity(&tonic::Request::new(()), 1000).is_err());
    assert!(certificate_identity(b"untrusted text", 1000).is_err());
    let mut p = rcgen::CertificateParams::new(vec!["localhost".into()]).unwrap();
    p.not_before = rcgen::date_time_ymd(2000, 1, 1);
    p.not_after = rcgen::date_time_ymd(2001, 1, 1);
    let key = rcgen::KeyPair::generate().unwrap();
    let cert = p.self_signed(&key).unwrap();
    assert!(certificate_identity(cert.der(), 2_000_000_000).is_err());
    let identity = certificate_identity(cert.der(), 970_000_000).unwrap();
    assert_eq!(identity.sha256.len(), 64);
    assert!(identity.not_after > 970_000_000);
}
