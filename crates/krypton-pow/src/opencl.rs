//! OpenCL GPU backend (AMD, Intel and NVIDIA). The OpenCL runtime is loaded
//! dynamically; the kernel is compiled at startup from the shared core.
#![allow(unsafe_code)]

use crate::cpu::{Job, Solution};
use crate::{Cache, PowParams, hash_le_cmp_target, hash_light};
use opencl3::command_queue::{CL_QUEUE_PROFILING_ENABLE, CommandQueue};
use opencl3::context::Context;
use opencl3::device::{CL_DEVICE_TYPE_GPU, Device, get_all_devices};
use opencl3::kernel::{ExecuteKernel, Kernel};
use opencl3::memory::{Buffer, CL_MEM_READ_ONLY, CL_MEM_READ_WRITE};
use opencl3::program::Program;
use opencl3::types::{CL_BLOCKING, cl_uint, cl_ulong};
use std::ptr;

const CORE: &str = include_str!("../kernels/kh_core.h");
const KERNELS: &str = include_str!("../kernels/kryptohash.cl");
const MAX_RESULTS: u32 = 8;

#[derive(Debug, thiserror::Error)]
pub enum ClError {
    #[error("OpenCL: {0}")]
    Cl(String),
    #[error("GPU result mismatch")]
    Mismatch,
}

fn e<E: std::fmt::Debug>(x: E) -> ClError {
    ClError::Cl(format!("{x:?}"))
}

pub fn devices() -> Result<Vec<(usize, String, u64)>, ClError> {
    let ids = std::panic::catch_unwind(|| get_all_devices(CL_DEVICE_TYPE_GPU))
        .map_err(|_| ClError::Cl("OpenCL runtime not found".into()))?
        .map_err(e)?;
    Ok(ids
        .iter()
        .enumerate()
        .map(|(i, id)| {
            let d = Device::new(*id);
            (
                i,
                d.name().unwrap_or_default(),
                d.global_mem_size().unwrap_or(0),
            )
        })
        .collect())
}

pub struct ClMiner {
    /// Index in the list of OpenCL GPUs (as in `--list-devices`).
    pub index: usize,
    pub name: String,
    context: Context,
    queue: CommandQueue,
    k_dataset: Kernel,
    k_search: Kernel,
    dataset: Option<(u64, Buffer<cl_uint>, u32)>,
    results: Buffer<cl_uint>,
    header: Buffer<cl_uint>,
    target: Buffer<cl_uint>,
}

impl ClMiner {
    pub fn new(index: usize) -> Result<Self, ClError> {
        let ids = get_all_devices(CL_DEVICE_TYPE_GPU).map_err(e)?;
        let id = *ids
            .get(index)
            .ok_or(ClError::Cl("no such OpenCL GPU".into()))?;
        let device = Device::new(id);
        let name = device.name().map_err(e)?;
        let context = Context::from_device(&device).map_err(e)?;
        #[allow(deprecated)]
        let queue = CommandQueue::create_default(&context, CL_QUEUE_PROFILING_ENABLE).map_err(e)?;
        // `static inline`: a plain C99 `inline` has no external definition, and NVIDIA's
        // OpenCL compiler fails to link the helpers when it does not inline them.
        let src = format!("#define KH_FN static inline\n#define KH_G __global\n{CORE}\n{KERNELS}");
        let program = Program::create_and_build_from_source(&context, &src, "-cl-std=CL1.2")
            .map_err(|log| {
                ClError::Cl(if log.trim().is_empty() || log.contains("BUILD_PROGRAM_FAILURE") {
                    format!("{log}: the GPU driver could not compile the mining kernel; reinstall the driver (a clean install) and try again")
                } else {
                    log
                })
            })?;
        let k_dataset = Kernel::create(&program, "kh_dataset").map_err(e)?;
        let k_search = Kernel::create(&program, "kh_search").map_err(e)?;
        let results = unsafe {
            Buffer::<cl_uint>::create(
                &context,
                CL_MEM_READ_WRITE,
                1 + 2 * MAX_RESULTS as usize,
                ptr::null_mut(),
            )
        }
        .map_err(e)?;
        let header =
            unsafe { Buffer::<cl_uint>::create(&context, CL_MEM_READ_ONLY, 8, ptr::null_mut()) }
                .map_err(e)?;
        let target =
            unsafe { Buffer::<cl_uint>::create(&context, CL_MEM_READ_ONLY, 8, ptr::null_mut()) }
                .map_err(e)?;
        Ok(Self {
            index,
            name,
            context,
            queue,
            k_dataset,
            k_search,
            dataset: None,
            results,
            header,
            target,
        })
    }

    pub fn epoch(&self) -> Option<u64> {
        self.dataset.as_ref().map(|d| d.0)
    }

    pub fn prepare(&mut self, params: &PowParams, cache: &Cache) -> Result<(), ClError> {
        if self.epoch() == Some(cache.epoch) {
            return Ok(());
        }
        self.dataset = None;
        let items = params.dataset_items(cache.epoch) as u32;
        let words = cache.words();
        let mut cbuf = unsafe {
            Buffer::<cl_uint>::create(
                &self.context,
                CL_MEM_READ_ONLY,
                words.len(),
                ptr::null_mut(),
            )
        }
        .map_err(e)?;
        unsafe {
            self.queue
                .enqueue_write_buffer(&mut cbuf, CL_BLOCKING, 0, &words, &[])
        }
        .map_err(e)?;
        let ds = unsafe {
            Buffer::<cl_uint>::create(
                &self.context,
                CL_MEM_READ_WRITE,
                items as usize * 16,
                ptr::null_mut(),
            )
        }
        .map_err(e)?;
        let n = cache.items.len() as cl_uint;
        let chunk: u32 = 1 << 18;
        let mut start = 0u32;
        while start < items {
            let count = chunk.min(items - start);
            unsafe {
                ExecuteKernel::new(&self.k_dataset)
                    .set_arg(&cbuf)
                    .set_arg(&n)
                    .set_arg(&ds)
                    .set_arg(&start)
                    .set_arg(&count)
                    .set_global_work_size(count.div_ceil(256) as usize * 256)
                    .set_local_work_size(256)
                    .enqueue_nd_range(&self.queue)
            }
            .map_err(e)?;
            start += count;
        }
        self.queue.finish().map_err(e)?;
        self.dataset = Some((cache.epoch, ds, items / 2));
        Ok(())
    }

    pub fn search(&mut self, job: &Job, count: u32) -> Result<Vec<u64>, ClError> {
        let (_, ds, pairs) = self
            .dataset
            .as_ref()
            .ok_or(ClError::Cl("dataset not prepared".into()))?;
        let pairs = *pairs;
        let tgt: Vec<u32> = job
            .target
            .iter()
            .flat_map(|l| [*l as u32, (*l >> 32) as u32])
            .collect();
        let zero = vec![0u32; 1 + 2 * MAX_RESULTS as usize];
        unsafe {
            self.queue
                .enqueue_write_buffer(&mut self.header, CL_BLOCKING, 0, &job.header, &[])
                .map_err(e)?;
            self.queue
                .enqueue_write_buffer(&mut self.target, CL_BLOCKING, 0, &tgt, &[])
                .map_err(e)?;
            self.queue
                .enqueue_write_buffer(&mut self.results, CL_BLOCKING, 0, &zero, &[])
                .map_err(e)?;
        }
        let start: cl_ulong = job.start_nonce;
        let max: cl_uint = MAX_RESULTS;
        unsafe {
            ExecuteKernel::new(&self.k_search)
                .set_arg(ds)
                .set_arg(&pairs)
                .set_arg(&self.header)
                .set_arg(&self.target)
                .set_arg(&start)
                .set_arg(&self.results)
                .set_arg(&max)
                .set_global_work_size(count.div_ceil(256) as usize * 256)
                .set_local_work_size(256)
                .enqueue_nd_range(&self.queue)
        }
        .map_err(e)?;
        let mut r = vec![0u32; 1 + 2 * MAX_RESULTS as usize];
        unsafe {
            self.queue
                .enqueue_read_buffer(&self.results, CL_BLOCKING, 0, &mut r, &[])
        }
        .map_err(e)?;
        let found = (r[0] as usize).min(MAX_RESULTS as usize);
        Ok((0..found)
            .map(|i| r[1 + 2 * i] as u64 | ((r[2 + 2 * i] as u64) << 32))
            .collect())
    }

    pub fn search_verified(
        &mut self,
        job: &Job,
        count: u32,
        cache: &Cache,
    ) -> Result<Option<Solution>, ClError> {
        let pairs = self.dataset.as_ref().map(|d| d.2).unwrap_or(0);
        let Some(&nonce) = self.search(job, count)?.first() else {
            return Ok(None);
        };
        let h = hash_light(cache, pairs, &job.header, nonce);
        if hash_le_cmp_target(&h, &job.target) {
            Ok(Some(Solution { nonce, hash: h }))
        } else {
            Err(ClError::Mismatch)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::header_words;
    #[test]
    fn opencl_finds_cpu_valid_solutions() {
        let Ok(devs) = devices() else { return };
        if devs.is_empty() {
            return;
        }
        let p = PowParams::DEVNET;
        let cache = Cache::new(&p, &[8; 32], 0);
        let mut m = ClMiner::new(0).unwrap();
        m.prepare(&p, &cache).unwrap();
        let job = Job {
            header: header_words(&[0x42; 32]),
            target: crate::target_from_difficulty(2000),
            start_nonce: 0,
        };
        let sol = m
            .search_verified(&job, 1 << 16, &cache)
            .unwrap()
            .expect("solution");
        assert!(crate::meets_difficulty(&sol.hash, 2000));
    }
}
