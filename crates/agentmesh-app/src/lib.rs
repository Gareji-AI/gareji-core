//! AgentMesh App manifest v0 and version-controlled toolchain pin.

mod install;
mod manifest;
mod pin;
mod resolve;
mod run_policy;
mod validate;

pub use install::{install_toolchain_bundle, InstallError, InstallReport};
pub use manifest::{
    AppConformance, AppEnv, AppLimits, AppManifest, AppPlugin, AppSchemas, AppSidecar,
    APP_MANIFEST_SCHEMA_VERSION,
};
pub use pin::{ToolchainPin, PIN_SCHEMA_VERSION, SUPPORTED_TARGETS};
pub use resolve::{
    default_toolchain_cache_root, resolve_dev_plugin, resolve_pinned_plugin, ReleaseBinary,
    ReleaseManifest, ResolveError, ResolveMode, ResolvedPlugin, RELEASE_MANIFEST_SCHEMA_VERSION,
};
pub use run_policy::{
    prepare_app_run, write_run_marker, AppRunError, AppRunMode, AppRunRequest, PreparedAppRun,
};
pub use validate::{validate_app_bundle, ValidationError, ValidationReport};
