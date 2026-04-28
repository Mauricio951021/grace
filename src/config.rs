use crate::arena::DEFAULT_ARENA_SLOTS;
use crate::executor::RT;


use std::thread;

const DEFAULT_LOCAL_TASK_BUFFER_SIZE: usize = 128;
const DEFAULT_WORKER_TASK_BUFFER_SIZE: usize = 4096;
const DEFAULT_WORKER_STACK_SIZE: usize = 2 * 1024 * 1024;


pub(crate) struct GraceConfig {
    pub(crate) local_task_buffer_size: usize,
    pub(crate) worker_task_buffer_size: usize,
    pub(crate) arena_slots: usize,
    pub(crate) worker_count: usize,
    pub(crate) worker_stack_size: usize,
}

impl Default for GraceConfig {
    fn default() -> Self {
        let worker_count = thread::available_parallelism()
        .map(|n| n.get()).unwrap_or(4);
        Self {
            local_task_buffer_size: DEFAULT_LOCAL_TASK_BUFFER_SIZE,
            worker_task_buffer_size: DEFAULT_WORKER_TASK_BUFFER_SIZE,
            arena_slots: DEFAULT_ARENA_SLOTS,
            worker_count,
            worker_stack_size: DEFAULT_WORKER_STACK_SIZE,
        }
    }
}

pub struct GraceBuilder {
    config: GraceConfig,
}

impl GraceBuilder {
    pub fn new() -> Self {
        Self {
            config: GraceConfig::default(),
        }
    }

    pub fn local_task_buffer_size(mut self, s: usize) -> Self {
        self.config.local_task_buffer_size = s;
        self
    }

    pub fn worker_task_buffer_size(mut self, s: usize) -> Self {
        self.config.worker_task_buffer_size = s;
        self
    }

    pub fn arena_slots(mut self, s: usize) -> Self {
        self.config.arena_slots = s;
        self
    }

    pub fn worker_count(mut self, n: usize) -> Self {
        self.config.worker_count = n;
        self
    }

    pub fn worker_stack_size(mut self, s: usize) -> Self {
        self.config.worker_stack_size = s;
        self
    }

    pub fn build(self) -> RT {
        RT::new(self.config)
    }
}

