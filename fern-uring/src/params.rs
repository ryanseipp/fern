//! Parameters for an `IoUring` instance.

use rustix::fd::BorrowedFd;
use rustix::io_uring::IoringSetupFlags;
#[cfg(doc)]
use rustix::io_uring::IoringSqFlags;

/// Configures the Linux kernel, SQ, CQ, and how `io_uring` handles certain
/// operations. Some options may result in performance improvements under
/// specific circumstances.
#[derive(Default, Debug, Clone, Copy)]
pub struct Params<'fd> {
    sq_entries: u32,
    cq_entries: u32,
    sq_thread_idle: u32,
    sq_thread_cpu: u32,
    work_queue_fd: Option<BorrowedFd<'fd>>,
    flags: IoringSetupFlags,
}

impl<'fd> Params<'fd> {
    /// Create a new Params instance
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Perform busy-waiting for an I/O completion, as opposed to getting notifications via an IRQ.
    ///
    /// The file system and block device must support polling in order for this to work.
    /// Busy-waiting provides lower latency, but may consume more CPU resources than interrupt
    /// driven I/O. Currently, this feature is only usable on a file descriptor opened using the
    /// `O_DIRECT` flag. When a read or write is submitted to a polled context, the application must
    /// poll for completions on the CQ ring. It is illegal to mix and match polled and non-polled
    /// I/O on an `io_uring` instance.
    ///
    /// This is only applicable for storage devices for now, and the storage device must be
    /// configured for polling. How to do that depends on the device type in question.
    #[must_use]
    pub const fn with_io_poll(mut self) -> Self {
        self.flags = self.flags.union(IoringSetupFlags::IOPOLL);

        self
    }

    /// Instruct the kernel to perform submission queue polling.
    ///
    /// An `io_uring` instance configured in this way enables an application to issue I/O without
    /// ever context switching into the kernel. By using the submission queue to fill in new
    /// submission queue entries and watching for completions on the completion queue, the
    /// application can submit and reap I/Os without doing a single system call.
    ///
    /// If the kernel thread is idle for more than `sq_thread_idle` milliseconds, it will set
    /// [`NEED_WAKEUP`] in `io_uring`'s SQ ring flags. When this happens, the application must
    /// enter the kernel to wake the kernel thread. If I/O is kept busy, the kernel thread will
    /// never sleep.
    ///
    /// [`NEED_WAKEUP`]: IoringSqFlags::NEED_WAKEUP
    #[must_use]
    pub const fn with_sq_poll(mut self, sq_thread_idle: Option<u32>) -> Self {
        self.flags = self.flags.union(IoringSetupFlags::SQPOLL);
        if let Some(sq_thread_idle) = sq_thread_idle {
            self.sq_thread_idle = sq_thread_idle;
        }

        self
    }

    /// Bind the poll thread to the CPU set in `sq_thread_cpu`.
    ///
    /// This option is only meaningful when SQ polling is enabled via [`Self::with_sq_poll`]. When the
    /// cgroup setting `cpuset.cpus` changes, the bound CPU set may be changed as well.
    #[must_use]
    pub const fn with_sq_affinity(mut self, sq_thread_cpu: u32) -> Self {
        self.flags = self.flags.union(IoringSetupFlags::SQ_AFF);
        self.sq_thread_cpu = sq_thread_cpu;

        self
    }

    /// Specify the number of submission queue entries. May be rounded to the next power of two.
    #[must_use]
    pub const fn with_sq_size(mut self, sq_size: u32) -> Self {
        self.sq_entries = sq_size.next_power_of_two();

        self
    }

    /// Specify the number of completion queue entries. Must be greater than the number of SQ
    /// entries, and may be rounded to the next power of two.
    #[must_use]
    pub const fn with_cq_size(mut self, cq_size: u32) -> Self {
        self.flags = self.flags.union(IoringSetupFlags::CQSIZE);
        self.cq_entries = cq_size.next_power_of_two();

        self
    }

    /// Share the asynchronous worker thread backend of the specified `ring_fd` `io_uring`
    /// instance. The polling thread will also be shared, if both rings are setup with [`Self::with_sq_poll`].
    #[must_use]
    pub const fn with_attached_work_queue<'a: 'fd>(mut self, ring_fd: BorrowedFd<'a>) -> Self {
        self.flags = self.flags.union(IoringSetupFlags::ATTACH_WQ);
        self.work_queue_fd = Some(ring_fd);

        self
    }

    /// Sets up the ring in a disabled state.
    ///
    /// When disabled, restrictions can be registered, but submissions are not allowed. The ring
    /// must be enabled before normal use can proceed.
    ///
    /// Available since Linux 5.10
    #[must_use]
    pub const fn with_disabled_ring(mut self) -> Self {
        self.flags = self.flags.union(IoringSetupFlags::R_DISABLED);

        self
    }

    /// Submits all SQ requests, even if one results in an error while submitting.
    ///
    /// Normally, `io_uring` stops submitting a batch of requests if one of them results in an
    /// error. This can cause submission of less than what was expected. Regardless, a CQE will
    /// still be posted for the errored request.
    ///
    /// Available since Linux 5.18
    #[must_use]
    pub const fn with_submit_all(mut self) -> Self {
        self.flags = self.flags.union(IoringSetupFlags::SUBMIT_ALL);

        self
    }

    /// Prevent interruption of tasks in userspace when a completion event is posted.
    ///
    /// By default, `io_uring` interrupts a task running in userspace when a completion event is
    /// posted. This is to ensure that completions run in a timely manner. For a lot of use cases,
    /// this is overkill and can cause reduced performance from the inter-processor interrupt and
    /// the kernel/user context switching. Most applications don't need forceful interruption as
    /// the events are processed at any kernel/user context switch. The exception are setups where
    /// the application uses multiple threads operating on the same ring, where the application
    /// waiting on completions isn't the one that submitted them.
    ///
    /// Available since Linux 5.19
    #[must_use]
    pub const fn with_cooperative_taskrun(mut self) -> Self {
        self.flags = self.flags.union(IoringSetupFlags::COOP_TASKRUN);
        self.flags = self.flags.union(IoringSetupFlags::TASKRUN_FLAG);

        self
    }

    /// Hint to the kernel that only a single task (or thread) will submit requests.
    ///
    /// This is used in the kernel for optimisations. The task specified is either the one that
    /// created the ring, or the task that enables the ring if it was created in a disabled state.
    /// The kernel enforces this rule, failing requests with `-EEXIST` if the restriction is
    /// violated.
    ///
    /// When SQ polling is enabled, the polling task does all submissions on behalf of the
    /// application, so it always complies with the above rules.
    ///
    /// Available since Linux 6.0
    #[must_use]
    pub const fn with_single_issuer(mut self) -> Self {
        self.flags = self.flags.union(IoringSetupFlags::SINGLE_ISSUER);

        self
    }

    /// Process all outstanding work at the end of any system call or thread interrupt.
    ///
    /// This may delay the application from making other progress, but hints to `io_uring` that it
    /// should defer work until `io_uring_enter` is called with the `IORING_ENTER_GETEVENTS` flag
    /// set. This allows the application to request work to run just before it wants to process
    /// completions.
    ///
    /// Requires that the ring was setup with [`Self::with_single_issuer`].
    ///
    /// Available since Linux 6.1
    #[must_use]
    pub const fn with_deferred_taskrun(mut self) -> Self {
        self.flags = self.flags.union(IoringSetupFlags::DEFER_TASKRUN);

        self
    }
}

#[cfg(test)]
mod test {
    use rustix::{
        fd::{AsRawFd, BorrowedFd},
        io_uring::IoringSetupFlags,
    };

    use super::Params;

    // Ensure we only run tests that the host machine can support
    // fn at_least_kernel_version(version: &str) -> bool {
    //     use semver::{Version, VersionReq};
    //     let k_version =
    //         String::from_utf8_lossy(rustix::system::uname().release().to_bytes()).to_string();
    //     println!("Kernel version: {k_version}");
    //
    //     let request = VersionReq::parse(&format!(">={version}")).unwrap();
    //     let k_version = Version::parse(&k_version).unwrap();
    //
    //     request.matches(&k_version)
    // }

    #[test]
    fn creates_default_parameters() {
        let params = Params::new();

        assert_eq!(params.sq_entries, 0);
        assert_eq!(params.cq_entries, 0);
        assert_eq!(params.sq_thread_cpu, 0);
        assert_eq!(params.sq_thread_idle, 0);
        assert!(params.work_queue_fd.is_none());
        assert_eq!(params.flags, IoringSetupFlags::empty());
    }

    #[test]
    fn sets_appropriate_flags() {
        struct TestCase {
            func: fn(Params<'_>) -> Params<'_>,
            flags: Vec<IoringSetupFlags>,
        }

        let expected: Vec<TestCase> = vec![
            TestCase {
                func: |p: Params| p.with_io_poll(),
                flags: vec![IoringSetupFlags::IOPOLL],
            },
            TestCase {
                func: |p: Params| p.with_disabled_ring(),
                flags: vec![IoringSetupFlags::R_DISABLED],
            },
            TestCase {
                func: |p: Params| p.with_submit_all(),
                flags: vec![IoringSetupFlags::SUBMIT_ALL],
            },
            TestCase {
                func: |p: Params| p.with_cooperative_taskrun(),
                flags: vec![
                    IoringSetupFlags::COOP_TASKRUN,
                    IoringSetupFlags::TASKRUN_FLAG,
                ],
            },
            TestCase {
                func: |p: Params| p.with_single_issuer(),
                flags: vec![IoringSetupFlags::SINGLE_ISSUER],
            },
            TestCase {
                func: |p: Params| p.with_deferred_taskrun(),
                flags: vec![IoringSetupFlags::DEFER_TASKRUN],
            },
        ];

        for test_case in &expected {
            let params = Params::new();
            let result = (test_case.func)(params);

            for flag in &test_case.flags {
                assert!(result.flags.contains(*flag));
            }
        }
    }

    #[test]
    fn sets_sq_poll_without_cpu() {
        let params = Params::new().with_sq_poll(None);
        assert!(params.flags.contains(IoringSetupFlags::SQPOLL));
        assert_eq!(params.sq_thread_idle, 0);
    }

    #[test]
    fn sets_sq_poll_with_cpu() {
        let params = Params::new().with_sq_poll(Some(1));
        assert!(params.flags.contains(IoringSetupFlags::SQPOLL));
        assert_eq!(params.sq_thread_idle, 1);
    }

    #[test]
    fn sets_sq_affinity() {
        let params = Params::new().with_sq_affinity(1);
        assert!(params.flags.contains(IoringSetupFlags::SQ_AFF));
        assert_eq!(params.sq_thread_cpu, 1);
    }

    #[test]
    fn sets_sq_size() {
        let params = Params::new().with_sq_size(2);
        assert_eq!(params.sq_entries, 2);
    }

    #[test]
    fn sets_sq_size_to_next_power_of_two() {
        let params = Params::new().with_sq_size(3);
        assert_eq!(params.sq_entries, 4);
    }

    #[test]
    fn sets_cq_size() {
        let params = Params::new().with_cq_size(2);
        assert_eq!(params.cq_entries, 2);
    }

    #[test]
    fn sets_cq_size_to_next_power_of_two() {
        let params = Params::new().with_cq_size(3);
        assert_eq!(params.cq_entries, 4);
    }

    #[test]
    fn sets_attached_work_queue() {
        let raw_fd = 1;
        let fd = unsafe { BorrowedFd::borrow_raw(raw_fd) };
        let params = Params::new().with_attached_work_queue(fd);
        assert!(
            params
                .work_queue_fd
                .is_some_and(|fd| fd.as_raw_fd() == raw_fd)
        );
    }
}
