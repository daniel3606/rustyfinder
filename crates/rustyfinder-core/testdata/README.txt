Small RAR archives from the unrar crate (https://github.com/muja/unrar.rs,
MIT OR Apache-2.0). RAR archives cannot be made without the RAR tool, so
the tests read these. Each holds one text file, "target\nCargo.lock\n".

- crypted.rar: contents encrypted with the password "unrar".
- comment-hpw-password.rar: names encrypted too, password "password".
