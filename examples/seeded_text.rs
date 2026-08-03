use std::io::{self, BufWriter, Write};

use clap::Parser;

const DEFAULT_BYTES: u64 = 400 * 1024 * 1024;
const DEFAULT_SEED: u64 = 7_318_615_769_902_165_317;
const LEXICON_SIZE: usize = 200;

#[derive(Debug, Parser)]
#[command(about = "Generate deterministic, highly compressible text")]
struct Args {
    /// Minimum number of bytes to write. Output ends after the next full line.
    #[arg(long, default_value_t = DEFAULT_BYTES)]
    bytes: u64,

    /// Seed for lexicon and line generation.
    #[arg(long, default_value_t = DEFAULT_SEED)]
    seed: u64,
}

#[derive(Clone)]
struct SplitMix64 {
    state: u64,
}

impl SplitMix64 {
    fn new(seed: u64) -> Self {
        Self { state: seed }
    }

    fn next(&mut self) -> u64 {
        self.state = self.state.wrapping_add(0x9e37_79b9_7f4a_7c15);
        let mut value = self.state;
        value = (value ^ (value >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
        value = (value ^ (value >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
        value ^ (value >> 31)
    }

    fn below(&mut self, limit: usize) -> usize {
        (self.next() % limit as u64) as usize
    }
}

fn make_lexicon(rng: &mut SplitMix64) -> Vec<Vec<u8>> {
    (0..LEXICON_SIZE)
        .map(|_| {
            let length = 1 + rng.below(10);
            (0..length)
                .map(|_| if rng.next() & 1 == 0 { b'a' } else { b'b' })
                .collect()
        })
        .collect()
}

fn generate(mut output: impl Write, minimum_bytes: u64, seed: u64) -> io::Result<u64> {
    let mut rng = SplitMix64::new(seed);
    let lexicon = make_lexicon(&mut rng);
    let mut line = Vec::with_capacity(1001);
    let mut written = 0_u64;

    while written < minimum_bytes {
        line.clear();
        let target = 200 + rng.below(801);

        loop {
            let word = &lexicon[rng.below(lexicon.len())];
            let separator = usize::from(!line.is_empty());
            let candidate_length = line.len() + separator + word.len();
            if line.len() >= 200 && candidate_length > target {
                break;
            }
            if separator != 0 {
                line.push(b' ');
            }
            line.extend_from_slice(word);
        }

        debug_assert!((200..=1000).contains(&line.len()));
        line.push(b'\n');
        output.write_all(&line)?;
        written += line.len() as u64;
    }

    Ok(written)
}

fn main() -> io::Result<()> {
    let args = Args::parse();
    let stdout = io::stdout().lock();
    let mut output = BufWriter::with_capacity(1024 * 1024, stdout);
    let written = generate(&mut output, args.bytes, args.seed)?;
    output.flush()?;
    eprintln!("seed={} bytes={written}", args.seed);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lexicon_has_the_requested_shape() {
        let mut rng = SplitMix64::new(DEFAULT_SEED);
        let lexicon = make_lexicon(&mut rng);
        assert_eq!(lexicon.len(), LEXICON_SIZE);
        assert!(lexicon.iter().all(|word| (1..=10).contains(&word.len())));
        assert!(
            lexicon
                .iter()
                .flatten()
                .all(|letter| matches!(letter, b'a' | b'b'))
        );
    }

    #[test]
    fn output_is_deterministic_and_line_bounded() {
        let mut first = Vec::new();
        let mut second = Vec::new();
        let requested = 32 * 1024;
        let first_length = generate(&mut first, requested, DEFAULT_SEED).expect("generate first");
        let second_length =
            generate(&mut second, requested, DEFAULT_SEED).expect("generate second");

        assert_eq!(first, second);
        assert_eq!(first_length, second_length);
        assert!(first_length >= requested);
        assert!(first_length <= requested + 1001);
        assert_eq!(first.last(), Some(&b'\n'));

        for line in first
            .split(|byte| *byte == b'\n')
            .filter(|line| !line.is_empty())
        {
            assert!((200..=1000).contains(&line.len()));
            assert!(line.iter().all(|byte| matches!(byte, b'a' | b'b' | b' ')));
            assert!(
                line.split(|byte| *byte == b' ')
                    .all(|word| !word.is_empty())
            );
        }
    }

    #[test]
    fn changing_the_seed_changes_the_output() {
        let mut first = Vec::new();
        let mut second = Vec::new();
        generate(&mut first, 4096, DEFAULT_SEED).expect("generate first");
        generate(&mut second, 4096, DEFAULT_SEED + 1).expect("generate second");
        assert_ne!(first, second);
    }
}
