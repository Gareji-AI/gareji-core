//! Deterministic human-facing rendering for Gareji CLI reports.

use std::fmt::Write as _;

use crate::{SetupReport, StatusReport, StepStatus};

/// Render one Setup or Doctor report for a human-facing terminal.
#[must_use]
pub fn render_setup_report(report: &SetupReport) -> String {
    let mut output = String::new();
    writeln!(output, "Gareji").expect("writing to a string cannot fail");
    if let Some(project_id) = &report.project_id {
        writeln!(output, "Project: {project_id}").expect("writing to a string cannot fail");
    }
    writeln!(output, "Status: {}", overall_status(report))
        .expect("writing to a string cannot fail");
    writeln!(output).expect("writing to a string cannot fail");
    write_system(&mut output, report);

    if let Some(next) = setup_next_action(report) {
        writeln!(output).expect("writing to a string cannot fail");
        writeln!(output, "Next: {next}").expect("writing to a string cannot fail");
    }
    output
}

/// Render one project-centered Status report for a human-facing terminal.
#[must_use]
pub fn render_status_report(report: &StatusReport) -> String {
    let mut output = String::new();
    writeln!(output, "{}", report.project_name).expect("writing to a string cannot fail");
    writeln!(output, "Project: {}", report.project_id).expect("writing to a string cannot fail");
    writeln!(
        output,
        "Workspace: {}",
        human_windows_path(&report.workspace)
    )
    .expect("writing to a string cannot fail");
    writeln!(output, "Context references: {}", report.context_references)
        .expect("writing to a string cannot fail");
    writeln!(output, "Status: {}", overall_status(&report.health))
        .expect("writing to a string cannot fail");
    writeln!(output).expect("writing to a string cannot fail");
    write_system(&mut output, &report.health);
    writeln!(output).expect("writing to a string cannot fail");
    writeln!(output, "Recent activity").expect("writing to a string cannot fail");

    if report.recent_activity.is_empty() {
        writeln!(output, "  No activity recorded.").expect("writing to a string cannot fail");
    } else {
        for activity in &report.recent_activity {
            writeln!(output, "  {}  {}", activity.recorded_at, activity.outcome)
                .expect("writing to a string cannot fail");
            writeln!(output, "    {}", activity.summary).expect("writing to a string cannot fail");
            if !activity.changed_paths.is_empty() {
                writeln!(output, "    Changed").expect("writing to a string cannot fail");
                for path in activity.changed_paths.iter().take(5) {
                    writeln!(output, "      {path}").expect("writing to a string cannot fail");
                }
                if activity.changed_paths.len() > 5 {
                    writeln!(
                        output,
                        "      ... and {} more",
                        activity.changed_paths.len() - 5
                    )
                    .expect("writing to a string cannot fail");
                }
            }
        }
    }

    if let Some(next) = status_next_action(report) {
        writeln!(output).expect("writing to a string cannot fail");
        writeln!(output, "Next: {next}").expect("writing to a string cannot fail");
    }
    output
}

fn write_system(output: &mut String, report: &SetupReport) {
    writeln!(output, "System").expect("writing to a string cannot fail");
    for step in &report.steps {
        writeln!(
            output,
            "  {:<8} {:<12} {}",
            step_status_label(&step.status),
            step.step.to_string(),
            step.message
        )
        .expect("writing to a string cannot fail");
    }
}

fn overall_status(report: &SetupReport) -> &'static str {
    if !report.healthy
        || report
            .steps
            .iter()
            .any(|step| step.status == StepStatus::Missing)
    {
        "needs attention"
    } else if report
        .steps
        .iter()
        .any(|step| step.status == StepStatus::Planned)
    {
        "planned"
    } else if report
        .steps
        .iter()
        .any(|step| step.status == StepStatus::Changed)
    {
        "changed"
    } else {
        "ready"
    }
}

fn setup_next_action(report: &SetupReport) -> Option<&'static str> {
    if report.restart_codex {
        Some("restart Codex, then review and trust the Gareji Progress Stop Hook.")
    } else if report
        .steps
        .iter()
        .any(|step| step.status == StepStatus::Planned)
    {
        Some("rerun this command without --dry-run to apply the setup.")
    } else if !report.healthy {
        Some("run `gareji setup` to repair the missing layers, then rerun `gareji doctor`.")
    } else if report.project_id.is_some() {
        Some("work in Codex, then run `gareji status` from the project workspace.")
    } else {
        None
    }
}

fn status_next_action(report: &StatusReport) -> Option<&'static str> {
    if !report.health.healthy {
        Some("run `gareji setup` for this workspace, then rerun `gareji status`.")
    } else if report.recent_activity.is_empty() {
        Some("complete a Codex turn in this workspace.")
    } else {
        None
    }
}

fn step_status_label(status: &StepStatus) -> &'static str {
    match status {
        StepStatus::Ready => "ready",
        StepStatus::Changed => "changed",
        StepStatus::Planned => "planned",
        StepStatus::Missing => "missing",
    }
}

fn human_windows_path(path: &str) -> String {
    if let Some(path) = path.strip_prefix(r"\\?\UNC\") {
        return format!(r"\\{path}");
    }
    path.strip_prefix(r"\\?\").unwrap_or(path).to_owned()
}
