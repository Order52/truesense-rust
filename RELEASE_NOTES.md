# Release notes

Newest first. Write what changed under "## Unreleased" as you merge changes; `tools/release.sh prepare X.Y.Z --title "..."` turns it into the section of the release, and the release workflow publishes it as the GitHub release text.

## Unreleased

- The Rust edition of TrueSense: one static program (daemon, command line, terminal UI, window, Fn-key and audio services) that replaces the Python (0.3.x) and Go (0.4.x) editions in place, with the same commands, files, control socket and kernel module.
- Install with the one-line installer or `sudo tools/install.sh`. The installer takes `bin/truesense` of a release bundle, or builds the program with cargo as your user, and keeps the previous version as `truesense.prev` until the new service has started; an older edition's files are removed only after that.
- Releases are built by CI from the tag: the bundle is reproducible (the program is built twice and compared), comes with `SHA256SUMS`, a signed build provenance attestation and a CycloneDX SBOM of the Rust crates, plus an Arch package built with cargo.
