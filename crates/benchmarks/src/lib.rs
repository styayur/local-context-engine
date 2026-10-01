//! Shared fixtures for the Local Context Engine benchmarks.
//!
//! The generated corpus is deterministic: the same seed always produces the
//! same one million names, so a benchmark number can be compared across
//! machines and across commits.

use windows_files::FileStore;

/// A tiny, reproducible pseudo random number generator.
///
/// Using `rand` here would add a dependency to the workspace just for
/// benchmarks; a xorshift is three lines and perfectly adequate for building a
/// name corpus.
#[derive(Debug, Clone, Copy)]
pub struct XorShift64(u64);

impl XorShift64 {
    /// Seed the generator. Seed zero is remapped, since xorshift cannot escape
    /// it.
    #[must_use]
    pub const fn new(seed: u64) -> Self {
        Self(if seed == 0 {
            0x9E37_79B9_7F4A_7C15
        } else {
            seed
        })
    }

    /// The next value from the generator.
    pub fn next_u64(&mut self) -> u64 {
        let mut value = self.0;
        value ^= value << 13;
        value ^= value >> 7;
        value ^= value << 17;
        self.0 = value;
        value
    }

    /// A value in `0..bound`.
    pub fn below(&mut self, bound: usize) -> usize {
        if bound == 0 {
            return 0;
        }
        (self.next_u64() % bound as u64) as usize
    }
}

/// The word pools the synthetic names are built from.
const STEMS: [&str; 20] = [
    "report",
    "notes",
    "invoice",
    "design",
    "index",
    "search",
    "config",
    "session",
    "project",
    "archive",
    "meeting",
    "budget",
    "roadmap",
    "release",
    "changelog",
    "readme",
    "rust",
    "python",
    "typescript",
    "database",
];

const EXTENSIONS: [&str; 12] = [
    "rs", "toml", "md", "json", "pdf", "txt", "png", "ts", "py", "log", "csv", "exe",
];

const DIRECTORIES: [&str; 16] = [
    "src", "docs", "build", "tests", "assets", "scripts", "crates", "apps", "tools", "vendor",
    "data", "logs", "tmp", "config", "public", "internal",
];

/// Build a store with exactly `entries` records, spread over `depth` levels.
///
/// Roughly one in six records is a directory, which mirrors a real project tree
/// closely enough for the matcher to behave realistically.
#[must_use]
pub fn synthetic_store(entries: usize, seed: u64) -> FileStore {
    let mut rng = XorShift64::new(seed);
    let mut store = FileStore::new();
    store.reserve(entries);

    let root = store.push_root('C');
    // A pool of directories to hang files off, so paths stay plausible.
    let mut directories: Vec<u32> = vec![root];
    let mut created = 1usize;
    let mut counter = 0u64;

    while created < entries {
        let parent = directories[rng.below(directories.len())];
        counter += 1;
        let stem = STEMS[rng.below(STEMS.len())];
        let name = format!("{stem}-{counter}");

        if rng.below(6) == 0 {
            let index = store.push_entry(
                parent,
                &name,
                'C',
                true,
                0,
                1_700_000_000_000 + (rng.next_u64() % 1_000_000_000) as i64,
                0,
                0,
            );
            directories.push(index);
        } else {
            let extension = EXTENSIONS[rng.below(EXTENSIONS.len())];
            let size = rng.next_u64() % 50_000_000;
            store.push_entry(
                parent,
                &format!("{name}.{extension}"),
                'C',
                false,
                size,
                1_700_000_000_000 + (rng.next_u64() % 1_000_000_000) as i64,
                0,
                0,
            );
        }
        created += 1;
    }

    store
}

/// Names a directory chain, used by the deep-path benchmarks.
#[must_use]
pub fn directory_name(index: usize) -> String {
    format!("{}-{index}", DIRECTORIES[index % DIRECTORIES.len()])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_generator_is_deterministic_and_not_stuck() {
        let mut first = XorShift64::new(42);
        let mut second = XorShift64::new(42);
        for _ in 0..8 {
            assert_eq!(first.next_u64(), second.next_u64());
        }
        let mut rng = XorShift64::new(7);
        let values: Vec<u64> = (0..8).map(|_| rng.next_u64()).collect();
        assert!(values.windows(2).any(|pair| pair[0] != pair[1]));
    }

    #[test]
    fn a_seed_of_zero_is_remapped() {
        let mut rng = XorShift64::new(0);
        assert_ne!(rng.next_u64(), 0);
    }

    #[test]
    fn a_small_corpus_has_the_requested_size() {
        let store = synthetic_store(500, 1);
        assert_eq!(store.len(), 500);
    }

    #[test]
    fn the_corpus_contains_both_files_and_directories() {
        let store = synthetic_store(2_000, 9);
        assert!(store.file_count() > 0);
        assert!(store.directory_count() > 1);
    }

    #[test]
    fn the_corpus_round_trips_through_the_arena() {
        let store = synthetic_store(200, 3);
        for index in 0..u32::try_from(store.len()).unwrap_or(0) {
            assert!(!store.name(index).is_empty());
        }
    }
}
