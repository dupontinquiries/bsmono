//! vCard 3.0 serialiser — ported from VCardIO::exportContact().

use crate::{escape::escape, VCard};
use chrono::Utc;
use uuid::Uuid;

/// Serialise a `VCard` to a vCard 3.0 string (CRLF line endings, RFC 2426).
pub fn write_vcard(card: &VCard) -> String {
    let uid = if card.uid.is_empty() {
        Uuid::new_v4().to_string()
    } else {
        card.uid.clone()
    };
    let rev = card.rev.clone()
        .unwrap_or_else(|| Utc::now().format("%Y-%m-%dT%H:%M:%SZ").to_string());

    let name = escape(&card.display_name);
    let mut out = String::with_capacity(512);

    line(&mut out, "BEGIN:VCARD");
    line(&mut out, "VERSION:3.0");
    line(&mut out, &format!("FN:{}", name));
    // N: family;given;additional;prefix;suffix — we use FN as family name
    line(&mut out, &format!("N:{};;;;;", name));
    line(&mut out, &format!("UID:{}", uid));
    line(&mut out, &format!("REV:{}", rev));

    if !card.nickname.is_empty() {
        line(&mut out, &format!("NICKNAME:{}", escape(&card.nickname)));
    }
    if !card.notes.is_empty() {
        line(&mut out, &format!("NOTE:{}", escape(&card.notes)));
    }
    if let Some(ref sc) = card.sc_id {
        if !sc.is_empty() {
            line(&mut out, &format!("X-SNAPCLONE-ID:{}", sc));
        }
    }
    for tel in &card.phone {
        line(&mut out, &format!("TEL;TYPE=VOICE:{}", escape(tel)));
    }
    for mail in &card.email {
        line(&mut out, &format!("EMAIL:{}", escape(mail)));
    }
    if let Some(ref adr) = card.address {
        line(&mut out, &format!("ADR:{}", escape(adr)));
    }
    if let Some(ref photo) = card.photo_b64 {
        // Fold long base64 lines per RFC 2426 §2.6
        line(&mut out, "PHOTO;ENCODING=BASE64;TYPE=JPEG:");
        for chunk in photo.as_bytes().chunks(72) {
            out.push(' ');
            out.push_str(std::str::from_utf8(chunk).unwrap_or(""));
            out.push_str("\r\n");
        }
    }
    line(&mut out, "END:VCARD");
    out
}

fn line(out: &mut String, s: &str) {
    out.push_str(s);
    out.push_str("\r\n");
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn contains_required_fields() {
        let v = VCard { display_name: "Bob".to_owned(), ..Default::default() };
        let s = write_vcard(&v);
        assert!(s.contains("BEGIN:VCARD"));
        assert!(s.contains("FN:Bob"));
        assert!(s.contains("END:VCARD"));
        assert!(s.contains("VERSION:3.0"));
        assert!(s.contains("UID:"));
    }
    #[test]
    fn escapes_special_chars() {
        let v = VCard { display_name: "O'Brien, Jr.".to_owned(), ..Default::default() };
        let s = write_vcard(&v);
        assert!(s.contains("FN:O'Brien\\, Jr."));
    }
    #[test]
    fn sc_id_written() {
        let v = VCard {
            display_name: "Alice".to_owned(),
            sc_id: Some("sc-deadbeef".to_owned()),
            ..Default::default()
        };
        assert!(write_vcard(&v).contains("X-SNAPCLONE-ID:sc-deadbeef"));
    }
}
