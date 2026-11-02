pub const LIMIT: usize = 12_000;
#[cfg(any(feature = "docker", feature = "kubernetes", test))]
pub fn bounded(bytes: &[u8]) -> String {
    let mut text = String::from_utf8_lossy(&bytes[..bytes.len().min(LIMIT)]).into_owned();
    let mut end = text.len().min(LIMIT);
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    text.truncate(end);
    text
}
#[cfg(test)]
mod tests {
    #[test]
    fn preserves_utf8_without_expanding_past_wire_bound() {
        let bytes = "é".repeat(7000).into_bytes();
        let value = super::bounded(&bytes);
        assert_eq!(value.len(), super::LIMIT);
        assert!(value.ends_with('é'));
        let value = super::bounded(&vec![255; super::LIMIT]);
        assert_eq!(value.len(), super::LIMIT);
    }
}
