pub use webdock_mail_message::*;

/// The SMTP policy proof remains part of Core's admission contract, separate from MIME handling.
pub fn stamp_message(raw: &[u8], id: &str, proof: &str) -> crate::Result<Vec<u8>> {
    if !crate::proof::valid_id(id)
        || proof.len() != 43
        || !proof
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"_-".contains(&b))
    {
        return Err(crate::Error::Invalid);
    }
    let mut out = format!("X-Webdock-Submission: {id}.{proof}\r\n").into_bytes();
    if out.len() + raw.len() > MAX_MESSAGE {
        return Err(crate::Error::Invalid);
    }
    out.extend_from_slice(raw);
    Ok(out)
}
