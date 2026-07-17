# Security policy

Gareji Core executes trusted local native Plugins with the operating-system authority of the current user. It is not a sandbox. Read [the threat model](docs/threat-model-v0.md) before enabling a Plugin or forwarding environment variables.

## Reporting

Do not open a public issue containing a vulnerability, credential, personal path, real payload, or Audit sidecar. Use GitHub private vulnerability reporting when it is enabled for the repository, or contact the maintainers through an already established private channel.

Include the affected version or revision, platform, minimal synthetic reproduction, impact, and any known workaround. Remove secrets and user data before sending evidence.

## Supported surface

Until a public release policy is published, only the current private development revision is maintained. A public compatibility and support window will be declared before the first release.
