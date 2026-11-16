//! Small I/O helpers shared by the protocol reader and the session scanner.

use std::io::BufRead;

/// Read one `\n`-terminated line into `buf`, keeping at most `cap` bytes.
/// Returns the line's full byte length; a value above `cap` means it was
/// truncated. Consumes the rest of the line without buffering it.
pub(crate) fn read_line_capped<R: BufRead>(
    reader: &mut R,
    buf: &mut Vec<u8>,
    cap: usize,
) -> std::io::Result<usize> {
    buf.clear();
    let mut total = 0usize;
    loop {
        let available = reader.fill_buf()?;
        if available.is_empty() {
            return Ok(total);
        }
        let newline = available.iter().position(|byte| *byte == b'\n');
        let take = newline.map_or(available.len(), |index| index + 1);
        if buf.len() < cap {
            let room = cap - buf.len();
            buf.extend_from_slice(&available[..take.min(room)]);
        }
        reader.consume(take);
        total += take;
        if newline.is_some() {
            return Ok(total);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keeps_only_the_capped_prefix() {
        let input = b"1234567890\nok\n";
        let mut reader: &[u8] = input;
        let mut buf = Vec::new();
        assert_eq!(read_line_capped(&mut reader, &mut buf, 4).unwrap(), 11);
        assert_eq!(buf, b"1234");
        assert_eq!(read_line_capped(&mut reader, &mut buf, 4).unwrap(), 3);
        assert_eq!(buf, b"ok\n");
        assert_eq!(read_line_capped(&mut reader, &mut buf, 4).unwrap(), 0);
    }
}
