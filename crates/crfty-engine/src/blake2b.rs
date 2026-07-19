const BLOCK_BYTES: usize = 128;
const DIGEST_BYTES: usize = 16;
const WORD_BYTES: usize = 8;
const STATE_WORDS: usize = 8;
const BLOCK_WORDS: usize = 16;
const ROUND_COUNT: usize = 12;
const MIX_WORDS: usize = 4;
const MIXES_PER_ROUND: usize = 8;
const PARAMETER_BLOCK: u64 = 0x0101_0000 ^ DIGEST_BYTES as u64;
const FIRST_ROTATION_BITS: u32 = 32;
const SECOND_ROTATION_BITS: u32 = 24;
const THIRD_ROTATION_BITS: u32 = 16;
const FOURTH_ROTATION_BITS: u32 = 63;
const COUNTER_LOW_WORD: usize = 12;
const COUNTER_HIGH_WORD: usize = 13;
const FINAL_FLAG_WORD: usize = 14;

// Initialization vector and message schedule are the BLAKE2b constants from RFC 7693.
const INITIALIZATION_VECTOR: [u64; STATE_WORDS] = [
    0x6a09_e667_f3bc_c908,
    0xbb67_ae85_84ca_a73b,
    0x3c6e_f372_fe94_f82b,
    0xa54f_f53a_5f1d_36f1,
    0x510e_527f_ade6_82d1,
    0x9b05_688c_2b3e_6c1f,
    0x1f83_d9ab_fb41_bd6b,
    0x5be0_cd19_137e_2179,
];

const MESSAGE_SCHEDULE: [[usize; BLOCK_WORDS]; ROUND_COUNT] = [
    [0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15],
    [14, 10, 4, 8, 9, 15, 13, 6, 1, 12, 0, 2, 11, 7, 5, 3],
    [11, 8, 12, 0, 5, 2, 15, 13, 10, 14, 3, 6, 7, 1, 9, 4],
    [7, 9, 3, 1, 13, 12, 11, 14, 2, 6, 5, 10, 4, 0, 15, 8],
    [9, 0, 5, 7, 2, 4, 10, 15, 14, 1, 11, 12, 6, 8, 3, 13],
    [2, 12, 6, 10, 0, 11, 8, 3, 4, 13, 7, 5, 15, 14, 1, 9],
    [12, 5, 1, 15, 14, 13, 4, 10, 0, 7, 6, 3, 9, 2, 8, 11],
    [13, 11, 7, 14, 12, 1, 3, 9, 5, 0, 15, 4, 8, 6, 2, 10],
    [6, 15, 14, 9, 11, 3, 0, 8, 12, 2, 13, 7, 1, 4, 10, 5],
    [10, 2, 8, 4, 7, 6, 1, 5, 15, 11, 9, 14, 3, 12, 13, 0],
    [0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15],
    [14, 10, 4, 8, 9, 15, 13, 6, 1, 12, 0, 2, 11, 7, 5, 3],
];
const ROUND_MIXES: [([usize; MIX_WORDS], usize); MIXES_PER_ROUND] = [
    ([0, 4, 8, 12], 0),
    ([1, 5, 9, 13], 2),
    ([2, 6, 10, 14], 4),
    ([3, 7, 11, 15], 6),
    ([0, 5, 10, 15], 8),
    ([1, 6, 11, 12], 10),
    ([2, 7, 8, 13], 12),
    ([3, 4, 9, 14], 14),
];

pub(crate) struct Blake2b128 {
    state: [u64; STATE_WORDS],
    buffer: [u8; BLOCK_BYTES],
    buffered: usize,
    compressed_bytes: u128,
}

impl Blake2b128 {
    pub(crate) fn new() -> Self {
        let mut state = INITIALIZATION_VECTOR;
        if let Some(first) = state.first_mut() {
            *first ^= PARAMETER_BLOCK;
        }
        Self {
            state,
            buffer: [0; BLOCK_BYTES],
            buffered: 0,
            compressed_bytes: 0,
        }
    }

    pub(crate) fn update(&mut self, mut input: &[u8]) {
        while !input.is_empty() {
            if self.buffered == BLOCK_BYTES {
                self.compressed_bytes = self.compressed_bytes.saturating_add(BLOCK_BYTES as u128);
                compress(&mut self.state, &self.buffer, self.compressed_bytes, false);
                self.buffered = 0;
            }
            let take = BLOCK_BYTES.saturating_sub(self.buffered).min(input.len());
            let (source, remaining) = input.split_at(take);
            if let Some(destination) = self
                .buffer
                .get_mut(self.buffered..self.buffered.saturating_add(take))
            {
                destination.copy_from_slice(source);
            }
            self.buffered = self.buffered.saturating_add(take);
            input = remaining;
        }
    }

    pub(crate) fn finalize(mut self) -> [u8; DIGEST_BYTES] {
        self.compressed_bytes = self.compressed_bytes.saturating_add(self.buffered as u128);
        if let Some(padding) = self.buffer.get_mut(self.buffered..) {
            padding.fill(0);
        }
        compress(&mut self.state, &self.buffer, self.compressed_bytes, true);
        let mut digest = [0_u8; DIGEST_BYTES];
        for (destination, source) in digest
            .iter_mut()
            .zip(self.state.iter().flat_map(|word| word.to_le_bytes()))
        {
            *destination = source;
        }
        digest
    }
}

fn compress(
    state: &mut [u64; STATE_WORDS],
    block: &[u8; BLOCK_BYTES],
    count: u128,
    final_block: bool,
) {
    let mut message = [0_u64; BLOCK_WORDS];
    for (word, bytes) in message.iter_mut().zip(block.chunks_exact(WORD_BYTES)) {
        let mut encoded = [0_u8; WORD_BYTES];
        encoded.copy_from_slice(bytes);
        *word = u64::from_le_bytes(encoded);
    }
    let mut work = [0_u64; BLOCK_WORDS];
    let (left, right) = work.split_at_mut(state.len());
    left.copy_from_slice(state);
    right.copy_from_slice(&INITIALIZATION_VECTOR);
    if let Some(counter_low) = work.get_mut(COUNTER_LOW_WORD) {
        *counter_low ^= count as u64;
    }
    if let Some(counter_high) = work.get_mut(COUNTER_HIGH_WORD) {
        *counter_high ^= (count >> u64::BITS) as u64;
    }
    if final_block && let Some(flag) = work.get_mut(FINAL_FLAG_WORD) {
        *flag = !*flag;
    }
    for schedule in MESSAGE_SCHEDULE {
        round(&mut work, &message, &schedule);
    }
    let state_len = state.len();
    for (index, value) in state.iter_mut().enumerate() {
        let left = work.get(index).copied().unwrap_or_default();
        let right = work
            .get(index.saturating_add(state_len))
            .copied()
            .unwrap_or_default();
        *value ^= left ^ right;
    }
}

fn round(
    work: &mut [u64; BLOCK_WORDS],
    message: &[u64; BLOCK_WORDS],
    schedule: &[usize; BLOCK_WORDS],
) {
    for (positions, message_index) in ROUND_MIXES {
        mix(work, message, schedule, positions, message_index);
    }
}

fn mix(
    work: &mut [u64; BLOCK_WORDS],
    message: &[u64; BLOCK_WORDS],
    schedule: &[usize; BLOCK_WORDS],
    positions: [usize; MIX_WORDS],
    message_index: usize,
) {
    let Some(&x_index) = schedule.get(message_index) else {
        return;
    };
    let Some(&y_index) = schedule.get(message_index.saturating_add(1)) else {
        return;
    };
    let x = message.get(x_index).copied().unwrap_or_default();
    let y = message.get(y_index).copied().unwrap_or_default();
    let [a_pos, b_pos, c_pos, d_pos] = positions;
    let Ok([a, b, c, d]) = work.get_disjoint_mut([a_pos, b_pos, c_pos, d_pos]) else {
        return;
    };
    *a = a.wrapping_add(*b).wrapping_add(x);
    *d = (*d ^ *a).rotate_right(FIRST_ROTATION_BITS);
    *c = c.wrapping_add(*d);
    *b = (*b ^ *c).rotate_right(SECOND_ROTATION_BITS);
    *a = a.wrapping_add(*b).wrapping_add(y);
    *d = (*d ^ *a).rotate_right(THIRD_ROTATION_BITS);
    *c = c.wrapping_add(*d);
    *b = (*b ^ *c).rotate_right(FOURTH_ROTATION_BITS);
}

#[cfg(test)]
mod tests {
    use super::{BLOCK_BYTES, Blake2b128};

    #[test]
    fn matches_published_blake2b_128_vectors() {
        assert_eq!(hex(b""), "cae66941d9efbd404e4d88758ea67670");
        assert_eq!(hex(b"abc"), "cf4ab791c62b8d2b2109c90275287816");
        assert_eq!(hex(&[0; BLOCK_BYTES]), "40d697295c01d2312035c1fa7d884b8d");
        assert_eq!(
            hex(&[0; BLOCK_BYTES + 1]),
            "dcd04926f5dafbeb0bb326f1915d1066"
        );
        assert_eq!(
            hex(&vec![0; BLOCK_BYTES * 3]),
            "776903ae2e93f55f20096a50fa92cab2"
        );
    }

    #[test]
    fn chunk_boundaries_do_not_change_the_digest() {
        let input = vec![0x5a; BLOCK_BYTES * 3];
        let expected = hex(&input);
        let mut hasher = Blake2b128::new();
        for chunk in input.chunks(17) {
            hasher.update(chunk);
        }
        let chunked: String = hasher
            .finalize()
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect();
        assert_eq!(chunked, expected);
    }

    fn hex(input: &[u8]) -> String {
        let mut hasher = Blake2b128::new();
        hasher.update(input);
        hasher
            .finalize()
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect()
    }
}
