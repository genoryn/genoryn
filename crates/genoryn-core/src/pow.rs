use crate::codec::Hash;
use anyhow::{Context, Result, ensure};
use randomx_rs::{RandomXCache, RandomXDataset, RandomXFlag, RandomXVM};
use std::cell::RefCell;

// The native VM is thread-confined. We deliberately do not assert Send/Sync.
thread_local! { static VM: RefCell<Option<(Hash, RandomXVM)>> = const { RefCell::new(None) }; }

fn preferred_flags() -> RandomXFlag {
    execution_flags(
        RandomXFlag::get_recommended_flags(),
        std::env::var_os("GENORYN_RANDOMX_PORTABLE").is_some_and(|value| value == "1"),
    )
}
fn execution_flags(recommended: RandomXFlag, portable: bool) -> RandomXFlag {
    if portable {
        return RandomXFlag::FLAG_DEFAULT;
    }
    // Execution optimization only. Never enable a new algorithm, full-memory
    // mode, or privileged large pages through an upstream recommendation.
    let allowed = RandomXFlag::FLAG_HARD_AES
        | RandomXFlag::FLAG_JIT
        | RandomXFlag::FLAG_ARGON2_SSSE3
        | RandomXFlag::FLAG_ARGON2_AVX2;
    let mut flags = recommended & allowed;
    if flags.contains(RandomXFlag::FLAG_JIT) {
        flags |= RandomXFlag::FLAG_SECURE;
    }
    flags
}
fn create_vm(seed: &Hash, flags: RandomXFlag) -> Result<RandomXVM> {
    let cache = RandomXCache::new(flags, seed)?;
    Ok(RandomXVM::new(flags, Some(cache), None)?)
}
pub fn randomx(seed: Hash, input: &[u8]) -> Result<Hash> {
    VM.with(|slot| {
        let mut slot = slot.borrow_mut();
        if slot.as_ref().is_none_or(|(key, _)| *key != seed) {
            let vm = create_vm(&seed, preferred_flags())
                .or_else(|_| create_vm(&seed, RandomXFlag::FLAG_DEFAULT))?;
            *slot = Some((seed, vm));
        }
        let result = slot
            .as_ref()
            .context("RandomX VM unavailable")?
            .1
            .calculate_hash(input)?;
        result
            .try_into()
            .map_err(|_| anyhow::anyhow!("invalid RandomX output length"))
    })
}

/// Full-memory ("fast") RandomX for mining: one dataset of about 2 GiB per PoW
/// seed, shared by every hashing thread. Hashes are identical to [`randomx`], only
/// several times faster; nodes keep verifying in light mode.
#[derive(Clone)]
pub struct FastDataset {
    seed: Hash,
    flags: RandomXFlag,
    dataset: RandomXDataset,
}
impl FastDataset {
    /// Builds the dataset for `seed` on the calling thread, which takes about a minute.
    pub fn new(seed: Hash) -> Result<Self> {
        let flags = preferred_flags();
        let cache = RandomXCache::new(flags, &seed)?;
        let dataset = RandomXDataset::new(RandomXFlag::FLAG_DEFAULT, cache, 0)?;
        Ok(Self {
            seed,
            flags: full_memory_flags(flags),
            dataset,
        })
    }
    pub fn seed(&self) -> Hash {
        self.seed
    }
    /// A VM for the calling thread; like the light-mode VM it never leaves it.
    pub fn vm(&self) -> Result<FastVm> {
        let vm = RandomXVM::new(self.flags, None, Some(self.dataset.clone())).or_else(|_| {
            RandomXVM::new(
                full_memory_flags(RandomXFlag::FLAG_DEFAULT),
                None,
                Some(self.dataset.clone()),
            )
        })?;
        Ok(FastVm(vm))
    }
}
fn full_memory_flags(execution: RandomXFlag) -> RandomXFlag {
    execution | RandomXFlag::FLAG_FULL_MEM
}
pub struct FastVm(RandomXVM);
impl FastVm {
    pub fn hash(&self, input: &[u8]) -> Result<Hash> {
        self.0
            .calculate_hash(input)?
            .try_into()
            .map_err(|_| anyhow::anyhow!("invalid RandomX output length"))
    }
}

/// Little-endian 256-bit hash multiplied by difficulty must fit in 256 bits.
/// This is equivalent to H <= floor((2^256-1)/difficulty), with no floating point.
pub fn meets_difficulty(hash: Hash, difficulty: u64) -> bool {
    if difficulty == 0 {
        return false;
    }
    let mut carry = 0u128;
    for limb in hash.chunks_exact(8) {
        let value = u64::from_le_bytes(limb.try_into().expect("fixed chunk size"));
        let product = value as u128 * difficulty as u128 + carry;
        carry = product >> 64;
    }
    carry == 0
}
pub fn verify(seed: Hash, input: &[u8], difficulty: u64) -> Result<Hash> {
    ensure!(difficulty > 0, "zero difficulty");
    let result = randomx(seed, input)?;
    ensure!(
        meets_difficulty(result, difficulty),
        "invalid proof of work"
    );
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn explicit_portable_mode_and_resource_allowlist() {
        let input = RandomXFlag::FLAG_HARD_AES
            | RandomXFlag::FLAG_JIT
            | RandomXFlag::FLAG_LARGE_PAGES
            | RandomXFlag::FLAG_FULL_MEM
            | RandomXFlag::FLAG_ARGON2;
        assert_eq!(execution_flags(input, true), RandomXFlag::FLAG_DEFAULT);
        let selected = execution_flags(input, false);
        assert!(selected.contains(RandomXFlag::FLAG_JIT | RandomXFlag::FLAG_SECURE));
        assert!(!selected.intersects(RandomXFlag::FLAG_LARGE_PAGES | RandomXFlag::FLAG_FULL_MEM));
        let fast = full_memory_flags(selected);
        assert_eq!(fast & !RandomXFlag::FLAG_FULL_MEM, selected);
        assert!(!fast.contains(RandomXFlag::FLAG_LARGE_PAGES));
        assert_eq!(
            full_memory_flags(execution_flags(input, true)),
            RandomXFlag::FLAG_FULL_MEM
        );
    }
    #[test]
    #[ignore = "builds the 2 GiB full-memory dataset, about a minute"]
    fn fast_mode_matches_light_hashes_on_every_thread() {
        let seed = [42u8; 32];
        let dataset = FastDataset::new(seed).unwrap();
        assert_eq!(dataset.seed(), seed);
        let inputs: Vec<Vec<u8>> = vec![
            b"GENORYN execution-mode vector".to_vec(),
            b"different input".to_vec(),
            vec![0x5a; 80],
            vec![0; 140],
        ];
        let expected: Vec<Hash> = inputs.iter().map(|i| randomx(seed, i).unwrap()).collect();
        let vm = dataset.vm().unwrap();
        for (input, expected) in inputs.iter().zip(&expected) {
            assert_eq!(&vm.hash(input).unwrap(), expected);
        }
        let workers: Vec<_> = (0..4)
            .map(|_| {
                let (dataset, inputs, expected) =
                    (dataset.clone(), inputs.clone(), expected.clone());
                std::thread::spawn(move || {
                    let vm = dataset.vm().unwrap();
                    for (input, expected) in inputs.iter().zip(&expected) {
                        assert_eq!(&vm.hash(input).unwrap(), expected);
                    }
                })
            })
            .collect();
        drop(dataset);
        for worker in workers {
            worker.join().unwrap();
        }
        assert_ne!(
            vm.hash(&inputs[0]).unwrap(),
            randomx([43; 32], &inputs[0]).unwrap()
        );
    }
    #[test]
    fn integer_target_boundaries() {
        assert!(meets_difficulty([255; 32], 1));
        assert!(!meets_difficulty([255; 32], 2));
        assert!(meets_difficulty([0; 32], u64::MAX));
        assert!(!meets_difficulty([0; 32], 0));
        let mut half = [255; 32];
        half[31] = 127;
        assert!(meets_difficulty(half, 2));
        half[31] = 128;
        assert!(!meets_difficulty(half, 2));
    }
    #[test]
    fn real_randomx_deterministic() {
        let a = randomx([42; 32], b"GENORYN consensus vector 1").unwrap();
        assert_eq!(a, randomx([42; 32], b"GENORYN consensus vector 1").unwrap());
        assert_ne!(a, randomx([42; 32], b"GENORYN consensus vector 2").unwrap());
    }
    #[test]
    fn optimized_light_mode_matches_portable_hashes() {
        let flags = preferred_flags();
        assert!(!flags.intersects(RandomXFlag::FLAG_FULL_MEM | RandomXFlag::FLAG_LARGE_PAGES));
        assert!(!flags.contains(RandomXFlag::FLAG_JIT) || flags.contains(RandomXFlag::FLAG_SECURE));
        for seed in [[0u8; 32], [42u8; 32]] {
            let portable = create_vm(&seed, RandomXFlag::FLAG_DEFAULT).unwrap();
            let optimized = create_vm(&seed, flags).unwrap();
            let inputs = [
                b"GENORYN execution-mode vector".as_slice(),
                b"different input".as_slice(),
                &[0x5a; 80],
            ];
            let expected: Vec<_> = inputs
                .iter()
                .map(|input| portable.calculate_hash(input).unwrap())
                .collect();
            for (input, expected) in inputs.iter().zip(&expected) {
                assert_eq!(expected, &optimized.calculate_hash(input).unwrap());
            }
            assert_eq!(portable.calculate_hash_set(&inputs).unwrap(), expected);
            assert_eq!(optimized.calculate_hash_set(&inputs).unwrap(), expected);
        }
    }
}
