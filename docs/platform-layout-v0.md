# Platform layout v0

Status: implemented by Gareji Setup and the Gareji Progress Stop Hook.

`plugins/gareji-progress/platform-layout.json` is the single machine-layout contract for the installed Core binary. Gareji Setup embeds that contract when it is compiled; the packaged Python Stop Hook reads the same file at runtime. Platform path changes therefore have one source and cannot silently diverge between the installer and Hook.

The contract selects the platform application-data environment variable, an optional home-directory fallback, the `Gareji` data suffix, and the Core executable name. `GAREJI_CORE_BIN` and a `gareji-core` executable on PATH still take precedence in the Hook.

The layout file contains no machine-specific absolute path, credentials, or user data and is included in the runtime Plugin package.
