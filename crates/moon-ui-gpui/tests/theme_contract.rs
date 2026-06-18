use std::fs;
use std::path::{Path, PathBuf};

fn rust_sources(dir: &Path, out: &mut Vec<PathBuf>) {
    let entries = fs::read_dir(dir).unwrap_or_else(|err| {
        panic!("failed to read {}: {err}", dir.display());
    });
    for entry in entries {
        let entry = entry.unwrap_or_else(|err| panic!("failed to read dir entry: {err}"));
        let path = entry.path();
        if path.is_dir() {
            rust_sources(&path, out);
        } else if path.extension().and_then(|ext| ext.to_str()) == Some("rs") {
            out.push(path);
        }
    }
}

#[test]
fn terminal_ui_uses_runtime_moon_ui_theme() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let mut sources = Vec::new();
    rust_sources(&root, &mut sources);

    let mut violations = Vec::new();
    for path in sources {
        let text = fs::read_to_string(&path)
            .unwrap_or_else(|err| panic!("failed to read {}: {err}", path.display()));
        for (line_ix, line) in text.lines().enumerate() {
            let check = line.replace("moon_ui::", "moon_ui__");
            if check.contains("MoonPalette::TERMINAL")
                || check.contains("moon_core::palette")
                || check.contains("use moon_core::palette")
                || check.contains("palette::")
            {
                violations.push(format!(
                    "{}:{}: {}",
                    path.display(),
                    line_ix + 1,
                    line.trim()
                ));
            }
        }
    }

    assert!(
        violations.is_empty(),
        "terminal UI must use MoonPalette::active/MoonTheme runtime config, not old palette sources:\n{}",
        violations.join("\n")
    );
}

#[test]
fn chart_background_policy_keeps_gpu_canvas_under_scene() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let chartdx = fs::read_to_string(root.join("chartdx").join("mod.rs")).unwrap();
    let chart_panel = fs::read_to_string(root.join("panels").join("chart.rs")).unwrap();
    let chart_tabs = fs::read_to_string(root.join("chart_tabs.rs")).unwrap();
    let main = fs::read_to_string(root.join("main.rs")).unwrap();
    let detached = fs::read_to_string(root.join("detached.rs")).unwrap();

    assert!(
        chartdx.contains("gpu_canvas(self.canvas.clone())")
            && !chartdx.contains("add_gpu_pass")
            && !chart_panel.contains("request_continuous_presentation"),
        "chart must use element-scoped gpu_canvas, not old window-global pass/continuous present"
    );
    assert!(
        chart_panel.contains("fn background_policy(&self, _cx: &App) -> MoonBackgroundPolicy")
            && chart_panel.contains("MoonBackgroundPolicy::NoFill"),
        "ChartPanel must keep NoFill background policy"
    );
    assert!(
        chart_tabs.contains("fn background_policy(&self, _cx: &App) -> MoonBackgroundPolicy")
            && chart_tabs.contains("MoonBackgroundPolicy::NoFill"),
        "ChartTabs host must keep NoFill background policy"
    );
    assert!(
        main.contains(".background_policy(MoonBackgroundPolicy::NoFill)")
            && main.contains(".tab_background_policy(MoonBackgroundPolicy::NoFill)"),
        "main chart dock/root path must keep NoFill policies"
    );
    assert!(
        main.contains(
            "Root::new(host, window, cx).background_policy(MoonBackgroundPolicy::NoFill)"
        ) && chart_tabs.contains(
            "Root::new(host, window, cx).background_policy(MoonBackgroundPolicy::NoFill)"
        ),
        "detached/debug chart windows must keep NoFill roots so UnderScene gpu_canvas stays visible"
    );
    assert!(
        !main.contains(".bg(rgb(p.shell))\n                    .child(self.panel.clone())")
            && !chart_tabs
                .contains(".bg(rgb(p.shell))\n                    .child(self.panel.clone())"),
        "chart window body must not paint an opaque GPUI quad over UnderScene gpu_canvas"
    );
    assert!(
        detached.contains(".background_policy(MoonBackgroundPolicy::Opaque)"),
        "detached non-chart windows must paint an explicit opaque root"
    );
}

#[test]
fn terminal_windows_use_closed_window_frame_api() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let mut sources = Vec::new();
    rust_sources(&root, &mut sources);

    let mut violations = Vec::new();
    for path in sources {
        let text = fs::read_to_string(&path)
            .unwrap_or_else(|err| panic!("failed to read {}: {err}", path.display()));
        let rel = path.strip_prefix(&root).unwrap_or(&path);
        let rel_text = rel.to_string_lossy().replace('\\', "/");
        for (line_ix, line) in text.lines().enumerate() {
            let trimmed = line.trim();
            let is_windowing = rel_text == "windowing.rs";
            let is_design = rel_text == "design.rs";
            if trimmed.contains("MoonWindowChrome::new")
                || trimmed.contains("MoonWindowChromeButton")
                || trimmed.contains("WindowControlArea::Drag")
                || trimmed.contains("start_window_move")
                || trimmed.contains("titlebar_double_click")
                || (!is_design
                    && (trimmed.contains("logo_sized(")
                        || trimmed.contains("logo_mark(")
                        || trimmed.contains("design::logo_sized")
                        || trimmed.contains("design::logo_mark")))
                || (!is_windowing && trimmed.contains("WindowOptions {"))
            {
                violations.push(format!("{}:{}: {}", path.display(), line_ix + 1, trimmed));
            }
        }
    }

    assert!(
        violations.is_empty(),
        "terminal windows must go through windowing.rs + MoonWindowFrame instead of ad-hoc chrome/window options:\n{}",
        violations.join("\n")
    );
}
