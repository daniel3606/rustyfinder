# russh-sftp, vendored

Upstream: russh-sftp 3.0.1 from crates.io (Apache-2.0, see LICENSE). Used
through `[patch.crates-io]` in the workspace `Cargo.toml`.

RustyFinder's changes, all marked `RustyFinder:` in the source:

- `buf.rs` / `ser.rs`: names that are not valid UTF-8 are no longer
  decoded lossily. Each byte that is not part of valid UTF-8 becomes one
  character in U+10FF00..=U+10FFFF (supplementary private use area B), and
  the serializer turns those characters back into the same single bytes.
  So a name like the bytes `0xff 0x41` round-trips and can be opened,
  renamed and deleted (edge-cases spec §1, SF-09, EC-10), and a
  per-connection name encoding can be applied above this layer.
- `client/session.rs`: `read_dir` fills in owner and group names from each
  entry's longname (SFTP v3 attributes carry only the numeric ids).
- `client/session.rs`: `posix_rename`, the `posix-rename@openssh.com`
  extension, which replaces an existing target in one atomic step (SF-10).
