//! Idempotent first-run setup for a local Gareji installation.

use std::ffi::{OsStr, OsString};
use std::fs::{self, File};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use anyhow::{anyhow, bail, Context, Result};
use gareji_contracts::{
    CheckpointStatusResult, ListProgressResult, ProjectGrant, ProjectRegistration, SourcedContext,
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use tempfile::NamedTempFile;

const MAX_COMMAND_OUTPUT_BYTES: usize = 1_048_576;
const MARKETPLACE_NAME: &str = "gareji-local";
const PLUGIN_ID: &str = "gareji-progress@gareji-local";

/// One requested local installation and project registration.
#[derive(Clone, Debug)]
pub struct SetupRequest {
    /// Stable project identity. When omitted, Setup derives one from the workspace name.
    pub project_id: Option<String>,
    /// Human-readable project name. When omitted, Setup uses the workspace name.
    pub project_name: Option<String>,
    /// Absolute project workspace directory.
    pub workspace: PathBuf,
    /// One or more absolute Markdown context paths.
    pub context_paths: Vec<PathBuf>,
    /// Gareji Core binary to install.
    pub core_source: PathBuf,
    /// Directory that receives the installed Core binary.
    pub install_dir: PathBuf,
    /// Codex CLI executable.
    pub codex_binary: PathBuf,
    /// Local marketplace root containing `.agents/plugins/marketplace.json`.
    pub marketplace_root: PathBuf,
    /// Report intended changes without writing or invoking mutating commands.
    pub dry_run: bool,
    /// Replace a differing registration with the same project identity.
    pub replace_project: bool,
}

/// Inputs for a non-repairing Gareji installation diagnosis.
#[derive(Clone, Debug)]
pub struct DoctorRequest {
    /// Directory that should contain the installed Core binary.
    pub install_dir: PathBuf,
    /// Codex CLI executable.
    pub codex_binary: PathBuf,
    /// Optional project identity that should be registered.
    pub project_id: Option<String>,
}

/// Inputs for a project-centered, non-repairing status view.
#[derive(Clone, Debug)]
pub struct StatusRequest {
    /// Registered project workspace. Relative paths are resolved before matching.
    pub workspace: PathBuf,
    /// Directory that should contain the installed Core binary.
    pub install_dir: PathBuf,
    /// Codex CLI executable.
    pub codex_binary: PathBuf,
    /// Maximum number of recent Progress Checkpoints to return.
    pub limit: u16,
}

/// Result of one setup or diagnosis step.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum StepStatus {
    /// The desired state was already present.
    Ready,
    /// Setup changed local state.
    Changed,
    /// Dry-run identified a change that would be made.
    Planned,
    /// Doctor could not find the desired state.
    Missing,
}

/// One bounded setup or diagnosis observation.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct StepReport {
    /// Stable step name.
    pub step: String,
    /// Current step status.
    pub status: StepStatus,
    /// Bounded human-readable explanation.
    pub message: String,
}

/// Complete bounded report returned by Setup or Doctor.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct SetupReport {
    /// Project identity, when the operation was project-scoped.
    pub project_id: Option<String>,
    /// Ordered setup or diagnosis steps.
    pub steps: Vec<StepReport>,
    /// Whether Codex must restart and review the installed Hook.
    pub restart_codex: bool,
    /// Whether every diagnosed requirement is ready.
    pub healthy: bool,
}

/// One human-oriented, bounded progress observation.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct ActivityReport {
    /// Stable immutable checkpoint identity.
    pub checkpoint_id: String,
    /// UTC timestamp supplied by the accepted checkpoint.
    pub recorded_at: String,
    /// Stable checkpoint outcome.
    pub outcome: String,
    /// Bounded checkpoint summary.
    pub summary: String,
    /// Workspace-relative paths changed during the recorded turn.
    pub changed_paths: Vec<String>,
}

/// Project-centered status returned by the user-facing `gareji status` Interface.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct StatusReport {
    /// Stable registered project identity.
    pub project_id: String,
    /// Human-readable registered project name.
    pub project_name: String,
    /// Canonical registered execution workspace.
    pub workspace: String,
    /// Number of attributed context references configured for the project.
    pub context_references: usize,
    /// Installation and integration health.
    pub health: SetupReport,
    /// Newest accepted project progress, in intake order.
    pub recent_activity: Vec<ActivityReport>,
}

/// Bounded output captured from one local command.
#[derive(Clone, Debug)]
pub struct CommandOutput {
    /// Process exit status represented as a success flag.
    pub success: bool,
    /// Bounded stdout bytes.
    pub stdout: Vec<u8>,
}

/// Internal command-execution seam used by the Setup implementation and its tests.
pub trait CommandRunner {
    /// Run one local executable with explicit arguments and bounded output.
    fn run(&mut self, program: &Path, args: &[OsString]) -> Result<CommandOutput>;
}

/// Production command runner.
#[derive(Default)]
pub struct SystemCommandRunner;

impl CommandRunner for SystemCommandRunner {
    fn run(&mut self, program: &Path, args: &[OsString]) -> Result<CommandOutput> {
        let mut command = system_command(program);
        let output = command
            .args(args)
            .output()
            .with_context(|| format!("could not start {}", program.display()))?;
        bounded_output(output)
    }
}

/// Apply or plan one complete first-run setup.
pub fn setup(request: &SetupRequest, runner: &mut impl CommandRunner) -> Result<SetupReport> {
    let prepared = PreparedSetup::new(request)?;
    let mut steps = Vec::new();
    let core_changed = binary_needs_install(&prepared.core_source, &prepared.core_destination)?;

    if request.dry_run {
        steps.push(step(
            "core",
            if core_changed {
                StepStatus::Planned
            } else {
                StepStatus::Ready
            },
            if core_changed {
                "install Gareji Core in the local application-data directory"
            } else {
                "installed Gareji Core already matches the requested binary"
            },
        ));
        steps.push(step(
            "project",
            StepStatus::Planned,
            "create or replace the idempotent local project registration",
        ));
        steps.push(step(
            "marketplace",
            StepStatus::Planned,
            "ensure the Gareji local Codex marketplace is configured",
        ));
        steps.push(step(
            "plugin",
            StepStatus::Planned,
            "ensure the requested Gareji Progress Codex Plugin version is installed",
        ));
        return Ok(SetupReport {
            project_id: Some(prepared.registration.project_id),
            steps,
            restart_codex: false,
            healthy: true,
        });
    }

    if core_changed {
        install_binary(&prepared.core_source, &prepared.core_destination)?;
        steps.push(step(
            "core",
            StepStatus::Changed,
            "installed Gareji Core in the local application-data directory",
        ));
    } else {
        steps.push(step(
            "core",
            StepStatus::Ready,
            "installed Gareji Core already matches the requested binary",
        ));
    }

    let project_changed = ensure_project(
        &prepared.core_destination,
        &prepared.registration,
        request.replace_project,
        runner,
    )?;
    steps.push(step(
        "project",
        if project_changed {
            StepStatus::Changed
        } else {
            StepStatus::Ready
        },
        if project_changed {
            "registered the project with local Markdown context references"
        } else {
            "the project registration already matches the requested configuration"
        },
    ));

    let marketplace_changed =
        ensure_marketplace(&prepared.codex_binary, &prepared.marketplace_root, runner)?;
    steps.push(step(
        "marketplace",
        if marketplace_changed {
            StepStatus::Changed
        } else {
            StepStatus::Ready
        },
        if marketplace_changed {
            "added the Gareji local Codex marketplace"
        } else {
            "the Gareji local Codex marketplace is already configured"
        },
    ));

    let plugin_changed = ensure_plugin(&prepared.codex_binary, &prepared.plugin_version, runner)?;
    steps.push(step(
        "plugin",
        if plugin_changed {
            StepStatus::Changed
        } else {
            StepStatus::Ready
        },
        if plugin_changed {
            "installed or updated the Gareji Progress Codex Plugin"
        } else {
            "the Gareji Progress Codex Plugin is already installed and enabled"
        },
    ));

    Ok(SetupReport {
        project_id: Some(prepared.registration.project_id),
        steps,
        restart_codex: marketplace_changed || plugin_changed,
        healthy: true,
    })
}

/// Diagnose the installed Core, optional project registration, and Codex Plugin without repair.
pub fn doctor(request: &DoctorRequest, runner: &mut impl CommandRunner) -> Result<SetupReport> {
    let core = request.install_dir.join(core_binary_name());
    let mut steps = Vec::new();
    let core_ready = core.is_file();
    steps.push(step(
        "core",
        ready_or_missing(core_ready),
        if core_ready {
            "the installed Gareji Core binary is present"
        } else {
            "the installed Gareji Core binary is missing"
        },
    ));

    let mut project_ready = request.project_id.is_none();
    if let Some(project_id) = &request.project_id {
        if core_ready {
            project_ready = list_projects(&core, runner)?
                .iter()
                .any(|project| &project.project_id == project_id);
        }
        steps.push(step(
            "project",
            ready_or_missing(project_ready),
            if project_ready {
                "the requested project is registered"
            } else {
                "the requested project is not registered"
            },
        ));
    }

    let marketplaces = list_marketplaces(&request.codex_binary, runner)?;
    let marketplace_ready = marketplaces
        .marketplaces
        .iter()
        .any(|marketplace| marketplace.name == MARKETPLACE_NAME);
    steps.push(step(
        "marketplace",
        ready_or_missing(marketplace_ready),
        if marketplace_ready {
            "the Gareji local Codex marketplace is configured"
        } else {
            "the Gareji local Codex marketplace is missing"
        },
    ));

    let plugin_ready = plugin_is_ready(&request.codex_binary, None, runner)?;
    steps.push(step(
        "plugin",
        ready_or_missing(plugin_ready),
        if plugin_ready {
            "the Gareji Progress Codex Plugin is installed and enabled"
        } else {
            "the Gareji Progress Codex Plugin is missing or disabled"
        },
    ));

    Ok(SetupReport {
        project_id: request.project_id.clone(),
        steps,
        restart_codex: false,
        healthy: core_ready && project_ready && marketplace_ready && plugin_ready,
    })
}

/// Inspect one registered workspace and its newest accepted progress without repair.
pub fn status(request: &StatusRequest, runner: &mut impl CommandRunner) -> Result<StatusReport> {
    if !(1..=100).contains(&request.limit) {
        bail!("status limit must be from 1 through 100");
    }
    let core = request.install_dir.join(core_binary_name());
    if !core.is_file() {
        bail!("Gareji Core is not installed; run gareji setup first");
    }
    let workspace = request
        .workspace
        .canonicalize()
        .context("could not resolve the status workspace")?;
    let project = list_projects(&core, runner)?
        .into_iter()
        .find(|project| same_path(Path::new(&project.execution_workspace), &workspace))
        .context("this workspace is not registered; run gareji setup first")?;
    let health = doctor(
        &DoctorRequest {
            install_dir: request.install_dir.clone(),
            codex_binary: request.codex_binary.clone(),
            project_id: Some(project.project_id.clone()),
        },
        runner,
    )?;
    let progress: ListProgressResult = run_json(
        runner,
        &core,
        &[
            OsString::from("progress"),
            OsString::from("list"),
            OsString::from("--project-id"),
            OsString::from(&project.project_id),
            OsString::from("--limit"),
            OsString::from(request.limit.to_string()),
        ],
        "Core progress list",
    )?;
    let recent_activity = progress
        .checkpoints
        .into_iter()
        .map(activity_report)
        .collect::<Result<Vec<_>>>()?;

    Ok(StatusReport {
        project_id: project.project_id,
        project_name: project.name,
        workspace: project.execution_workspace,
        context_references: project.sourced_context.len(),
        health,
        recent_activity,
    })
}

fn activity_report(status: CheckpointStatusResult) -> Result<ActivityReport> {
    #[derive(Deserialize)]
    struct Payload {
        recorded_at: String,
        outcome: String,
        summary: String,
        #[serde(default)]
        changed_paths: Vec<String>,
    }

    let payload: Payload = serde_json::from_value(status.checkpoint)
        .context("Core returned an invalid Progress Checkpoint payload")?;
    Ok(ActivityReport {
        checkpoint_id: status.checkpoint_id,
        recorded_at: payload.recorded_at,
        outcome: payload.outcome,
        summary: payload.summary,
        changed_paths: payload.changed_paths,
    })
}

/// Resolve the platform-specific application-data directory used by Gareji.
pub fn default_data_dir() -> Result<PathBuf> {
    let base = if cfg!(windows) {
        environment_path("LOCALAPPDATA")
    } else if cfg!(target_os = "macos") {
        environment_path("HOME").map(|path| path.join("Library/Application Support"))
    } else {
        environment_path("XDG_DATA_HOME")
            .or_else(|| environment_path("HOME").map(|path| path.join(".local/share")))
    }
    .context("could not resolve the local application-data directory")?;
    Ok(base.join("Gareji"))
}

/// Find a Gareji local marketplace by walking upward from a starting path.
pub fn discover_marketplace_root(start: &Path) -> Option<PathBuf> {
    let start = if start.is_file() {
        start.parent()?
    } else {
        start
    };
    start
        .ancestors()
        .find(|candidate| {
            candidate.join(".agents/plugins/marketplace.json").is_file()
                && candidate
                    .join("plugins/gareji-progress/.codex-plugin/plugin.json")
                    .is_file()
        })
        .map(Path::to_path_buf)
}

/// Derive a bounded project identity from a workspace directory name.
pub fn derive_project_id(workspace: &Path) -> Result<String> {
    let name = workspace
        .file_name()
        .and_then(OsStr::to_str)
        .context("workspace has no usable directory name")?;
    let mut id = String::new();
    let mut previous_hyphen = false;
    for character in name.chars() {
        if character.is_ascii_alphanumeric() {
            id.push(character.to_ascii_lowercase());
            previous_hyphen = false;
        } else if !previous_hyphen && !id.is_empty() {
            id.push('-');
            previous_hyphen = true;
        }
    }
    while id.ends_with('-') {
        id.pop();
    }
    if id.is_empty() {
        bail!("could not derive an ASCII project identity; pass --project-id");
    }
    Ok(id)
}

#[derive(Debug)]
struct PreparedSetup {
    registration: ProjectRegistration,
    core_source: PathBuf,
    core_destination: PathBuf,
    codex_binary: PathBuf,
    marketplace_root: PathBuf,
    plugin_version: String,
}

impl PreparedSetup {
    fn new(request: &SetupRequest) -> Result<Self> {
        let workspace = canonical_existing_directory(&request.workspace, "workspace")?;
        if request.context_paths.is_empty() {
            bail!("at least one --context path is required");
        }
        let context_paths = request
            .context_paths
            .iter()
            .map(|path| canonical_markdown(path))
            .collect::<Result<Vec<_>>>()?;
        let core_source = canonical_existing_file(&request.core_source, "Core source")?;
        let marketplace_root =
            canonical_existing_directory(&request.marketplace_root, "marketplace root")?;
        validate_marketplace_root(&marketplace_root)?;
        let plugin_version = read_plugin_version(&marketplace_root)?;
        if !request.install_dir.is_absolute() {
            bail!("install directory must be absolute");
        }

        let project_id = request
            .project_id
            .clone()
            .map_or_else(|| derive_project_id(&workspace), Ok)?;
        let project_name = request.project_name.clone().unwrap_or_else(|| {
            workspace
                .file_name()
                .and_then(OsStr::to_str)
                .unwrap_or(&project_id)
                .to_owned()
        });
        let sourced_context = context_paths
            .iter()
            .enumerate()
            .map(|(index, path)| SourcedContext {
                workspace_id: "local-markdown".to_owned(),
                source_id: context_source_id(path, index),
                freshness: "configured".to_owned(),
                content: None,
                evidence_ref: Some(path.to_string_lossy().into_owned()),
            })
            .collect();
        let registration = ProjectRegistration {
            project_id,
            name: project_name,
            execution_workspace: workspace.to_string_lossy().into_owned(),
            context_sources: vec!["local-markdown".to_owned()],
            grants: vec![ProjectGrant::ReadContext, ProjectGrant::WriteProgress],
            sourced_context,
            delivery_targets: vec![],
        };

        Ok(Self {
            registration,
            core_source,
            core_destination: request.install_dir.join(core_binary_name()),
            codex_binary: request.codex_binary.clone(),
            marketplace_root,
            plugin_version,
        })
    }
}

fn context_source_id(path: &Path, index: usize) -> String {
    let raw = path
        .file_stem()
        .and_then(OsStr::to_str)
        .unwrap_or("context");
    let mut id: String = raw
        .chars()
        .filter_map(|character| {
            if character.is_ascii_alphanumeric() {
                Some(character.to_ascii_lowercase())
            } else if character == '-' || character == '_' {
                Some(character)
            } else {
                None
            }
        })
        .take(48)
        .collect();
    if id.is_empty() {
        id = "context".to_owned();
    }
    format!("{id}-{}", index + 1)
}

fn canonical_existing_directory(path: &Path, label: &str) -> Result<PathBuf> {
    if !path.is_absolute() {
        bail!("{label} path must be absolute");
    }
    let canonical = path
        .canonicalize()
        .with_context(|| format!("{label} path does not exist"))?;
    if !canonical.is_dir() {
        bail!("{label} path is not a directory");
    }
    Ok(canonical)
}

fn canonical_existing_file(path: &Path, label: &str) -> Result<PathBuf> {
    if !path.is_absolute() {
        bail!("{label} path must be absolute");
    }
    let canonical = path
        .canonicalize()
        .with_context(|| format!("{label} path does not exist"))?;
    if !canonical.is_file() {
        bail!("{label} path is not a file");
    }
    Ok(canonical)
}

fn canonical_markdown(path: &Path) -> Result<PathBuf> {
    let canonical = canonical_existing_file(path, "context")?;
    let is_markdown = canonical
        .extension()
        .and_then(OsStr::to_str)
        .is_some_and(|extension| extension.eq_ignore_ascii_case("md"));
    if !is_markdown {
        bail!("context path must reference a Markdown file");
    }
    Ok(canonical)
}

fn validate_marketplace_root(root: &Path) -> Result<()> {
    if !root.join(".agents/plugins/marketplace.json").is_file() {
        bail!("marketplace root has no .agents/plugins/marketplace.json");
    }
    if !root
        .join("plugins/gareji-progress/.codex-plugin/plugin.json")
        .is_file()
    {
        bail!("marketplace root has no Gareji Progress Plugin");
    }
    Ok(())
}

fn read_plugin_version(root: &Path) -> Result<String> {
    let manifest_path = root.join("plugins/gareji-progress/.codex-plugin/plugin.json");
    let manifest_file = File::open(&manifest_path)
        .with_context(|| format!("could not open {}", manifest_path.display()))?;
    let manifest: PluginManifest = serde_json::from_reader(manifest_file)
        .with_context(|| format!("could not parse {}", manifest_path.display()))?;
    let version = manifest.version.trim();
    if version.is_empty() || version.len() > 128 {
        bail!("Gareji Progress Plugin version must contain 1 to 128 characters");
    }
    Ok(version.to_owned())
}

fn binary_needs_install(source: &Path, destination: &Path) -> Result<bool> {
    if !destination.is_file() {
        return Ok(true);
    }
    Ok(file_sha256(source)? != file_sha256(destination)?)
}

fn install_binary(source: &Path, destination: &Path) -> Result<()> {
    let parent = destination
        .parent()
        .context("installed Core path has no parent directory")?;
    fs::create_dir_all(parent).context("could not create the Gareji install directory")?;
    let mut source_file = File::open(source).context("could not open the Core source binary")?;
    let mut temporary =
        NamedTempFile::new_in(parent).context("could not create a temporary Core binary")?;
    std::io::copy(&mut source_file, &mut temporary).context("could not copy the Core binary")?;
    temporary
        .as_file()
        .set_permissions(fs::metadata(source)?.permissions())?;
    temporary.as_file_mut().sync_all()?;
    temporary
        .persist(destination)
        .map_err(|error| anyhow!("could not install the Core binary: {}", error.error))?;
    Ok(())
}

fn file_sha256(path: &Path) -> Result<[u8; 32]> {
    let mut file = File::open(path)?;
    let mut digest = Sha256::new();
    let mut buffer = [0_u8; 64 * 1024];
    loop {
        let read = file.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        digest.update(&buffer[..read]);
    }
    Ok(digest.finalize().into())
}

fn register_project(
    core: &Path,
    registration: &ProjectRegistration,
    runner: &mut impl CommandRunner,
) -> Result<()> {
    let mut file = NamedTempFile::new().context("could not create registration input")?;
    serde_json::to_writer(file.as_file_mut(), registration)?;
    file.as_file_mut().flush()?;
    run_success(
        runner,
        core,
        &[
            OsString::from("project"),
            OsString::from("register"),
            OsString::from("--file"),
            file.path().as_os_str().to_owned(),
        ],
        "Core project registration",
    )?;
    Ok(())
}

fn ensure_project(
    core: &Path,
    expected: &ProjectRegistration,
    replace_project: bool,
    runner: &mut impl CommandRunner,
) -> Result<bool> {
    if let Some(existing) = list_projects(core, runner)?
        .into_iter()
        .find(|project| project.project_id == expected.project_id)
    {
        if &existing == expected {
            return Ok(false);
        }
        if !replace_project {
            bail!(
                "project is already registered with different settings; review it and pass --replace-project to update"
            );
        }
    }
    register_project(core, expected, runner)?;
    verify_project(core, expected, runner)?;
    Ok(true)
}

fn verify_project(
    core: &Path,
    expected: &ProjectRegistration,
    runner: &mut impl CommandRunner,
) -> Result<()> {
    let actual = list_projects(core, runner)?
        .into_iter()
        .find(|project| project.project_id == expected.project_id)
        .context("Core did not return the registered project")?;
    if &actual != expected {
        bail!("Core returned a project registration that differs from Setup input");
    }
    Ok(())
}

fn list_projects(core: &Path, runner: &mut impl CommandRunner) -> Result<Vec<ProjectRegistration>> {
    run_json(
        runner,
        core,
        &[OsString::from("project"), OsString::from("list")],
        "Core project list",
    )
}

fn ensure_marketplace(
    codex: &Path,
    expected_root: &Path,
    runner: &mut impl CommandRunner,
) -> Result<bool> {
    let marketplaces = list_marketplaces(codex, runner)?;
    if let Some(existing) = marketplaces
        .marketplaces
        .iter()
        .find(|marketplace| marketplace.name == MARKETPLACE_NAME)
    {
        if same_path(&existing.root, expected_root) {
            return Ok(false);
        }
        bail!("the gareji-local marketplace name is already mapped to another location");
    }
    run_success(
        runner,
        codex,
        &[
            OsString::from("plugin"),
            OsString::from("marketplace"),
            OsString::from("add"),
            expected_root.as_os_str().to_owned(),
            OsString::from("--json"),
        ],
        "Codex marketplace installation",
    )?;
    let configured = list_marketplaces(codex, runner)?
        .marketplaces
        .into_iter()
        .any(|marketplace| {
            marketplace.name == MARKETPLACE_NAME && same_path(&marketplace.root, expected_root)
        });
    if !configured {
        bail!("Codex did not retain the Gareji local marketplace");
    }
    Ok(true)
}

fn list_marketplaces(codex: &Path, runner: &mut impl CommandRunner) -> Result<MarketplaceList> {
    run_json(
        runner,
        codex,
        &[
            OsString::from("plugin"),
            OsString::from("marketplace"),
            OsString::from("list"),
            OsString::from("--json"),
        ],
        "Codex marketplace list",
    )
}

fn ensure_plugin(
    codex: &Path,
    expected_version: &str,
    runner: &mut impl CommandRunner,
) -> Result<bool> {
    let existing = installed_plugin(codex, runner)?;
    if existing.as_ref().is_some_and(|plugin| {
        plugin.installed && plugin.enabled && plugin.version == expected_version
    }) {
        return Ok(false);
    }
    if existing.is_some() {
        run_success(
            runner,
            codex,
            &[
                OsString::from("plugin"),
                OsString::from("remove"),
                OsString::from(PLUGIN_ID),
                OsString::from("--json"),
            ],
            "Codex Plugin removal before update",
        )?;
    }
    run_success(
        runner,
        codex,
        &[
            OsString::from("plugin"),
            OsString::from("add"),
            OsString::from(PLUGIN_ID),
            OsString::from("--json"),
        ],
        "Codex Plugin installation",
    )?;
    if !plugin_is_ready(codex, Some(expected_version), runner)? {
        bail!(
            "Codex did not report Gareji Progress Plugin version {expected_version} as installed and enabled"
        );
    }
    Ok(true)
}

fn installed_plugin(codex: &Path, runner: &mut impl CommandRunner) -> Result<Option<PluginEntry>> {
    let plugins: PluginList = run_json(
        runner,
        codex,
        &[
            OsString::from("plugin"),
            OsString::from("list"),
            OsString::from("--available"),
            OsString::from("--json"),
        ],
        "Codex Plugin list",
    )?;
    Ok(plugins
        .installed
        .into_iter()
        .find(|plugin| plugin.plugin_id == PLUGIN_ID))
}

fn plugin_is_ready(
    codex: &Path,
    expected_version: Option<&str>,
    runner: &mut impl CommandRunner,
) -> Result<bool> {
    Ok(installed_plugin(codex, runner)?.is_some_and(|plugin| {
        plugin.installed
            && plugin.enabled
            && expected_version.is_none_or(|version| plugin.version == version)
    }))
}

fn run_json<T: for<'de> Deserialize<'de>>(
    runner: &mut impl CommandRunner,
    program: &Path,
    args: &[OsString],
    operation: &str,
) -> Result<T> {
    let output = run_success(runner, program, args, operation)?;
    serde_json::from_slice(&output.stdout)
        .with_context(|| format!("{operation} returned invalid JSON"))
}

fn run_success(
    runner: &mut impl CommandRunner,
    program: &Path,
    args: &[OsString],
    operation: &str,
) -> Result<CommandOutput> {
    let output = runner.run(program, args)?;
    if !output.success {
        bail!("{operation} failed");
    }
    Ok(output)
}

fn bounded_output(output: Output) -> Result<CommandOutput> {
    if output.stdout.len() > MAX_COMMAND_OUTPUT_BYTES
        || output.stderr.len() > MAX_COMMAND_OUTPUT_BYTES
    {
        bail!("a setup command returned too much output");
    }
    Ok(CommandOutput {
        success: output.status.success(),
        stdout: output.stdout,
    })
}

fn system_command(program: &Path) -> Command {
    let is_powershell_script = cfg!(windows)
        && program
            .extension()
            .and_then(OsStr::to_str)
            .is_some_and(|extension| extension.eq_ignore_ascii_case("ps1"));
    if is_powershell_script {
        let mut command = Command::new("powershell.exe");
        command.args([
            "-NoProfile",
            "-NonInteractive",
            "-ExecutionPolicy",
            "Bypass",
            "-File",
        ]);
        command.arg(shell_compatible_path(program));
        command
    } else {
        Command::new(program)
    }
}

fn shell_compatible_path(path: &Path) -> PathBuf {
    if !cfg!(windows) {
        return path.to_path_buf();
    }
    let value = path.to_string_lossy();
    if let Some(rest) = value.strip_prefix(r"\\?\UNC\") {
        return PathBuf::from(format!(r"\\{rest}"));
    }
    value
        .strip_prefix(r"\\?\")
        .map_or_else(|| path.to_path_buf(), PathBuf::from)
}

fn same_path(left: &Path, right: &Path) -> bool {
    let normalize = |path: &Path| path.canonicalize().unwrap_or_else(|_| path.to_path_buf());
    normalize(left) == normalize(right)
}

fn step(step_name: &str, status: StepStatus, message: &str) -> StepReport {
    StepReport {
        step: step_name.to_owned(),
        status,
        message: message.to_owned(),
    }
}

fn ready_or_missing(ready: bool) -> StepStatus {
    if ready {
        StepStatus::Ready
    } else {
        StepStatus::Missing
    }
}

fn core_binary_name() -> &'static str {
    if cfg!(windows) {
        "gareji-core.exe"
    } else {
        "gareji-core"
    }
}

fn environment_path(name: &str) -> Option<PathBuf> {
    std::env::var_os(name)
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
}

#[derive(Debug, Deserialize)]
struct MarketplaceList {
    #[serde(default)]
    marketplaces: Vec<MarketplaceEntry>,
}

#[derive(Debug, Deserialize)]
struct MarketplaceEntry {
    name: String,
    root: PathBuf,
}

#[derive(Debug, Deserialize)]
struct PluginList {
    #[serde(default)]
    installed: Vec<PluginEntry>,
}

#[derive(Debug, Deserialize)]
struct PluginManifest {
    version: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct PluginEntry {
    plugin_id: String,
    #[serde(default)]
    version: String,
    installed: bool,
    enabled: bool,
}

#[cfg(test)]
mod tests {
    use std::collections::VecDeque;

    use super::*;

    struct ScriptedRunner {
        outputs: VecDeque<CommandOutput>,
        calls: Vec<Vec<String>>,
    }

    impl ScriptedRunner {
        fn new(outputs: Vec<CommandOutput>) -> Self {
            Self {
                outputs: outputs.into(),
                calls: vec![],
            }
        }
    }

    impl CommandRunner for ScriptedRunner {
        fn run(&mut self, program: &Path, args: &[OsString]) -> Result<CommandOutput> {
            let mut call = vec![program.to_string_lossy().into_owned()];
            call.extend(
                args.iter()
                    .map(|argument| argument.to_string_lossy().into_owned()),
            );
            self.calls.push(call);
            self.outputs
                .pop_front()
                .context("test command script was exhausted")
        }
    }

    fn json_output(value: serde_json::Value) -> CommandOutput {
        CommandOutput {
            success: true,
            stdout: serde_json::to_vec(&value).unwrap(),
        }
    }

    fn empty_success() -> CommandOutput {
        CommandOutput {
            success: true,
            stdout: vec![],
        }
    }

    fn fixture() -> (tempfile::TempDir, SetupRequest, ProjectRegistration) {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path().canonicalize().unwrap();
        let workspace = root.join("Sample Project");
        fs::create_dir(&workspace).unwrap();
        let context = workspace.join("PROJECT.md");
        fs::write(&context, "# Project\n").unwrap();
        let core_source = root.join(core_binary_name());
        fs::write(&core_source, b"core-binary").unwrap();
        let marketplace_root = root.join("distribution");
        fs::create_dir_all(marketplace_root.join(".agents/plugins")).unwrap();
        fs::create_dir_all(marketplace_root.join("plugins/gareji-progress/.codex-plugin")).unwrap();
        fs::write(
            marketplace_root.join(".agents/plugins/marketplace.json"),
            "{}",
        )
        .unwrap();
        fs::write(
            marketplace_root.join("plugins/gareji-progress/.codex-plugin/plugin.json"),
            r#"{"version":"0.1.1"}"#,
        )
        .unwrap();
        let request = SetupRequest {
            project_id: None,
            project_name: None,
            workspace: workspace.clone(),
            context_paths: vec![context.clone()],
            core_source,
            install_dir: root.join("installed"),
            codex_binary: PathBuf::from("codex"),
            marketplace_root: marketplace_root.clone(),
            dry_run: false,
            replace_project: false,
        };
        let registration = ProjectRegistration {
            project_id: "sample-project".to_owned(),
            name: "Sample Project".to_owned(),
            execution_workspace: workspace
                .canonicalize()
                .unwrap()
                .to_string_lossy()
                .into_owned(),
            context_sources: vec!["local-markdown".to_owned()],
            grants: vec![ProjectGrant::ReadContext, ProjectGrant::WriteProgress],
            sourced_context: vec![SourcedContext {
                workspace_id: "local-markdown".to_owned(),
                source_id: "project-1".to_owned(),
                freshness: "configured".to_owned(),
                content: None,
                evidence_ref: Some(
                    context
                        .canonicalize()
                        .unwrap()
                        .to_string_lossy()
                        .into_owned(),
                ),
            }],
            delivery_targets: vec![],
        };
        (directory, request, registration)
    }

    #[test]
    fn dry_run_plans_without_invoking_commands_or_writing() {
        let (_directory, mut request, _registration) = fixture();
        request.dry_run = true;
        let destination = request.install_dir.join(core_binary_name());
        let mut runner = ScriptedRunner::new(vec![]);

        let report = setup(&request, &mut runner).unwrap();

        assert!(report.healthy);
        assert!(!destination.exists());
        assert!(runner.calls.is_empty());
        assert!(report
            .steps
            .iter()
            .all(|step| matches!(step.status, StepStatus::Planned | StepStatus::Ready)));
    }

    #[test]
    fn setup_installs_registers_and_verifies_every_layer() {
        let (_directory, request, registration) = fixture();
        let root = request.marketplace_root.canonicalize().unwrap();
        let mut runner = ScriptedRunner::new(vec![
            json_output(serde_json::json!([])),
            empty_success(),
            json_output(serde_json::to_value(vec![&registration]).unwrap()),
            json_output(serde_json::json!({"marketplaces": []})),
            empty_success(),
            json_output(serde_json::json!({
                "marketplaces": [{"name": MARKETPLACE_NAME, "root": root}]
            })),
            json_output(serde_json::json!({"installed": []})),
            empty_success(),
            json_output(serde_json::json!({
                "installed": [{
                    "pluginId": PLUGIN_ID,
                    "version": "0.1.1",
                    "installed": true,
                    "enabled": true
                }]
            })),
        ]);

        let report = setup(&request, &mut runner).unwrap();

        assert!(report.healthy);
        assert!(report.restart_codex);
        assert!(request.install_dir.join(core_binary_name()).is_file());
        assert_eq!(runner.calls.len(), 9);
        assert!(runner.calls[1].iter().any(|part| part == "register"));
        assert!(runner.calls[4].iter().any(|part| part == "add"));
        assert!(runner.calls[7].iter().any(|part| part == PLUGIN_ID));
    }

    #[test]
    fn setup_is_idempotent_when_everything_is_ready() {
        let (_directory, request, registration) = fixture();
        fs::create_dir_all(&request.install_dir).unwrap();
        fs::copy(
            &request.core_source,
            request.install_dir.join(core_binary_name()),
        )
        .unwrap();
        let root = request.marketplace_root.canonicalize().unwrap();
        let mut runner = ScriptedRunner::new(vec![
            json_output(serde_json::to_value(vec![&registration]).unwrap()),
            json_output(serde_json::json!({
                "marketplaces": [{"name": MARKETPLACE_NAME, "root": root}]
            })),
            json_output(serde_json::json!({
                "installed": [{
                    "pluginId": PLUGIN_ID,
                    "version": "0.1.1",
                    "installed": true,
                    "enabled": true
                }]
            })),
        ]);

        let report = setup(&request, &mut runner).unwrap();

        assert!(!report.restart_codex);
        assert_eq!(runner.calls.len(), 3);
        assert_eq!(report.steps[0].status, StepStatus::Ready);
        assert_eq!(report.steps[1].status, StepStatus::Ready);
        assert_eq!(report.steps[2].status, StepStatus::Ready);
        assert_eq!(report.steps[3].status, StepStatus::Ready);
    }

    #[test]
    fn setup_reinstalls_an_outdated_plugin_from_the_local_marketplace() {
        let (_directory, request, registration) = fixture();
        fs::create_dir_all(&request.install_dir).unwrap();
        fs::copy(
            &request.core_source,
            request.install_dir.join(core_binary_name()),
        )
        .unwrap();
        let root = request.marketplace_root.canonicalize().unwrap();
        let mut runner = ScriptedRunner::new(vec![
            json_output(serde_json::to_value(vec![&registration]).unwrap()),
            json_output(serde_json::json!({
                "marketplaces": [{"name": MARKETPLACE_NAME, "root": root}]
            })),
            json_output(serde_json::json!({
                "installed": [{
                    "pluginId": PLUGIN_ID,
                    "version": "0.1.0",
                    "installed": true,
                    "enabled": true
                }]
            })),
            empty_success(),
            empty_success(),
            json_output(serde_json::json!({
                "installed": [{
                    "pluginId": PLUGIN_ID,
                    "version": "0.1.1",
                    "installed": true,
                    "enabled": true
                }]
            })),
        ]);

        let report = setup(&request, &mut runner).unwrap();

        assert!(report.restart_codex);
        assert_eq!(report.steps[3].status, StepStatus::Changed);
        assert_eq!(runner.calls[3][1..4], ["plugin", "remove", PLUGIN_ID]);
        assert_eq!(runner.calls[4][1..4], ["plugin", "add", PLUGIN_ID]);
    }

    #[test]
    fn rejects_relative_or_non_markdown_context_paths() {
        let (_directory, mut request, _registration) = fixture();
        request.context_paths = vec![PathBuf::from("PROJECT.md")];
        let mut runner = ScriptedRunner::new(vec![]);
        assert!(setup(&request, &mut runner)
            .unwrap_err()
            .to_string()
            .contains("absolute"));

        let text = request.workspace.join("PROJECT.txt");
        fs::write(&text, "project").unwrap();
        request.context_paths = vec![text];
        assert!(setup(&request, &mut runner)
            .unwrap_err()
            .to_string()
            .contains("Markdown"));
    }

    #[test]
    fn binary_install_atomically_replaces_an_outdated_copy() {
        let directory = tempfile::tempdir().unwrap();
        let source = directory.path().join("source-core");
        let destination = directory.path().join("installed/core");
        fs::create_dir_all(destination.parent().unwrap()).unwrap();
        fs::write(&source, b"new-core").unwrap();
        fs::write(&destination, b"old-core").unwrap();

        assert!(binary_needs_install(&source, &destination).unwrap());
        install_binary(&source, &destination).unwrap();

        assert_eq!(fs::read(destination).unwrap(), b"new-core");
    }

    #[cfg(windows)]
    #[test]
    fn powershell_receives_a_non_verbatim_windows_path() {
        assert_eq!(
            shell_compatible_path(Path::new(r"\\?\C:\tools\codex.ps1")),
            PathBuf::from(r"C:\tools\codex.ps1")
        );
    }

    #[test]
    fn detects_marketplace_name_conflicts_without_mutating() {
        let (_directory, request, registration) = fixture();
        let mut runner = ScriptedRunner::new(vec![
            json_output(serde_json::to_value(vec![&registration]).unwrap()),
            json_output(serde_json::json!({
                "marketplaces": [{"name": MARKETPLACE_NAME, "root": request.workspace}]
            })),
        ]);

        let error = setup(&request, &mut runner).unwrap_err();

        assert!(error.to_string().contains("another location"));
        assert_eq!(runner.calls.len(), 2);
    }

    #[test]
    fn project_conflict_requires_explicit_replacement() {
        let (_directory, request, mut registration) = fixture();
        registration.name = "Manually configured".to_owned();
        let mut runner = ScriptedRunner::new(vec![json_output(
            serde_json::to_value(vec![&registration]).unwrap(),
        )]);

        let error = setup(&request, &mut runner).unwrap_err();

        assert!(error.to_string().contains("--replace-project"));
        assert_eq!(runner.calls.len(), 1);
        assert!(!runner.calls[0].iter().any(|part| part == "register"));
    }

    #[test]
    fn marketplace_discovery_requires_both_catalog_and_plugin() {
        let (_directory, request, _registration) = fixture();
        let nested = request.marketplace_root.join("plugins/gareji-progress");
        assert_eq!(
            discover_marketplace_root(&nested).unwrap(),
            request.marketplace_root
        );
    }

    #[test]
    fn doctor_reports_every_ready_layer_without_repair_commands() {
        let (_directory, request, registration) = fixture();
        fs::create_dir_all(&request.install_dir).unwrap();
        fs::copy(
            &request.core_source,
            request.install_dir.join(core_binary_name()),
        )
        .unwrap();
        let root = request.marketplace_root.canonicalize().unwrap();
        let mut runner = ScriptedRunner::new(vec![
            json_output(serde_json::to_value(vec![&registration]).unwrap()),
            json_output(serde_json::json!({
                "marketplaces": [{"name": MARKETPLACE_NAME, "root": root}]
            })),
            json_output(serde_json::json!({
                "installed": [{
                    "pluginId": PLUGIN_ID,
                    "version": "0.1.1",
                    "installed": true,
                    "enabled": true
                }]
            })),
        ]);

        let report = doctor(
            &DoctorRequest {
                install_dir: request.install_dir,
                codex_binary: request.codex_binary,
                project_id: Some("sample-project".to_owned()),
            },
            &mut runner,
        )
        .unwrap();

        assert!(report.healthy);
        assert_eq!(runner.calls.len(), 3);
        assert!(runner
            .calls
            .iter()
            .all(|call| { !call.iter().any(|part| part == "add" || part == "register") }));
    }

    #[test]
    fn status_combines_health_context_and_recent_progress() {
        let (_directory, request, registration) = fixture();
        fs::create_dir_all(&request.install_dir).unwrap();
        fs::copy(
            &request.core_source,
            request.install_dir.join(core_binary_name()),
        )
        .unwrap();
        let root = request.marketplace_root.canonicalize().unwrap();
        let projects = serde_json::to_value(vec![&registration]).unwrap();
        let mut runner = ScriptedRunner::new(vec![
            json_output(projects.clone()),
            json_output(projects),
            json_output(serde_json::json!({
                "marketplaces": [{"name": MARKETPLACE_NAME, "root": root}]
            })),
            json_output(serde_json::json!({
                "installed": [{
                    "pluginId": PLUGIN_ID,
                    "version": "0.1.1",
                    "installed": true,
                    "enabled": true
                }]
            })),
            json_output(serde_json::json!({
                "checkpoints": [{
                    "checkpoint_id": "codex-stop-1",
                    "checkpoint": {
                        "recorded_at": "2026-07-19T01:00:00Z",
                        "outcome": "progress",
                        "summary": "Codex turn ended with 2 changed paths.",
                        "changed_paths": ["README.md", "src/lib.rs"]
                    },
                    "deliveries": []
                }],
                "next_cursor": null
            })),
        ]);

        let report = status(
            &StatusRequest {
                workspace: request.workspace,
                install_dir: request.install_dir,
                codex_binary: request.codex_binary,
                limit: 5,
            },
            &mut runner,
        )
        .unwrap();

        assert!(report.health.healthy);
        assert_eq!(report.project_id, "sample-project");
        assert_eq!(report.project_name, "Sample Project");
        assert_eq!(report.context_references, 1);
        assert_eq!(report.recent_activity.len(), 1);
        assert_eq!(
            report.recent_activity[0].changed_paths,
            ["README.md", "src/lib.rs"]
        );
        assert_eq!(runner.calls.len(), 5);
        assert_eq!(runner.calls[4][1..3], ["progress", "list"]);
    }
}
