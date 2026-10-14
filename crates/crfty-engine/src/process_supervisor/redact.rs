//! Replace known paths across read boundaries before bounding captured diagnostics.

#[derive(Default)]
pub(super) struct Redactor {
    patterns: Vec<(Vec<u8>, Vec<u8>)>,
    pending: Vec<u8>,
}
impl Redactor {
    pub(super) fn new(patterns: Vec<(Vec<u8>, Vec<u8>)>) -> Self {
        let mut patterns: Vec<_> = patterns
            .into_iter()
            .filter(|(text, _)| !text.is_empty())
            .collect();
        patterns.sort_by_key(|(text, _)| std::cmp::Reverse(text.len()));
        Self {
            patterns,
            pending: Vec::new(),
        }
    }
    pub(super) fn push(&mut self, bytes: &[u8], eof: bool) -> Vec<u8> {
        self.pending.extend_from_slice(bytes);
        let mut output = Vec::new();
        let mut cursor = 0;
        'scan: while let Some(remaining) = self.pending.get(cursor..)
            && !remaining.is_empty()
        {
            for (text, replacement) in &self.patterns {
                if remaining.starts_with(text) {
                    output.extend_from_slice(replacement);
                    cursor += text.len();
                    continue 'scan;
                }
                if text.starts_with(remaining) {
                    if eof {
                        output.extend_from_slice(replacement);
                        cursor += remaining.len();
                    }
                    break 'scan;
                }
            }
            if let Some(byte) = remaining.first() {
                output.push(*byte);
            }
            cursor += 1;
        }
        self.pending.drain(..cursor);
        output
    }
}
#[cfg(test)]
mod tests {
    use super::Redactor;
    #[test]
    fn every_read_split_scrubs_source_and_sample_paths_before_capture() {
        let input = "/videos/private name.mp4";
        let text = "error /videos/private name.mp4 and /tmp/search/.ab-av1-random/private name.av1.crf34.12.mkv";
        for split in 0..=text.len() {
            let mut redact = Redactor::new(vec![
                (input.into(), "<input>".into()),
                ("private name".into(), "<media>".into()),
                ("/tmp/search".into(), "<temporary>".into()),
            ]);
            let (head, tail) = text.as_bytes().split_at(split);
            let mut result = redact.push(head, false);
            result.extend(redact.push(tail, true));
            assert_eq!(
                String::from_utf8_lossy(&result),
                "error <input> and <temporary>/.ab-av1-random/<media>.av1.crf34.12.mkv"
            );
        }
    }
    #[test]
    fn native_bytes_and_truncated_path_prefixes_are_redacted() {
        let path = b"/videos/private-\xff-name.mkv";
        for length in 1..=path.len() {
            let mut redact = Redactor::new(vec![(path.to_vec(), b"<input>".to_vec())]);
            let prefix = path.get(..length).unwrap_or_default();
            let mut output = Vec::new();
            for byte in prefix {
                output.extend(redact.push(&[*byte], false));
            }
            output.extend(redact.push(&[], true));
            assert_eq!(output, b"<input>");
        }
    }
}
