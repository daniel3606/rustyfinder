# Releasing RustyFinder

A release is a GitHub release with a DMG, a ZIP and their SHA-256
checksums, plus a Homebrew cask in
[daniel3606/homebrew-tap](https://github.com/daniel3606/homebrew-tap) that
points at the ZIP. The [Release workflow](../.github/workflows/release.yml)
does all of it from a tag.

## Cutting a release

1. Bump the version: `version` in `Cargo.toml` (then `cargo check` to
   update `Cargo.lock`), and `MARKETING_VERSION` and
   `CURRENT_PROJECT_VERSION` in `RustyFinderApp/project.yml`.
2. Write what changed in `docs/releases/<version>.md`. It becomes the top of
   the release notes; download, Homebrew, checksum and signing sections are
   added below it by `scripts/release-notes.sh`.
3. Merge to `main`, then tag that commit and push the tag:

   ```bash
   git tag v1.2.3 && git push origin v1.2.3
   ```

The workflow then:

| Job | Runs on | What it does |
| --- | --- | --- |
| Build, test and package | `macos-26` (Apple silicon) | Checks the tag matches `MARKETING_VERSION`; `scripts/release.sh` (fmt, clippy, Rust tests, drag-end check, build, `scripts/package.sh`); starts the packed app once in a scratch folder |
| Publish GitHub release | Ubuntu | Re-checks the checksums and creates the release. An existing release is never overwritten: the job fails instead |
| Update Homebrew cask | Ubuntu | Downloads the published ZIP, checks it against its `.sha256`, writes `version` and `sha256` into `Casks/rustyfinder.rb` and pushes to the tap |

`scripts/release.sh` runs the same build and packaging locally, without
publishing. Its output is in `dist/`:

```
RustyFinder-<version>-macOS-arm64.dmg         drag-to-Applications installer
RustyFinder-<version>-macOS-arm64.zip         the same app, zipped (Homebrew uses this)
RustyFinder-<version>-macOS-arm64.*.sha256    one checksum per file
SHA256SUMS.txt                                both checksums
```

The DMG window is drawn by `packaging/dmg/make-background.swift` and laid out
by `packaging/dmg/settings.py` with [dmgbuild](https://github.com/dmgbuild/dmgbuild),
which `scripts/package.sh` installs into a throwaway virtualenv from
`packaging/dmg/requirements.txt` (pinned by hash). No Finder scripting is
involved, so it builds the same way on a CI runner.

Only Apple silicon is built. `scripts/build.sh` refuses to add an Intel slice
until one has been run and tested on an Intel Mac.

## Repository secrets

| Secret | Used for |
| --- | --- |
| `HOMEBREW_TAP_DEPLOY_KEY` | Private half of an SSH deploy key with write access to `daniel3606/homebrew-tap` only. Without it the cask job fails with a message and the cask must be bumped by hand |
| `DEVELOPER_ID_P12_BASE64`, `DEVELOPER_ID_P12_PASSWORD` | Optional. A "Developer ID Application" certificate and its private key, exported as .p12 and base64-encoded |
| `NOTARY_API_KEY_BASE64`, `NOTARY_API_KEY_ID`, `NOTARY_API_ISSUER` | Optional. An App Store Connect API key (.p8, base64) with its key ID and issuer ID, for `notarytool` |

To replace the deploy key: delete it under the tap's Settings › Deploy keys,
then

```bash
ssh-keygen -t ed25519 -N "" -C "rustyfinder release workflow" -f tapkey
```

```bash
gh api repos/daniel3606/homebrew-tap/keys -f title="rustyfinder release workflow" -f key="$(cat tapkey.pub)" -F read_only=false
```

```bash
gh secret set HOMEBREW_TAP_DEPLOY_KEY -R daniel3606/rustyfinder < tapkey && rm tapkey tapkey.pub
```

## Signing and notarization

**Releases are currently ad-hoc signed and not notarized.** macOS therefore
blocks the first launch of a downloaded copy (and, for Homebrew, the first
launch after each upgrade, because an ad-hoc signature changes with every
build and Homebrew carries an approval over only when the signer is the
same). Users allow it once with **Open Anyway** in System Settings › Privacy
& Security. Nothing in the app, the DMG or the cask removes the quarantine
flag or otherwise works around Gatekeeper.

Why: checked on the release Mac on 2026-11-18, the keychain holds only an
**Apple Development** certificate. That kind is for running builds on your
own devices; Apple does not accept it for notarization or for distributing
outside the App Store. There is no **Developer ID Application** certificate
and no `notarytool` credential profile.

To ship signed, notarized builds:

1. Have a paid Apple Developer Program membership. The Account Holder
   creates a **Developer ID Application** certificate (Certificates,
   Identifiers & Profiles, or Xcode › Settings › Accounts › Manage
   Certificates).
2. Export it with its private key from Keychain Access as a .p12 with a
   password, and create an App Store Connect API key (Users and Access ›
   Integrations, role Developer).
3. Add the five optional secrets above (`base64 -i file | pbcopy` gives the
   base64 text).

The next release is then signed with the hardened runtime and a secure
timestamp, the app and the DMG are notarized and their tickets stapled, and
the release notes say so. Locally the same happens when `scripts/package.sh`
finds `DEVELOPER_ID_APPLICATION` and either `NOTARY_KEYCHAIN_PROFILE` or the
three `NOTARY_API_*` variables in the environment.

What has been verified of that path without a Developer ID: the app starts
normally when signed with the hardened runtime and `packaging/entitlements.plist`
(Apple events to Finder stay allowed). The certificate import, `notarytool`
submission and stapling steps have not run yet, because there is no
certificate to run them with.
