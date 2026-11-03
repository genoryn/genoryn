//! NVIDIA GPU backend (CUDA driver API, loaded at runtime: no toolkit needed).
//! The kernel ships as PTX and is JIT-compiled by the driver for the installed GPU.
#![allow(unsafe_code)]

use crate::cpu::{Job, Solution};
use crate::{Cache, PowParams, hash_light, header_words};
use cudarc::driver::{
    CudaContext, CudaFunction, CudaSlice, CudaStream, LaunchConfig, PushKernelArg,
};
use cudarc::nvrtc::Ptx;
use std::sync::Arc;

const PTX: &str = include_str!("../kernels/kryptohash.ptx");
const MAX_RESULTS: u32 = 8;
const BLOCK: u32 = 256;

#[derive(Debug, thiserror::Error)]
pub enum GpuError {
    #[error("CUDA: {0}")]
    Cuda(String),
    #[error("GPU result mismatch: the GPU produced a hash that does not verify")]
    Mismatch,
}

fn e<E: std::fmt::Debug>(x: E) -> GpuError {
    GpuError::Cuda(format!("{x:?}"))
}

#[derive(Clone, Debug)]
pub struct DeviceInfo {
    pub ordinal: usize,
    pub name: String,
    pub memory_bytes: usize,
}

pub fn devices() -> Result<Vec<DeviceInfo>, GpuError> {
    let n = std::panic::catch_unwind(CudaContext::device_count)
        .map_err(|_| GpuError::Cuda("NVIDIA driver library not found".into()))?
        .map_err(e)?;
    let mut v = Vec::new();
    for ordinal in 0..n as usize {
        let ctx = CudaContext::new(ordinal).map_err(e)?;
        v.push(DeviceInfo {
            ordinal,
            name: ctx.name().map_err(e)?,
            memory_bytes: ctx.total_mem().map_err(e)?,
        });
    }
    Ok(v)
}

pub struct CudaMiner {
    pub info: DeviceInfo,
    stream: Arc<CudaStream>,
    f_dataset: CudaFunction,
    f_search: CudaFunction,
    f_batch: CudaFunction,
    dataset: Option<(u64, CudaSlice<u32>, u32)>,
    results: CudaSlice<u32>,
    header: CudaSlice<u32>,
    target: CudaSlice<u32>,
    cache_items: u32,
}

impl CudaMiner {
    pub fn new(ordinal: usize) -> Result<Self, GpuError> {
        let ctx = std::panic::catch_unwind(|| CudaContext::new(ordinal))
            .map_err(|_| GpuError::Cuda("NVIDIA driver library not found".into()))?
            .map_err(e)?;
        let info = DeviceInfo {
            ordinal,
            name: ctx.name().map_err(e)?,
            memory_bytes: ctx.total_mem().map_err(e)?,
        };
        let module = ctx.load_module(Ptx::from_src(PTX)).map_err(e)?;
        let stream = ctx.default_stream();
        let results = stream
            .alloc_zeros::<u32>(1 + 2 * MAX_RESULTS as usize)
            .map_err(e)?;
        let header = stream.alloc_zeros::<u32>(8).map_err(e)?;
        let target = stream.alloc_zeros::<u32>(8).map_err(e)?;
        Ok(Self {
            info,
            f_dataset: module.load_function("kh_dataset").map_err(e)?,
            f_search: module.load_function("kh_search").map_err(e)?,
            f_batch: module.load_function("kh_hash_batch").map_err(e)?,
            stream,
            dataset: None,
            results,
            header,
            target,
            cache_items: 0,
        })
    }

    pub fn epoch(&self) -> Option<u64> {
        self.dataset.as_ref().map(|d| d.0)
    }

    /// Builds the epoch dataset on the GPU from the (CPU-generated) cache.
    pub fn prepare(&mut self, params: &PowParams, cache: &Cache) -> Result<(), GpuError> {
        if self.epoch() == Some(cache.epoch) {
            return Ok(());
        }
        self.dataset = None;
        let items = params.dataset_items(cache.epoch) as u32;
        let cache_dev = self.stream.clone_htod(&cache.words()).map_err(e)?;
        let mut ds = self
            .stream
            .alloc_zeros::<u32>(items as usize * 16)
            .map_err(e)?;
        let n = cache.items.len() as u32;
        let chunk: u32 = 1 << 18;
        let mut start = 0u32;
        while start < items {
            let count = chunk.min(items - start);
            let cfg = LaunchConfig {
                grid_dim: (count.div_ceil(BLOCK), 1, 1),
                block_dim: (BLOCK, 1, 1),
                shared_mem_bytes: 0,
            };
            let mut b = self.stream.launch_builder(&self.f_dataset);
            b.arg(&cache_dev)
                .arg(&n)
                .arg(&mut ds)
                .arg(&start)
                .arg(&count);
            unsafe { b.launch(cfg) }.map_err(e)?;
            start += count;
        }
        self.stream.synchronize().map_err(e)?;
        self.cache_items = n;
        self.dataset = Some((cache.epoch, ds, items / 2));
        Ok(())
    }

    /// Searches `count` nonces starting at `job.start_nonce`.
    pub fn search(&mut self, job: &Job, count: u32) -> Result<Vec<u64>, GpuError> {
        let (_, ds, pairs) = self
            .dataset
            .as_ref()
            .ok_or(GpuError::Cuda("dataset not prepared".into()))?;
        let pairs = *pairs;
        let tgt: Vec<u32> = job
            .target
            .iter()
            .flat_map(|l| [*l as u32, (*l >> 32) as u32])
            .collect();
        self.stream
            .memcpy_htod(&job.header, &mut self.header)
            .map_err(e)?;
        self.stream.memcpy_htod(&tgt, &mut self.target).map_err(e)?;
        self.stream.memset_zeros(&mut self.results).map_err(e)?;
        let cfg = LaunchConfig {
            grid_dim: (count.div_ceil(BLOCK), 1, 1),
            block_dim: (BLOCK, 1, 1),
            shared_mem_bytes: 0,
        };
        let max = MAX_RESULTS;
        let mut b = self.stream.launch_builder(&self.f_search);
        b.arg(ds)
            .arg(&pairs)
            .arg(&self.header)
            .arg(&self.target)
            .arg(&job.start_nonce)
            .arg(&mut self.results)
            .arg(&max);
        unsafe { b.launch(cfg) }.map_err(e)?;
        let r = self.stream.clone_dtoh(&self.results).map_err(e)?;
        let found = (r[0] as usize).min(MAX_RESULTS as usize);
        Ok((0..found)
            .map(|i| r[1 + 2 * i] as u64 | ((r[2 + 2 * i] as u64) << 32))
            .collect())
    }

    /// Search and re-verify every candidate on the CPU with the light algorithm.
    pub fn search_verified(
        &mut self,
        job: &Job,
        count: u32,
        cache: &Cache,
    ) -> Result<Option<Solution>, GpuError> {
        let pairs = self.dataset.as_ref().map(|d| d.2).unwrap_or(0);
        let Some(&nonce) = self.search(job, count)?.first() else {
            return Ok(None);
        };
        let h = hash_light(cache, pairs, &job.header, nonce);
        if crate::hash_le_cmp_target(&h, &job.target) {
            Ok(Some(Solution { nonce, hash: h }))
        } else {
            Err(GpuError::Mismatch)
        }
    }

    /// Hashes `count` consecutive nonces (self-test against the CPU implementation).
    pub fn hash_batch(
        &mut self,
        header: &[u8; 32],
        start: u64,
        count: u32,
    ) -> Result<Vec<[u8; 32]>, GpuError> {
        let (_, ds, pairs) = self
            .dataset
            .as_ref()
            .ok_or(GpuError::Cuda("dataset not prepared".into()))?;
        let pairs = *pairs;
        let hw = header_words(header);
        self.stream.memcpy_htod(&hw, &mut self.header).map_err(e)?;
        let mut out = self
            .stream
            .alloc_zeros::<u32>(count as usize * 8)
            .map_err(e)?;
        let cfg = LaunchConfig {
            grid_dim: (count.div_ceil(BLOCK), 1, 1),
            block_dim: (BLOCK, 1, 1),
            shared_mem_bytes: 0,
        };
        let mut b = self.stream.launch_builder(&self.f_batch);
        b.arg(ds)
            .arg(&pairs)
            .arg(&self.header)
            .arg(&start)
            .arg(&count)
            .arg(&mut out);
        unsafe { b.launch(cfg) }.map_err(e)?;
        let v = self.stream.clone_dtoh(&out).map_err(e)?;
        Ok(v.chunks(8)
            .map(|c| {
                let mut h = [0u8; 32];
                for (i, w) in c.iter().enumerate() {
                    h[i * 4..i * 4 + 4].copy_from_slice(&w.to_le_bytes());
                }
                h
            })
            .collect())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn gpu_matches_cpu() {
        let Ok(devs) = devices() else { return };
        if devs.is_empty() {
            return;
        }
        let p = PowParams::DEVNET;
        let cache = Cache::new(&p, &[8; 32], 0);
        let mut m = CudaMiner::new(0).unwrap();
        m.prepare(&p, &cache).unwrap();
        let hdr = [0x42u8; 32];
        let gpu = m.hash_batch(&hdr, 1000, 64).unwrap();
        let pairs = p.dataset_pairs(0);
        for (k, g) in gpu.iter().enumerate() {
            assert_eq!(
                *g,
                hash_light(&cache, pairs, &header_words(&hdr), 1000 + k as u64),
                "nonce {}",
                1000 + k
            );
        }
        let job = Job {
            header: header_words(&hdr),
            target: crate::target_from_difficulty(2000),
            start_nonce: 0,
        };
        let sol = m
            .search_verified(&job, 1 << 16, &cache)
            .unwrap()
            .expect("solution within 65536 nonces");
        assert!(crate::meets_difficulty(&sol.hash, 2000));
    }
}
