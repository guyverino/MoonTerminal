//! Built-in diagnostic scenario runner.
//!
//! `moonterminal --debug-script chart-smoke` opens a chart, injects a short native mouse storm
//! over it and fails the process if cursor movement wakes expensive GPUI paths or burns CPU.

use std::time::{Duration, Instant};

use gpui::Context;
use moon_core::metrics::MetricsSnapshot;
use moon_core::session::CoreId;

use crate::{Backend, diag};

const DEFAULT_MARKET: &str = "BTCUSDT";
const START_DELAY: Duration = Duration::from_millis(1000);
const BASELINE: Duration = Duration::from_millis(5000);
const BASELINE_WARMUP: Duration = Duration::from_millis(1500);
const COOLDOWN: Duration = Duration::from_millis(1200);
const TEXT_WARMUP: Duration = Duration::from_millis(2500);
const OPEN_TIMEOUT: Duration = Duration::from_millis(10_000);
const PROBE_TIMEOUT: Duration = Duration::from_millis(10_000);
const DEFAULT_MOUSE_HZ: f64 = 5000.0;
const DEFAULT_STORM: Duration = Duration::from_millis(5000);
const DEFAULT_TEXT_LABELS: usize = 0;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Phase {
    WaitStartup,
    WaitOpen,
    WaitProbe,
    WarmText,
    Baseline,
    Storm,
    Cooldown,
    Done,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Script {
    ChartSmoke,
}

#[derive(Clone, Debug)]
pub(crate) struct Config {
    script: Script,
    market: String,
    storm: Duration,
    mouse_hz: f64,
    text_labels: usize,
}

#[derive(Clone, Copy, Debug)]
pub(crate) struct ChartProbe {
    hwnd: Option<isize>,
    #[cfg_attr(not(target_os = "macos"), allow(dead_code))]
    screen_left: f32,
    #[cfg_attr(not(target_os = "macos"), allow(dead_code))]
    screen_top: f32,
    left: f32,
    top: f32,
    width: f32,
    height: f32,
    scale_factor: f32,
}

#[derive(Clone)]
struct Sample {
    phase: Phase,
    rates: Vec<diag::DiagRate>,
    metrics: MetricsSnapshot,
    gpu_frame_ms: f64,
}

struct MouseStorm {
    stop: std::sync::Arc<std::sync::atomic::AtomicBool>,
    done: std::sync::Arc<std::sync::atomic::AtomicBool>,
}

pub(crate) struct Runtime {
    config: Config,
    started: Instant,
    phase: Phase,
    phase_since: Instant,
    probe: Option<ChartProbe>,
    samples: Vec<Sample>,
    storm: Option<MouseStorm>,
    opened_target: Option<(CoreId, String)>,
    text_overlay_enabled: bool,
    present_pressure_enabled: bool,
    last_wait_log: Instant,
}

impl Config {
    pub(crate) fn from_args<I>(args: I) -> anyhow::Result<Option<Self>>
    where
        I: IntoIterator<Item = String>,
    {
        let mut args = args.into_iter();
        let mut script = None;
        while let Some(arg) = args.next() {
            if arg != "--debug-script" {
                continue;
            }
            let Some(value) = args.next() else {
                anyhow::bail!("--debug-script requires a script name");
            };
            script = Some(match value.as_str() {
                "chart-smoke" => Script::ChartSmoke,
                other => anyhow::bail!("unknown --debug-script {other:?}"),
            });
        }

        let Some(script) = script else {
            return Ok(None);
        };
        let market = std::env::var("MOON_FIRETEST_MARKET")
            .ok()
            .filter(|s| !s.trim().is_empty())
            .unwrap_or_else(|| DEFAULT_MARKET.to_string());
        let mouse_hz = std::env::var("MOON_FIRETEST_MOUSE_HZ")
            .ok()
            .and_then(|s| s.parse::<f64>().ok())
            .filter(|v| v.is_finite() && *v >= 100.0)
            .unwrap_or(DEFAULT_MOUSE_HZ);
        let storm = std::env::var("MOON_FIRETEST_STORM_MS")
            .ok()
            .and_then(|s| s.parse::<u64>().ok())
            .map(Duration::from_millis)
            .filter(|v| *v >= Duration::from_millis(1000))
            .unwrap_or(DEFAULT_STORM);
        let text_labels = std::env::var("MOON_FIRETEST_TEXT_LABELS")
            .ok()
            .and_then(|s| s.parse::<usize>().ok())
            .unwrap_or(DEFAULT_TEXT_LABELS);
        Ok(Some(Self {
            script,
            market,
            storm,
            mouse_hz,
            text_labels,
        }))
    }
}

impl ChartProbe {
    pub(crate) fn new(
        hwnd: Option<isize>,
        screen_left: f32,
        screen_top: f32,
        left: f32,
        top: f32,
        width: f32,
        height: f32,
        scale_factor: f32,
    ) -> Option<Self> {
        (width >= 80.0
            && height >= 80.0
            && scale_factor > 0.0
            && screen_left.is_finite()
            && screen_top.is_finite())
        .then_some(Self {
            hwnd,
            screen_left,
            screen_top,
            left,
            top,
            width,
            height,
            scale_factor,
        })
    }
}

impl Runtime {
    pub(crate) fn new(config: Config) -> Self {
        diag::force_enable();
        let now = Instant::now();
        firetest_info(&format!(
            "[firetest] script={:?} market={} storm_ms={} mouse_hz={:.0} text_labels={}",
            config.script,
            config.market,
            config.storm.as_millis(),
            config.mouse_hz,
            config.text_labels
        ));
        Self {
            config,
            started: now,
            phase: Phase::WaitStartup,
            phase_since: now,
            probe: None,
            samples: Vec::new(),
            storm: None,
            opened_target: None,
            text_overlay_enabled: false,
            present_pressure_enabled: false,
            last_wait_log: now,
        }
    }

    fn set_phase(&mut self, phase: Phase) {
        self.phase = phase;
        self.phase_since = Instant::now();
        firetest_info(&format!("[firetest] phase={phase:?}"));
    }

    fn observe_probe(&mut self, probe: ChartProbe) {
        if matches!(
            self.phase,
            Phase::WaitProbe | Phase::Baseline | Phase::Storm
        ) {
            self.probe = Some(probe);
        }
    }

    fn record_sample(
        &mut self,
        _elapsed_ms: f64,
        rates: &[diag::DiagRate],
        metrics: MetricsSnapshot,
        gpu_frame_ms: f64,
    ) {
        if self.phase == Phase::Done {
            return;
        }
        if self.phase == Phase::Baseline && self.phase_since.elapsed() < BASELINE_WARMUP {
            return;
        }
        self.samples.push(Sample {
            phase: self.phase,
            rates: rates.to_vec(),
            metrics,
            gpu_frame_ms,
        });
    }

    fn tick(&mut self, backend: &mut Backend, cx: &mut Context<Backend>) {
        match self.phase {
            Phase::WaitStartup => {
                if self.started.elapsed() >= START_DELAY {
                    self.set_phase(Phase::WaitOpen);
                }
            }
            Phase::WaitOpen => {
                if self.try_open_chart(backend, cx) {
                    self.set_phase(Phase::WaitProbe);
                } else if self.phase_since.elapsed() >= OPEN_TIMEOUT {
                    self.fail("no active visible core/window to open chart");
                } else {
                    self.wait_log("waiting for active visible core/window");
                }
            }
            Phase::WaitProbe => {
                if self.probe.is_some() {
                    let text_applied = self.enable_text_overlay(backend, cx);
                    if self.config.text_labels > 0 && text_applied == 0 {
                        self.fail("chart opened but firetest text overlay did not attach");
                        return;
                    }
                    if self.config.text_labels == 0 {
                        self.set_phase(Phase::Baseline);
                    } else {
                        self.set_phase(Phase::WarmText);
                    }
                } else if self.phase_since.elapsed() >= PROBE_TIMEOUT {
                    self.fail("chart opened but no chart bounds probe arrived");
                } else {
                    self.wait_log("waiting for chart bounds probe");
                }
            }
            Phase::WarmText => {
                if self.phase_since.elapsed() >= TEXT_WARMUP {
                    self.set_phase(Phase::Baseline);
                }
            }
            Phase::Baseline => {
                self.set_present_pressure(backend, true);
                if self.phase_since.elapsed() >= BASELINE {
                    let Some(probe) = self.probe else {
                        self.fail("missing chart probe before mouse storm");
                        return;
                    };
                    match start_mouse_storm(probe, self.config.storm, self.config.mouse_hz) {
                        Ok(storm) => {
                            self.storm = Some(storm);
                            self.set_phase(Phase::Storm);
                        }
                        Err(err) => self.fail(&err),
                    }
                }
            }
            Phase::Storm => {
                self.set_present_pressure(backend, true);
                let done = self.storm.as_ref().is_some_and(MouseStorm::is_done);
                if done || self.phase_since.elapsed() >= self.config.storm {
                    self.stop_storm();
                    self.set_phase(Phase::Cooldown);
                }
            }
            Phase::Cooldown => {
                self.set_present_pressure(backend, false);
                if self.phase_since.elapsed() >= COOLDOWN {
                    self.evaluate_and_exit();
                }
            }
            Phase::Done => {}
        }
    }

    fn wait_log(&mut self, msg: &str) {
        if self.last_wait_log.elapsed() < Duration::from_millis(1000) {
            return;
        }
        self.last_wait_log = Instant::now();
        firetest_info(&format!("[firetest] {msg}"));
    }

    fn try_open_chart(&mut self, backend: &mut Backend, cx: &mut Context<Backend>) -> bool {
        if backend.open_request.is_some() || backend.group_windows.is_empty() {
            return false;
        }

        let target_market = self.config.market.trim();
        let candidate = backend.config.servers.iter().find_map(|server| {
            let session_exists = backend
                .session
                .sessions()
                .iter()
                .any(|session| session.id == server.id && session.group == server.group);
            (server.active
                && server.show_window
                && backend.config.group(&server.group).active
                && backend.group_windows.contains_key(&server.group)
                && session_exists)
                .then(|| (server.id, server.group.clone(), server.name.clone()))
        });
        let Some((core, group, name)) = candidate else {
            return false;
        };

        let market = target_market.to_string();
        backend.open_request = Some((core, market.clone()));
        backend.open_request_rev = backend.open_request_rev.wrapping_add(1);
        backend.open_request_activate = false;
        backend.follow = true;
        self.opened_target = Some((core, market.clone()));
        firetest_info(&format!(
            "[firetest] open chart: core={core} group={group} name={name} market={market}"
        ));
        cx.notify();
        true
    }

    fn set_present_pressure(&mut self, backend: &mut Backend, enabled: bool) {
        if self.present_pressure_enabled == enabled {
            return;
        }
        self.present_pressure_enabled = enabled;
        for chart in backend.live_chart_consumers() {
            chart.set_firetest_force_present(enabled);
        }
    }

    fn enable_text_overlay(&mut self, backend: &mut Backend, cx: &mut Context<Backend>) -> usize {
        if self.text_overlay_enabled {
            return 0;
        }
        self.text_overlay_enabled = true;
        let count = self.config.text_labels;
        if count == 0 {
            firetest_info("[firetest] text overlay disabled");
            return 0;
        }
        let consumers = backend.live_chart_consumers();
        let mut applied = 0usize;
        for chart in consumers {
            if chart.set_firetest_text_labels(count) {
                applied += 1;
            }
        }
        firetest_info(&format!(
            "[firetest] text overlay labels={count} applied_to={applied}"
        ));
        if applied > 0 {
            cx.notify();
        }
        applied
    }

    fn stop_storm(&mut self) {
        if let Some(storm) = self.storm.take() {
            storm.stop();
        }
    }

    fn evaluate_and_exit(&mut self) {
        self.set_phase(Phase::Done);
        let baseline: Vec<&Sample> = self
            .samples
            .iter()
            .filter(|s| s.phase == Phase::Baseline)
            .collect();
        let storm: Vec<&Sample> = self
            .samples
            .iter()
            .filter(|s| s.phase == Phase::Storm)
            .collect();
        if storm.is_empty() {
            self.fail("no storm diag samples");
            return;
        }

        let avg_rate = |label: &str| -> f64 {
            storm.iter().map(|s| rate(s, label)).sum::<f64>() / storm.len() as f64
        };
        let baseline_max_rate = |label: &str| -> f64 {
            baseline
                .iter()
                .map(|s| rate(s, label))
                .fold(0.0_f64, f64::max)
        };
        let rate_delta =
            |label: &str| -> f64 { (avg_rate(label) - baseline_max_rate(label)).max(0.0) };
        let max_rate =
            |label: &str| -> f64 { storm.iter().map(|s| rate(s, label)).fold(0.0_f64, f64::max) };
        let avg_cpu = storm
            .iter()
            .map(|s| s.metrics.cpu_process as f64)
            .sum::<f64>()
            / storm.len() as f64;
        let max_cpu = storm
            .iter()
            .map(|s| s.metrics.cpu_process as f64)
            .fold(0.0_f64, f64::max);
        let baseline_cpu = if baseline.is_empty() {
            0.0
        } else {
            baseline
                .iter()
                .map(|s| s.metrics.cpu_process as f64)
                .sum::<f64>()
                / baseline.len() as f64
        };
        let cpu_delta = (avg_cpu - baseline_cpu).max(0.0);
        let avg_gpu_process = storm
            .iter()
            .map(|s| s.metrics.gpu_process as f64)
            .sum::<f64>()
            / storm.len() as f64;
        let max_gpu_process = storm
            .iter()
            .map(|s| s.metrics.gpu_process as f64)
            .fold(0.0_f64, f64::max);
        let baseline_gpu_process = if baseline.is_empty() {
            0.0
        } else {
            baseline
                .iter()
                .map(|s| s.metrics.gpu_process as f64)
                .sum::<f64>()
                / baseline.len() as f64
        };
        let gpu_process_delta = (avg_gpu_process - baseline_gpu_process).max(0.0);
        let avg_gpu_frame_ms =
            storm.iter().map(|s| s.gpu_frame_ms).sum::<f64>() / storm.len() as f64;
        let max_gpu_frame_ms = storm.iter().map(|s| s.gpu_frame_ms).fold(0.0_f64, f64::max);
        let mem_values: Vec<f64> = storm
            .iter()
            .map(|s| s.metrics.mem_mb as f64)
            .filter(|m| *m > 1.0)
            .collect();
        let mem_growth = if mem_values.len() >= 2 {
            let mem_min = mem_values.iter().copied().fold(f64::INFINITY, f64::min);
            let mem_max = mem_values.iter().copied().fold(0.0_f64, f64::max);
            (mem_max - mem_min).max(0.0)
        } else {
            0.0
        };

        let mut fail = Vec::new();
        check_min(
            &mut fail,
            "firetest_mouse_sent",
            avg_rate("firetest_mouse_sent"),
            1000.0,
        );
        check_min(
            &mut fail,
            "chart_mouse_move",
            avg_rate("chart_mouse_move"),
            100.0,
        );
        let chart_mouse = avg_rate("chart_mouse_move");
        let fast_mouse = avg_rate("chart_mouse_move_fast");
        if chart_mouse > 1.0 {
            check_min(
                &mut fail,
                "chart_mouse_fast_coverage",
                fast_mouse / chart_mouse,
                0.90,
            );
        }
        check_max(
            &mut fail,
            "chart_mouse_move_entity",
            max_rate("chart_mouse_move_entity"),
            5.0,
        );
        check_max(&mut fail, "shell_render", max_rate("shell_render"), 10.0);
        check_max(&mut fail, "orders_render", max_rate("orders_render"), 10.0);
        check_max(&mut fail, "chart_render", max_rate("chart_render"), 10.0);
        check_max(
            &mut fail,
            "chart_input_notify",
            max_rate("chart_input_notify"),
            5.0,
        );
        check_max(
            &mut fail,
            "chart_canvas_notify",
            max_rate("chart_canvas_notify"),
            5.0,
        );
        check_max(
            &mut fail,
            "chart_gpu_prepare_delta",
            rate_delta("chart_gpu_prepare"),
            8.0,
        );
        check_max(&mut fail, "bg_draw_delta", rate_delta("bg_draw"), 12.0);
        check_max(&mut fail, "grid_draw_delta", rate_delta("grid_draw"), 12.0);
        check_max(&mut fail, "combo_draw_delta", rate_delta("combo_draw"), 12.0);
        check_max(
            &mut fail,
            "userdata_draw_delta",
            rate_delta("userdata_draw"),
            12.0,
        );
        check_max(
            &mut fail,
            "base_bake_delta",
            rate_delta("base_bake"),
            8.0,
        );
        check_max(
            &mut fail,
            "combo_bake_delta",
            rate_delta("combo_bake"),
            8.0,
        );
        check_max(
            &mut fail,
            "orderbook_bake_delta",
            rate_delta("orderbook_bake"),
            8.0,
        );
        if self.config.text_labels > 0 {
            check_max(
                &mut fail,
                "firetest_text_cold",
                max_rate("firetest_text_cold"),
                100.0,
            );
        }
        check_max(&mut fail, "cpu_process_avg", avg_cpu, 25.0);
        check_max(&mut fail, "cpu_process_delta", cpu_delta, 12.0);
        check_max(&mut fail, "cpu_process_max", max_cpu, 40.0);
        if max_gpu_process > 0.1 {
            check_max(&mut fail, "gpu_process_avg", avg_gpu_process, 35.0);
            check_max(&mut fail, "gpu_process_delta", gpu_process_delta, 25.0);
            check_max(&mut fail, "gpu_process_max", max_gpu_process, 70.0);
        }
        if max_gpu_frame_ms > 0.01 {
            check_max(&mut fail, "gpu_frame_ms_avg", avg_gpu_frame_ms, 6.0);
            check_max(&mut fail, "gpu_frame_ms_max", max_gpu_frame_ms, 16.0);
        }
        check_max(&mut fail, "mem_growth_mb", mem_growth, 96.0);

        let summary = format!(
            "mouse_sent={:.0}/s chart_mouse={:.0}/s fast={:.0}/s entity={:.0}/s fast_stop={:.0}/s shell={:.0}/s orders={:.0}/s chart_render={:.0}/s input_notify={:.0}/s text_draw={:.0}/s text_cold={:.0}/s cpu_avg={:.1}% cpu_delta={:.1}% gpu_proc_avg={:.1}% gpu_proc_delta={:.1}% gpu_proc_max={:.1}% gpu_frame_avg={:.3}ms gpu_frame_max={:.3}ms mem_growth={:.1}MB present={:.0}/s cam_step={:.0}/s gpu_prepare={:.0}/s(+{:.0}) bg_draw={:.0}/s(+{:.0}) combo_draw={:.0}/s(+{:.0}) base_bake={:.0}/s(+{:.0}) combo_bake={:.0}/s(+{:.0}) book_bake={:.0}/s(+{:.0})",
            avg_rate("firetest_mouse_sent"),
            avg_rate("chart_mouse_move"),
            avg_rate("chart_mouse_move_fast"),
            avg_rate("chart_mouse_move_entity"),
            avg_rate("chart_mouse_fast_stop"),
            avg_rate("shell_render"),
            avg_rate("orders_render"),
            avg_rate("chart_render"),
            avg_rate("chart_input_notify"),
            avg_rate("firetest_text_draw"),
            avg_rate("firetest_text_cold"),
            avg_cpu,
            cpu_delta,
            avg_gpu_process,
            gpu_process_delta,
            max_gpu_process,
            avg_gpu_frame_ms,
            max_gpu_frame_ms,
            mem_growth,
            avg_rate("chart_present"),
            avg_rate("chart_cam_step"),
            avg_rate("chart_gpu_prepare"),
            rate_delta("chart_gpu_prepare"),
            avg_rate("bg_draw"),
            rate_delta("bg_draw"),
            avg_rate("combo_draw"),
            rate_delta("combo_draw"),
            avg_rate("base_bake"),
            rate_delta("base_bake"),
            avg_rate("combo_bake"),
            rate_delta("combo_bake"),
            avg_rate("orderbook_bake"),
            rate_delta("orderbook_bake"),
        );
        if fail.is_empty() {
            firetest_info(&format!("[firetest] result=PASS {summary}"));
            std::process::exit(0);
        }
        firetest_error(&format!(
            "[firetest] result=FAIL {summary} reasons={}",
            fail.join("; ")
        ));
        std::process::exit(2);
    }

    fn fail(&mut self, reason: &str) {
        self.set_phase(Phase::Done);
        self.stop_storm();
        firetest_error(&format!("[firetest] result=FAIL reason={reason}"));
        std::process::exit(2);
    }
}

fn firetest_info(line: &str) {
    log::info!("{line}");
    write_firetest_line(line);
}

fn firetest_error(line: &str) {
    log::error!("{line}");
    write_firetest_line(line);
}

fn write_firetest_line(line: &str) {
    use std::io::Write;
    if let Ok(mut f) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open("firetest.log")
    {
        let _ = writeln!(f, "{line}");
    }
}

impl MouseStorm {
    fn is_done(&self) -> bool {
        self.done.load(std::sync::atomic::Ordering::Relaxed)
    }

    fn stop(self) {
        self.stop.store(true, std::sync::atomic::Ordering::Relaxed);
    }
}

fn rate(sample: &Sample, label: &str) -> f64 {
    sample
        .rates
        .iter()
        .find(|r| r.label == label)
        .map(|r| r.hz)
        .unwrap_or(0.0)
}

fn check_min(fail: &mut Vec<String>, label: &str, got: f64, min: f64) {
    if got < min {
        fail.push(format!("{label} {got:.1} < {min:.1}"));
    }
}

fn check_max(fail: &mut Vec<String>, label: &str, got: f64, max: f64) {
    if got > max {
        fail.push(format!("{label} {got:.1} > {max:.1}"));
    }
}

pub(crate) fn tick_backend(backend: &mut Backend, cx: &mut Context<Backend>) {
    let Some(mut runtime) = backend.firetest.take() else {
        return;
    };
    runtime.tick(backend, cx);
    backend.firetest = Some(runtime);
}

pub(crate) fn observe_chart_probe(backend: &mut Backend, probe: ChartProbe) {
    if let Some(runtime) = backend.firetest.as_mut() {
        runtime.observe_probe(probe);
    }
}

pub(crate) fn record_diag_sample(backend: &mut Backend, elapsed_ms: f64, rates: &[diag::DiagRate]) {
    let metrics = backend.snap;
    let gpu_frame_ms = diag::take_gpu_frame_ms();
    if let Some(runtime) = backend.firetest.as_mut() {
        runtime.record_sample(elapsed_ms, rates, metrics, gpu_frame_ms);
    }
}

#[cfg(target_os = "windows")]
fn start_mouse_storm(
    probe: ChartProbe,
    duration: Duration,
    mouse_hz: f64,
) -> Result<MouseStorm, String> {
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, Ordering};
    use windows::Win32::Foundation::{HWND, POINT};
    use windows::Win32::Graphics::Gdi::ClientToScreen;
    use windows::Win32::UI::WindowsAndMessaging::{
        GetCursorPos, HWND_TOP, SW_RESTORE, SWP_NOMOVE, SWP_NOSIZE, SWP_SHOWWINDOW, SetCursorPos,
        SetForegroundWindow, SetWindowPos, ShowWindow,
    };

    let hwnd = probe
        .hwnd
        .ok_or_else(|| "Windows mouse storm needs a Win32 HWND probe".to_string())?;

    let stop = Arc::new(AtomicBool::new(false));
    let done = Arc::new(AtomicBool::new(false));
    let thread_stop = stop.clone();
    let thread_done = done.clone();
    std::thread::Builder::new()
        .name("moon-firetest-mouse".to_string())
        .spawn(move || {
            let start = Instant::now();
            let hwnd = HWND(hwnd as *mut _);
            unsafe {
                let _ = ShowWindow(hwnd, SW_RESTORE);
                let _ = SetWindowPos(
                    hwnd,
                    Some(HWND_TOP),
                    0,
                    0,
                    0,
                    0,
                    SWP_NOMOVE | SWP_NOSIZE | SWP_SHOWWINDOW,
                );
                let _ = SetForegroundWindow(hwnd);
            }
            let mut restore = POINT { x: 0, y: 0 };
            let restore_cursor = unsafe { GetCursorPos(&mut restore).is_ok() };
            let left = probe.left * probe.scale_factor;
            let top = probe.top * probe.scale_factor;
            let width = probe.width * probe.scale_factor;
            let height = probe.height * probe.scale_factor;
            let cx = left + width * 0.5;
            let cy = top + height * 0.5;
            let r = (width.min(height) * 0.35).max(12.0);
            let step = (2.0 * std::f32::consts::PI) / 96.0;
            let mut sent = 0_u64;
            while start.elapsed() < duration && !thread_stop.load(Ordering::Relaxed) {
                let angle = sent as f32 * step;
                let x = (cx + angle.cos() * r).round() as i32;
                let y = (cy + angle.sin() * r).round() as i32;
                let mut point = POINT { x, y };
                let moved = unsafe {
                    ClientToScreen(hwnd, &mut point).as_bool()
                        && SetCursorPos(point.x, point.y).is_ok()
                };
                if moved {
                    diag::bump(&diag::FIRETEST_MOUSE_SENT);
                } else {
                    diag::bump(&diag::FIRETEST_MOUSE_POST_FAIL);
                }
                sent = sent.wrapping_add(1);
                let target = Duration::from_secs_f64(sent as f64 / mouse_hz.max(1.0));
                let elapsed = start.elapsed();
                if target > elapsed {
                    std::thread::sleep(target - elapsed);
                } else if sent % 128 == 0 {
                    std::thread::yield_now();
                }
            }
            if restore_cursor {
                unsafe {
                    let _ = SetCursorPos(restore.x, restore.y);
                }
            }
            thread_done.store(true, Ordering::Relaxed);
        })
        .map_err(|e| format!("failed to spawn mouse storm thread: {e}"))?;
    Ok(MouseStorm { stop, done })
}

#[cfg(target_os = "macos")]
fn start_mouse_storm(
    probe: ChartProbe,
    duration: Duration,
    mouse_hz: f64,
) -> Result<MouseStorm, String> {
    use core_graphics::event::{CGEvent, CGEventTapLocation, CGEventType, CGMouseButton};
    use core_graphics::event_source::{CGEventSource, CGEventSourceStateID};
    use core_graphics::geometry::CGPoint;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, Ordering};

    let stop = Arc::new(AtomicBool::new(false));
    let done = Arc::new(AtomicBool::new(false));
    let thread_stop = stop.clone();
    let thread_done = done.clone();
    std::thread::Builder::new()
        .name("moon-firetest-mouse".to_string())
        .spawn(move || {
            let Ok(source) = CGEventSource::new(CGEventSourceStateID::HIDSystemState) else {
                diag::bump(&diag::FIRETEST_MOUSE_POST_FAIL);
                thread_done.store(true, Ordering::Relaxed);
                return;
            };
            let start = Instant::now();
            let left = probe.screen_left + probe.left;
            let top = probe.screen_top + probe.top;
            let width = probe.width;
            let height = probe.height;
            let cx = left + width * 0.5;
            let cy = top + height * 0.5;
            let r = (width.min(height) * 0.35).max(12.0);
            let step = (2.0 * std::f32::consts::PI) / 96.0;
            let mut sent = 0_u64;
            while start.elapsed() < duration && !thread_stop.load(Ordering::Relaxed) {
                let angle = sent as f32 * step;
                let point =
                    CGPoint::new((cx + angle.cos() * r) as f64, (cy + angle.sin() * r) as f64);
                match CGEvent::new_mouse_event(
                    source.clone(),
                    CGEventType::MouseMoved,
                    point,
                    CGMouseButton::Left,
                ) {
                    Ok(event) => {
                        event.post(CGEventTapLocation::HID);
                        diag::bump(&diag::FIRETEST_MOUSE_SENT);
                    }
                    Err(_) => diag::bump(&diag::FIRETEST_MOUSE_POST_FAIL),
                }
                sent = sent.wrapping_add(1);
                let target = Duration::from_secs_f64(sent as f64 / mouse_hz.max(1.0));
                let elapsed = start.elapsed();
                if target > elapsed {
                    std::thread::sleep(target - elapsed);
                } else if sent % 128 == 0 {
                    std::thread::yield_now();
                }
            }
            thread_done.store(true, Ordering::Relaxed);
        })
        .map_err(|e| format!("failed to spawn mouse storm thread: {e}"))?;
    Ok(MouseStorm { stop, done })
}

#[cfg(not(target_os = "windows"))]
#[cfg(not(target_os = "macos"))]
fn start_mouse_storm(
    _probe: ChartProbe,
    _duration: Duration,
    _mouse_hz: f64,
) -> Result<MouseStorm, String> {
    Err("--debug-script chart-smoke mouse storm is implemented for Windows and macOS; Linux X11/Wayland is not wired yet".into())
}
