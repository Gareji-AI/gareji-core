use gareji_bootstrap::{
    render_setup_report, render_status_report, ActivityReport, SetupReport, StatusReport, StepKind,
    StepReport, StepStatus,
};

fn step(step: StepKind, status: StepStatus, message: &str) -> StepReport {
    StepReport {
        step,
        status,
        message: message.to_owned(),
    }
}

fn ready_health() -> SetupReport {
    SetupReport {
        project_id: Some("garage-ai".to_owned()),
        steps: vec![
            step(
                StepKind::Core,
                StepStatus::Ready,
                "Gareji Core is installed",
            ),
            step(
                StepKind::Project,
                StepStatus::Ready,
                "the project registration matches",
            ),
        ],
        restart_codex: false,
        healthy: true,
    }
}

#[test]
fn setup_report_prioritizes_target_status_system_and_next_action() {
    let report = SetupReport {
        project_id: Some("garage-ai".to_owned()),
        steps: vec![
            step(StepKind::Core, StepStatus::Changed, "installed Gareji Core"),
            step(
                StepKind::Plugin,
                StepStatus::Changed,
                "installed the Gareji Progress Plugin",
            ),
        ],
        restart_codex: true,
        healthy: true,
    };

    assert_eq!(
        render_setup_report(&report),
        concat!(
            "Gareji\n",
            "Project: garage-ai\n",
            "Status: changed\n",
            "\n",
            "System\n",
            "  changed  core         installed Gareji Core\n",
            "  changed  plugin       installed the Gareji Progress Plugin\n",
            "\n",
            "Next: restart Codex, then review and trust the Gareji Progress Stop Hook.\n",
        )
    );
}

#[test]
fn status_report_explains_empty_activity_with_one_next_action() {
    let report = StatusReport {
        project_id: "garage-ai".to_owned(),
        project_name: "Garage AI".to_owned(),
        workspace: r"\\?\C:\Garage\Project".to_owned(),
        context_references: 1,
        health: ready_health(),
        recent_activity: Vec::new(),
    };

    assert_eq!(
        render_status_report(&report),
        concat!(
            "Garage AI\n",
            "Project: garage-ai\n",
            "Workspace: C:\\Garage\\Project\n",
            "Context references: 1\n",
            "Status: ready\n",
            "\n",
            "System\n",
            "  ready    core         Gareji Core is installed\n",
            "  ready    project      the project registration matches\n",
            "\n",
            "Recent activity\n",
            "  No activity recorded.\n",
            "\n",
            "Next: complete a Codex turn in this workspace.\n",
        )
    );
}

#[test]
fn status_report_bounds_changed_paths_and_preserves_plain_status_text() {
    let report = StatusReport {
        project_id: "garage-ai".to_owned(),
        project_name: "Garage AI".to_owned(),
        workspace: "/work/garage-ai".to_owned(),
        context_references: 2,
        health: ready_health(),
        recent_activity: vec![ActivityReport {
            checkpoint_id: "checkpoint-1".to_owned(),
            recorded_at: "2026-07-21T01:02:03Z".to_owned(),
            outcome: "completed".to_owned(),
            summary: "Refined the status output".to_owned(),
            changed_paths: (1..=7)
                .map(|index| format!("src/file-{index}.rs"))
                .collect(),
        }],
    };

    assert_eq!(
        render_status_report(&report),
        concat!(
            "Garage AI\n",
            "Project: garage-ai\n",
            "Workspace: /work/garage-ai\n",
            "Context references: 2\n",
            "Status: ready\n",
            "\n",
            "System\n",
            "  ready    core         Gareji Core is installed\n",
            "  ready    project      the project registration matches\n",
            "\n",
            "Recent activity\n",
            "  2026-07-21T01:02:03Z  completed\n",
            "    Refined the status output\n",
            "    Changed\n",
            "      src/file-1.rs\n",
            "      src/file-2.rs\n",
            "      src/file-3.rs\n",
            "      src/file-4.rs\n",
            "      src/file-5.rs\n",
            "      ... and 2 more\n",
        )
    );
}

#[test]
fn status_report_json_contract_is_unchanged() {
    let report = StatusReport {
        project_id: "garage-ai".to_owned(),
        project_name: "Garage AI".to_owned(),
        workspace: "/work/garage-ai".to_owned(),
        context_references: 1,
        health: SetupReport {
            project_id: Some("garage-ai".to_owned()),
            steps: vec![step(
                StepKind::Core,
                StepStatus::Missing,
                "Gareji Core is missing",
            )],
            restart_codex: false,
            healthy: false,
        },
        recent_activity: Vec::new(),
    };

    assert_eq!(
        serde_json::to_string(&report).unwrap(),
        r#"{"project_id":"garage-ai","project_name":"Garage AI","workspace":"/work/garage-ai","context_references":1,"health":{"project_id":"garage-ai","steps":[{"step":"core","status":"missing","message":"Gareji Core is missing"}],"restart_codex":false,"healthy":false},"recent_activity":[]}"#
    );
}

#[test]
fn setup_report_distinguishes_a_dry_run_from_applied_changes() {
    let report = SetupReport {
        project_id: Some("garage-ai".to_owned()),
        steps: vec![step(
            StepKind::Project,
            StepStatus::Planned,
            "would register the project",
        )],
        restart_codex: false,
        healthy: true,
    };

    assert_eq!(
        render_setup_report(&report),
        concat!(
            "Gareji\n",
            "Project: garage-ai\n",
            "Status: planned\n",
            "\n",
            "System\n",
            "  planned  project      would register the project\n",
            "\n",
            "Next: rerun this command without --dry-run to apply the setup.\n",
        )
    );
}

#[test]
fn status_report_prioritizes_repair_when_system_health_is_missing() {
    let report = StatusReport {
        project_id: "garage-ai".to_owned(),
        project_name: "Garage AI".to_owned(),
        workspace: r"\\?\UNC\server\share\Project".to_owned(),
        context_references: 1,
        health: SetupReport {
            project_id: Some("garage-ai".to_owned()),
            steps: vec![step(
                StepKind::Plugin,
                StepStatus::Missing,
                "the Gareji Progress Plugin is missing",
            )],
            restart_codex: false,
            healthy: false,
        },
        recent_activity: Vec::new(),
    };

    assert_eq!(
        render_status_report(&report),
        concat!(
            "Garage AI\n",
            "Project: garage-ai\n",
            "Workspace: \\\\server\\share\\Project\n",
            "Context references: 1\n",
            "Status: needs attention\n",
            "\n",
            "System\n",
            "  missing  plugin       the Gareji Progress Plugin is missing\n",
            "\n",
            "Recent activity\n",
            "  No activity recorded.\n",
            "\n",
            "Next: run `gareji setup` for this workspace, then rerun `gareji status`.\n",
        )
    );
}
