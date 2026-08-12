//! GPU timestamp queries: how long the GPU was actually busy.
//!
//! Added to settle a specific question, and it settled it in the *opposite*
//! direction to the one everyone expected — which is the argument for having
//! it.
//!
//! **The question.** The chaos loop runs ~12,288 dispatches/sec, about 82 µs
//! each, and `points_per_frame` is `buffer_capacity / 800` — a constant chosen
//! so the *interactive* ring cycles smoothly over 800 frames. An offline job
//! accumulating toward a sample target has no ring to cycle, so bigger batches
//! looked like free throughput: the same work in fewer submissions. But 82 µs
//! is wall clock around a submit-and-occasionally-poll cycle and cannot
//! separate fixed overhead from GPU time, so acting on it would have been
//! guessing.
//!
//! **What it measured**, on the reference desktop's GTX 1080:
//!
//! - There is real fixed overhead — about **0.031 ms per dispatch**, flat in
//!   batch size, in the submit-and-poll cycle. At the interactive rate that is
//!   44-64% of each dispatch's wall clock, which is exactly the waste the
//!   bigger-batch idea was aimed at.
//! - But GPU-busy time is **linear in the batch with no fixed GPU-side term at
//!   all** — 163,840 points in 0.089 ms, 819,200 in 0.579 ms — and if anything
//!   slightly *super*linear, so larger batches cost marginally more per point.
//!
//! **What happened when it was tried anyway.** Batching to a 2 ms GPU budget
//! (~4M points/dispatch) made the chaos fill **19% slower** end to end, not
//! faster. Swept at 1920x1080 / 100M points / accumulate 16384:
//!
//! | batch | chaos fill |
//! |---:|---:|
//! | 114,688 (the existing rate) | **1.47 s** |
//! | 229,376 | 1.59 s |
//! | 458,752 | 1.76 s |
//! | 917,504 | 1.78 s |
//! | 4,000,000 | 1.75 s |
//!
//! Monotonically worse, then flat. The batching was reverted; the measurement
//! that revealed it was kept. The plausible mechanism is locality — 16,384
//! walkers is a fixed amount of parallelism whatever the iteration count, so a
//! longer dispatch does not expose more of it, while the points it streams out
//! stop fitting in cache.
//!
//! **The untested follow-on**, recorded so it isn't re-derived: if there is a
//! lever here it is *more walkers*, not more iterations per walker.
//! `num_workgroups` is a hardcoded 64 (16,384 threads, ~6.4x oversubscription
//! on this GPU). Changing it changes walker seeding and therefore the image, so
//! it is not a free experiment.
//!
//! **Not guaranteed to exist.** `Features::TIMESTAMP_QUERY` is optional, so
//! every constructor here returns `None` rather than failing, and every caller
//! must work without it. Timing instrumentation that can refuse to run is worth
//! having; a renderer that refuses to run without it is not.

/// Timestamps are written at the boundaries of a pass, so each measured pass
/// costs two query slots.
const SLOTS_PER_PASS: u32 = 2;

pub struct GpuTimer {
    query_set: wgpu::QuerySet,
    /// `resolve_query_set` writes here (`QUERY_RESOLVE`, not mappable)
    resolve: wgpu::Buffer,
    /// ...and it is copied here to be read (`MAP_READ`, not resolvable)
    readback: wgpu::Buffer,
    /// Nanoseconds per timestamp tick, from `Queue::get_timestamp_period`
    period_ns: f32,
    capacity: u32,
    /// Next free slot. Measurement stops when the set fills rather than
    /// wrapping: a sample of the first N passes answers the question, and
    /// wrapping would silently mix two runs' numbers into one mean.
    used: u32,
}

impl GpuTimer {
    /// `passes` is how many passes can be measured before the set is full.
    ///
    /// `None` when the device was not created with `Features::TIMESTAMP_QUERY`
    /// — check with `device.features()`, since asking for a query set without
    /// it is a validation error rather than a soft failure.
    pub fn new(device: &wgpu::Device, queue: &wgpu::Queue, passes: u32) -> Option<Self> {
        if !device.features().contains(wgpu::Features::TIMESTAMP_QUERY) {
            return None;
        }
        let capacity = passes.max(1) * SLOTS_PER_PASS;
        let bytes = capacity as u64 * std::mem::size_of::<u64>() as u64;
        // Resolve destinations are aligned; round up rather than trusting the
        // caller's pass count to land on a multiple.
        let bytes = bytes.div_ceil(wgpu::QUERY_RESOLVE_BUFFER_ALIGNMENT)
            * wgpu::QUERY_RESOLVE_BUFFER_ALIGNMENT;
        Some(Self {
            query_set: device.create_query_set(&wgpu::QuerySetDescriptor {
                label: Some("gpu_timer_queries"),
                ty: wgpu::QueryType::Timestamp,
                count: capacity,
            }),
            resolve: device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("gpu_timer_resolve"),
                size: bytes,
                usage: wgpu::BufferUsages::QUERY_RESOLVE | wgpu::BufferUsages::COPY_SRC,
                mapped_at_creation: false,
            }),
            readback: device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("gpu_timer_readback"),
                size: bytes,
                usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
                mapped_at_creation: false,
            }),
            period_ns: queue.get_timestamp_period(),
            capacity,
            used: 0,
        })
    }

    /// Claim two slots for one compute pass, or `None` once the set is full.
    ///
    /// Returns the *descriptor field*, so a caller passes it straight to
    /// `begin_compute_pass` and needs to know nothing about slot bookkeeping.
    pub fn compute_pass(&mut self) -> Option<wgpu::ComputePassTimestampWrites<'_>> {
        let base = self.claim()?;
        Some(wgpu::ComputePassTimestampWrites {
            query_set: &self.query_set,
            beginning_of_pass_write_index: Some(base),
            end_of_pass_write_index: Some(base + 1),
        })
    }

    /// The same for a render pass.
    ///
    /// No caller yet: the chaos dispatch was the pass with a question attached
    /// to it. The splat accumulate, filter and tonemap passes are the obvious
    /// next things to measure, and this is what they will use — kept rather
    /// than deleted because a timer that can only measure compute passes would
    /// be a surprise to the next person who needs one.
    #[allow(dead_code)]
    pub fn render_pass(&mut self) -> Option<wgpu::RenderPassTimestampWrites<'_>> {
        let base = self.claim()?;
        Some(wgpu::RenderPassTimestampWrites {
            query_set: &self.query_set,
            beginning_of_pass_write_index: Some(base),
            end_of_pass_write_index: Some(base + 1),
        })
    }

    fn claim(&mut self) -> Option<u32> {
        if self.used + SLOTS_PER_PASS > self.capacity {
            return None;
        }
        let base = self.used;
        self.used += SLOTS_PER_PASS;
        Some(base)
    }

    /// How many passes have been measured so far.
    ///
    /// For a caller that wants to stop early once the sample is complete;
    /// `read` already reports the count, so nothing needs it yet.
    #[allow(dead_code)]
    pub fn measured(&self) -> u32 {
        self.used / SLOTS_PER_PASS
    }

    /// Whether the query set has room for another pass. Measuring past this
    /// point is a silent no-op, which is the safe behaviour but worth being
    /// able to ask about.
    #[allow(dead_code)]
    pub fn is_full(&self) -> bool {
        self.used + SLOTS_PER_PASS > self.capacity
    }

    /// Read the measured passes back, in milliseconds of GPU-busy time.
    ///
    /// Blocks on the queue: this is a measurement path, called once at the end
    /// of a run, not something on a frame's critical path.
    pub fn read(&self, device: &wgpu::Device, queue: &wgpu::Queue) -> Vec<f32> {
        if self.used == 0 {
            return Vec::new();
        }
        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("gpu_timer_resolve_encoder"),
        });
        encoder.resolve_query_set(&self.query_set, 0..self.used, &self.resolve, 0);
        let bytes = self.used as u64 * std::mem::size_of::<u64>() as u64;
        encoder.copy_buffer_to_buffer(&self.resolve, 0, &self.readback, 0, bytes);
        queue.submit(std::iter::once(encoder.finish()));

        let slice = self.readback.slice(..bytes);
        slice.map_async(wgpu::MapMode::Read, |_| {});
        let _ = device.poll(wgpu::PollType::Wait { submission_index: None, timeout: None });
        let out = {
            let data = slice.get_mapped_range();
            let ticks: Vec<u64> = data
                .chunks_exact(8)
                .map(|c| u64::from_le_bytes(c.try_into().expect("chunks_exact(8)")))
                .collect();
            ticks
                .chunks_exact(2)
                // `saturating_sub`: a pass whose end timestamp is not after its
                // beginning is a driver quirk, not a negative duration, and one
                // bad pair must not poison the mean by wrapping to 1.8e19.
                .map(|p| p[1].saturating_sub(p[0]) as f32 * self.period_ns / 1.0e6)
                .collect()
        };
        self.readback.unmap();
        out
    }
}

/// A one-line summary of a set of pass timings, for a `--gpu-timing` report.
///
/// Median rather than mean alone, and both printed: the first dispatch of a run
/// carries pipeline warmup and is not representative of the other twelve
/// thousand, so a mean that includes it overstates the steady state.
pub fn summarize(label: &str, ms: &[f32]) -> String {
    if ms.is_empty() {
        return format!("{}: no timings (device has no TIMESTAMP_QUERY)", label);
    }
    let mut sorted = ms.to_vec();
    sorted.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    let sum: f32 = ms.iter().sum();
    let mean = sum / ms.len() as f32;
    let median = sorted[sorted.len() / 2];
    format!(
        "{}: {} passes, {:.3} ms total, mean {:.4} ms, median {:.4} ms, min {:.4}, max {:.4}",
        label,
        ms.len(),
        sum,
        mean,
        median,
        sorted[0],
        sorted[sorted.len() - 1],
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_summary_of_nothing_says_why_rather_than_dividing_by_zero() {
        let s = summarize("chaos", &[]);
        assert!(s.contains("no timings"), "{}", s);
        assert!(s.contains("TIMESTAMP_QUERY"), "should say what is missing: {}", s);
    }

    /// The median is the number to read, so it has to be the median.
    #[test]
    fn a_summary_reports_both_mean_and_median() {
        // One slow first pass (pipeline warmup) among nine fast ones is exactly
        // the shape this has to describe honestly.
        let mut ms = vec![0.1f32; 9];
        ms.insert(0, 10.0);
        let s = summarize("chaos", &ms);
        assert!(s.contains("10 passes"), "{}", s);
        assert!(s.contains("median 0.1000"), "median must ignore the outlier: {}", s);
        assert!(s.contains("mean 1.0900"), "mean must include it: {}", s);
        assert!(s.contains("max 10.0000"), "{}", s);
    }
}
