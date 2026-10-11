//! The three shapes of one payment proof: its JSON, a single-line text code
//! (`GNRPROOF1-` and base64url of the compact JSON) for messages, and a PDF for
//! people. The PDF shows the details and carries the code, so any of the three
//! can be pasted or loaded on the Verify payment page.
use super::*;

pub(super) const CODE_PREFIX: &str = "GNRPROOF1-";
/// Proof files may be PDFs, which are larger than the JSON they carry.
pub(super) const MAX_PROOF_FILE_BYTES: usize = 256 * 1024;

const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";

fn base64url_encode(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let n = (u32::from(chunk[0]) << 16)
            | (u32::from(*chunk.get(1).unwrap_or(&0)) << 8)
            | u32::from(*chunk.get(2).unwrap_or(&0));
        let symbols = chunk.len() + 1;
        for index in 0..symbols {
            out.push(ALPHABET[(n >> (18 - 6 * index) & 63) as usize] as char);
        }
    }
    out
}
fn base64url_decode(text: &str) -> Option<Vec<u8>> {
    let value = |c: u8| ALPHABET.iter().position(|&a| a == c).map(|v| v as u32);
    let bytes = text.as_bytes();
    if bytes.len() % 4 == 1 {
        return None;
    }
    let mut out = Vec::with_capacity(bytes.len() * 3 / 4);
    for chunk in bytes.chunks(4) {
        let mut n = 0u32;
        for (index, &c) in chunk.iter().enumerate() {
            n |= value(c)? << (18 - 6 * index);
        }
        out.push((n >> 16) as u8);
        if chunk.len() > 2 {
            out.push((n >> 8) as u8);
        }
        if chunk.len() > 3 {
            out.push(n as u8);
        }
    }
    Some(out)
}

/// The proof as one line of text.
pub(super) fn encode_code(json: &str) -> Result<String> {
    let compact = serde_json::to_string(&serde_json::from_str::<Value>(json)?)?;
    Ok(format!(
        "{CODE_PREFIX}{}",
        base64url_encode(compact.as_bytes())
    ))
}

/// The proof JSON inside anything a person may paste or load: the JSON itself,
/// the text code (line breaks and spaces inside it are ignored), or a PDF made
/// by this wallet.
pub(super) fn extract_proof_json(input: &[u8]) -> Result<Zeroizing<String>> {
    ensure!(
        input.len() <= MAX_PROOF_FILE_BYTES,
        "This file is too large to be a payment proof."
    );
    let text = String::from_utf8_lossy(input);
    let trimmed = text.trim_start_matches('\u{feff}').trim();
    if trimmed.starts_with('{') {
        return Ok(Zeroizing::new(trimmed.to_owned()));
    }
    let mut best: Option<String> = None;
    for (start, _) in text.match_indices(CODE_PREFIX) {
        // Runs of code symbols separated by line breaks or spaces; words typed
        // after the code are dropped by trying the longest valid prefix.
        let mut runs = vec![String::new()];
        for c in text[start + CODE_PREFIX.len()..].chars() {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                runs.last_mut().expect("one run").push(c);
            } else if c.is_whitespace() {
                if !runs.last().expect("one run").is_empty() {
                    runs.push(String::new());
                }
            } else {
                break;
            }
        }
        let decoded = (1..=runs.len()).rev().find_map(|count| {
            base64url_decode(&runs[..count].concat())
                .and_then(|bytes| String::from_utf8(bytes).ok())
                .filter(|json| json.starts_with('{') && serde_json::from_str::<Value>(json).is_ok())
        });
        if let Some(json) = decoded
            && best.as_ref().is_none_or(|known| json.len() > known.len())
        {
            best = Some(json);
        }
    }
    best.map(Zeroizing::new).context(
        "No payment proof found. Paste the proof code (GNRPROOF1-…), the proof JSON, or load the .json or .pdf file.",
    )
}

/// What the PDF shows besides the code.
pub(super) struct PdfDetails<'a> {
    pub amount: &'a str,
    pub recipient: &'a str,
    pub txid: &'a str,
    pub action: u32,
    pub by_sender: bool,
    pub created_utc: String,
}

fn pdf_text(text: &str) -> String {
    text.chars()
        .filter(|c| c.is_ascii() && !c.is_ascii_control())
        .fold(String::new(), |mut out, c| {
            if matches!(c, '(' | ')' | '\\') {
                out.push('\\');
            }
            out.push(c);
            out
        })
}
/// Text lines written top to bottom.
struct PdfPage {
    content: String,
    y: f32,
}
impl PdfPage {
    fn line(&mut self, font: &str, size: f32, gap: f32, text: &str) {
        self.content.push_str(&format!(
            "BT /{font} {size} Tf 56 {:.1} Td ({}) Tj ET\n",
            self.y,
            pdf_text(text)
        ));
        self.y -= gap;
    }
}
/// A one-page A4 PDF with the proof's details and its code, built by hand with
/// the standard Helvetica and Courier fonts (nothing embedded). The code is
/// also stored whole in the document information, so loading the file finds it.
pub(super) fn pdf_document(details: &PdfDetails<'_>, code: &str) -> Vec<u8> {
    let mut page = PdfPage {
        content: String::new(),
        y: 790.0,
    };
    page.line("F2", 20.0, 30.0, "GENORYN payment proof");
    page.line(
        "F1",
        10.0,
        26.0,
        "Mainnet - a private payment, disclosed by choice",
    );
    page.line("F2", 11.0, 16.0, "Amount");
    page.line("F1", 14.0, 26.0, &format!("{} GNR", details.amount));
    page.line("F2", 11.0, 16.0, "Paid to (recipient address)");
    let recipient = details.recipient.as_bytes();
    for part in recipient.chunks(60) {
        page.line("F3", 9.5, 13.0, &String::from_utf8_lossy(part));
    }
    page.y -= 10.0;
    page.line("F2", 11.0, 16.0, "Transaction");
    page.line("F3", 9.5, 13.0, details.txid);
    page.line("F1", 9.5, 24.0, &format!("Output {}", details.action));
    page.line("F2", 11.0, 16.0, "Made by");
    page.line(
        "F1",
        10.0,
        24.0,
        if details.by_sender {
            "The paying wallet (it contains a key only the sender has)."
        } else {
            "The receiving wallet. It proves the payment, not who made this proof."
        },
    );
    page.line("F2", 11.0, 16.0, "Created");
    page.line("F1", 10.0, 28.0, &details.created_utc);
    page.line("F2", 11.0, 16.0, "How to verify");
    for text in [
        "Open the GENORYN wallet, go to Verify payment, and load this PDF file",
        "or paste the proof code below. The wallet checks it against the chain and",
        "can confirm that the payment went to a given address.",
    ] {
        page.line("F1", 10.0, 14.0, text);
    }
    page.y -= 12.0;
    page.line("F2", 11.0, 16.0, "Proof code");
    for part in code.as_bytes().chunks(76) {
        page.line("F3", 8.0, 10.5, &String::from_utf8_lossy(part));
    }
    page.y -= 12.0;
    page.line(
        "F1",
        8.0,
        11.0,
        "Anyone holding this proof can see this payment's recipient and amount (and its memo, if any).",
    );
    let objects = [
        "<< /Type /Catalog /Pages 2 0 R >>".to_owned(),
        "<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_owned(),
        "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 595 842] /Resources << /Font << /F1 4 0 R /F2 5 0 R /F3 6 0 R >> >> /Contents 7 0 R >>".to_owned(),
        "<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>".to_owned(),
        "<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica-Bold >>".to_owned(),
        "<< /Type /Font /Subtype /Type1 /BaseFont /Courier >>".to_owned(),
        format!("<< /Length {} >>\nstream\n{}endstream", page.content.len(), page.content),
        format!(
            "<< /Title (GENORYN payment proof) /Producer (GENORYN wallet) /GenorynProof ({}) >>",
            pdf_text(code)
        ),
    ];
    let mut pdf = b"%PDF-1.4\n%\xE2\xE3\xCF\xD3\n".to_vec();
    let mut offsets = Vec::new();
    for (index, object) in objects.iter().enumerate() {
        offsets.push(pdf.len());
        pdf.extend_from_slice(format!("{} 0 obj\n{object}\nendobj\n", index + 1).as_bytes());
    }
    let xref = pdf.len();
    let mut table = format!("xref\n0 {}\n0000000000 65535 f \n", objects.len() + 1);
    for offset in offsets {
        table.push_str(&format!("{offset:010} 00000 n \n"));
    }
    table.push_str(&format!(
        "trailer\n<< /Size {} /Root 1 0 R /Info {} 0 R >>\nstartxref\n{xref}\n%%EOF\n",
        objects.len() + 1,
        objects.len()
    ));
    pdf.extend_from_slice(table.as_bytes());
    pdf
}

/// `YYYY-MM-DD HH:MM UTC` for a Unix time.
pub(super) fn utc_text(unix: u64) -> String {
    let days = (unix / 86_400) as i64;
    let seconds = unix % 86_400;
    // Days to civil date (Howard Hinnant's algorithm).
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    format!(
        "{year:04}-{month:02}-{day:02} {:02}:{:02} UTC",
        seconds / 3_600,
        seconds % 3_600 / 60
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    const JSON: &str = r#"{"version":1,"network":"mainnet","txid":"ab","action":0,"recipient":"gnr1x","amount":5}"#;

    #[test]
    fn base64url_round_trips_every_length() {
        for length in 0..40 {
            let bytes: Vec<u8> = (0..length).map(|i| (i * 37 + 11) as u8).collect();
            let encoded = base64url_encode(&bytes);
            assert!(!encoded.contains('='));
            assert_eq!(base64url_decode(&encoded).unwrap(), bytes);
        }
        assert!(base64url_decode("a").is_none());
        assert!(base64url_decode("ab+c").is_none());
    }

    #[test]
    fn json_code_and_pdf_all_yield_the_same_proof() {
        let code = encode_code(JSON).unwrap();
        assert!(code.starts_with(CODE_PREFIX) && !code.contains(char::is_whitespace));
        // The code carries the same JSON content (key order may differ).
        let same = |text: &str| {
            serde_json::from_str::<Value>(text).unwrap()
                == serde_json::from_str::<Value>(JSON).unwrap()
        };
        assert_eq!(extract_proof_json(JSON.as_bytes()).unwrap().as_str(), JSON);
        assert!(same(&extract_proof_json(code.as_bytes()).unwrap()));
        // Copied out of a message or a PDF viewer: spaces and line breaks inside.
        let broken = format!(
            "Here is my proof:\n{}\n{} \r\n{}  thanks",
            &code[..20],
            &code[20..50],
            &code[50..]
        );
        assert!(same(&extract_proof_json(broken.as_bytes()).unwrap()));
        let pdf = pdf_document(
            &PdfDetails {
                amount: "1,000",
                recipient: "gnr1recipient",
                txid: "ab",
                action: 0,
                by_sender: true,
                created_utc: utc_text(0),
            },
            &code,
        );
        assert!(pdf.starts_with(b"%PDF-1.4") && pdf.ends_with(b"%%EOF\n"));
        assert!(same(&extract_proof_json(&pdf).unwrap()));
    }

    #[test]
    fn the_pdf_cross_reference_points_at_every_object() {
        let pdf = pdf_document(
            &PdfDetails {
                amount: "5",
                recipient: "gnr1(odd)\\name",
                txid: "cd",
                action: 1,
                by_sender: false,
                created_utc: utc_text(1_760_000_000),
            },
            "GNRPROOF1-AAAA",
        );
        // The binary marker comment is not UTF-8; same-length ASCII keeps offsets exact.
        let mut ascii = pdf.clone();
        ascii[10..14].copy_from_slice(b"xxxx");
        let text = String::from_utf8(ascii).unwrap();
        let start: usize = text
            .rsplit("startxref\n")
            .next()
            .unwrap()
            .lines()
            .next()
            .unwrap()
            .parse()
            .unwrap();
        assert!(text[start..].starts_with("xref"));
        let offsets: Vec<usize> = text[start..]
            .lines()
            .skip(3)
            .take_while(|line| line.ends_with(" n "))
            .map(|line| line[..10].parse().unwrap())
            .collect();
        assert_eq!(offsets.len(), 8);
        for (index, offset) in offsets.iter().enumerate() {
            assert!(text[*offset..].starts_with(&format!("{} 0 obj", index + 1)));
        }
        assert!(
            text.contains("gnr1\\(odd\\)\\\\name"),
            "PDF strings are escaped"
        );
    }

    #[test]
    fn nothing_else_is_taken_for_a_proof() {
        for input in [
            "",
            "hello",
            "GNRPROOF1-",
            "GNRPROOF1-!!!!",
            "GNRPROOF1-aGVsbG8",
        ] {
            assert!(extract_proof_json(input.as_bytes()).is_err(), "{input}");
        }
        assert!(extract_proof_json(&vec![b' '; MAX_PROOF_FILE_BYTES + 1]).is_err());
    }

    #[test]
    fn dates_are_utc_calendar_dates() {
        assert_eq!(utc_text(0), "1970-01-01 00:00 UTC");
        assert_eq!(utc_text(951_782_400), "2000-02-29 00:00 UTC");
        assert_eq!(utc_text(1_791_691_200), "2026-10-11 04:00 UTC");
    }
}
