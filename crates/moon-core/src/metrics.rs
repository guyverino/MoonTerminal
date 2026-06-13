//! Диагностические метрики процесса и системы для статус-бара: CPU
//! (процесс/система), RAM процесса и её рост за окно. sysinfo-обновление дорогое,
//! поэтому реально опрашиваем не чаще REFRESH_EVERY, между сэмплами отдаём кэш.

use std::collections::VecDeque;
use std::time::{Duration, Instant};

use sysinfo::{Pid, ProcessesToUpdate, System};

/// Как часто реально опрашиваем sysinfo.
const REFRESH_EVERY: Duration = Duration::from_millis(1000);
/// Окно, на котором считаем прирост памяти (растёт/падает).
const MEM_WINDOW: Duration = Duration::from_secs(5);

/// Снимок метрик — Copy, дёшево прокидывается в каждый `WindowHost::render`.
#[derive(Clone, Copy, Default)]
pub struct MetricsSnapshot {
    /// CPU процесса, % всей машины (как в Task Manager: 100% = все ядра заняты).
    pub cpu_process: f32,
    /// CPU всей системы, %.
    pub cpu_system: f32,
    /// RAM процесса (resident), МБ.
    pub mem_mb: f32,
    /// Прирост RAM за MEM_WINDOW, МБ (>0 — растёт; стабильный плюс → утечка).
    pub mem_delta_mb: f32,
}

pub struct Metrics {
    sys: System,
    pid: Pid,
    ncpu: f32,
    last_refresh: Option<Instant>,
    snap: MetricsSnapshot,
    /// (время, RAM МБ) для расчёта прироста за MEM_WINDOW.
    mem_hist: VecDeque<(Instant, f32)>,
}

impl Metrics {
    pub fn new() -> Self {
        let mut sys = System::new();
        sys.refresh_cpu_usage();
        let ncpu = sys.cpus().len().max(1) as f32;
        let pid = sysinfo::get_current_pid().unwrap_or(Pid::from(0));
        Self {
            sys,
            pid,
            ncpu,
            last_refresh: None,
            snap: MetricsSnapshot::default(),
            mem_hist: VecDeque::new(),
        }
    }

    /// Актуальный снимок; реально опрашивает систему не чаще REFRESH_EVERY.
    pub fn sample(&mut self, now: Instant) -> MetricsSnapshot {
        let due = self
            .last_refresh
            .is_none_or(|t| now.duration_since(t) >= REFRESH_EVERY);
        if !due {
            return self.snap;
        }
        self.last_refresh = Some(now);

        self.sys.refresh_cpu_usage();
        self.sys.refresh_memory();
        self.sys
            .refresh_processes(ProcessesToUpdate::Some(&[self.pid]), true);

        let cpu_system = self.sys.global_cpu_usage();
        let (cpu_process, mem_mb) = match self.sys.process(self.pid) {
            // cpu_usage(): 100% = одно ядро → делим на число ядер (как Task Manager).
            Some(p) => (
                p.cpu_usage() / self.ncpu,
                p.memory() as f32 / (1024.0 * 1024.0),
            ),
            None => (0.0, 0.0),
        };

        self.mem_hist.push_back((now, mem_mb));
        while self
            .mem_hist
            .front()
            .is_some_and(|(t, _)| now.duration_since(*t) > MEM_WINDOW)
        {
            self.mem_hist.pop_front();
        }
        let mem_delta_mb = self.mem_hist.front().map(|(_, m0)| mem_mb - *m0).unwrap_or(0.0);

        self.snap = MetricsSnapshot {
            cpu_process,
            cpu_system,
            mem_mb,
            mem_delta_mb,
        };
        self.snap
    }
}

impl Default for Metrics {
    fn default() -> Self {
        Self::new()
    }
}
