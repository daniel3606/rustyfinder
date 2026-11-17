//! Rules about volumes and shares that the shell applies with the
//! system's own calls: what Eject does, which shares a server offers, and
//! which folders macOS guards behind Full Disk Access.

/// What Eject on one volume should do.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EjectPlan {
    /// Ask "this volume or all of the disk's volumes?".
    Ask,
    /// Unmount just this volume.
    One,
    /// Unmount every volume of the disk and eject it.
    All,
}

/// PT-13: a disk with several mounted volumes asks first, unless Option
/// (all, without asking) or Control (this one only) is held. A
/// single-volume disk is just ejected.
pub fn eject_plan(volumes_on_disk: usize, option: bool, control: bool) -> EjectPlan {
    if volumes_on_disk <= 1 || option {
        EjectPlan::All
    } else if control {
        EjectPlan::One
    } else {
        EjectPlan::Ask
    }
}

/// The shares a server offers, from `smbutil view` output, as Finder
/// lists them: disk shares only, without hidden `$` admin shares.
pub fn smb_shares(output: &str) -> Vec<String> {
    let mut shares = Vec::new();
    let mut in_table = false;
    for line in output.lines() {
        let trimmed = line.trim_end();
        if trimmed.starts_with("Share") && trimmed.contains("Type") {
            in_table = true;
            continue;
        }
        if !in_table || trimmed.starts_with("---") || trimmed.trim().is_empty() {
            continue;
        }
        if trimmed.trim_start().chars().next().is_some_and(|c| c.is_ascii_digit()) && trimmed.contains("shares listed")
        {
            break;
        }
        // Columns: name (may contain single spaces), type, comment. The
        // type is the first of the known words that stands alone.
        let words: Vec<&str> = trimmed.split_whitespace().collect();
        let Some(type_at) = words.iter().position(|w| matches!(*w, "Disk" | "Pipe" | "Printer" | "Device")) else {
            continue;
        };
        if words[type_at] != "Disk" || type_at == 0 {
            continue;
        }
        // The name is everything before the type column, as written.
        let name_end = trimmed.find(&format!(" {}", words[type_at])).unwrap_or(trimmed.len());
        let name = trimmed[..name_end].trim().to_string();
        if !name.ends_with('$') {
            shares.push(name);
        }
    }
    shares
}

/// Folders macOS keeps from apps without Full Disk Access (or a specific
/// privacy permission). A read failure there is a privacy refusal, not a
/// plain permission problem.
pub fn is_privacy_protected(path: &str, home: &str) -> bool {
    let home = home.trim_end_matches('/');
    let guarded = [
        "Library/Mail",
        "Library/Messages",
        "Library/Safari",
        "Library/Cookies",
        "Library/HomeKit",
        "Library/Suggestions",
        "Library/Metadata/CoreSpotlight",
        "Library/Application Support/com.apple.TCC",
        "Library/Application Support/AddressBook",
        "Library/Application Support/CallHistoryDB",
        "Library/Calendars",
        "Library/Reminders",
        "Library/IdentityServices",
        "Library/Containers/com.apple.mail",
        "Library/Group Containers/group.com.apple.notes",
        "Desktop",
        "Documents",
        "Downloads",
        "Pictures/Photos Library.photoslibrary",
        "Library/Mobile Documents",
    ];
    let within = |root: &str| path == root || path.starts_with(&format!("{root}/"));
    guarded.iter().any(|g| within(&format!("{home}/{g}")))
        || ["/Library/Application Support/com.apple.TCC", "/private/var/db/dslocal"].iter().any(|r| within(r))
}

/// The probe used to tell whether the app has Full Disk Access: a folder
/// that only such apps can list.
pub fn full_disk_access_probe(home: &str) -> String {
    format!("{}/Library/Safari", home.trim_end_matches('/'))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pt13_several_volumes_ask_unless_a_modifier_decides() {
        assert_eq!(eject_plan(2, false, false), EjectPlan::Ask);
        assert_eq!(eject_plan(2, true, false), EjectPlan::All);
        assert_eq!(eject_plan(3, false, true), EjectPlan::One);
        assert_eq!(eject_plan(1, false, false), EjectPlan::All);
        assert_eq!(eject_plan(1, false, true), EjectPlan::All);
    }

    #[test]
    fn sm02_share_list_hides_admin_shares_and_non_disks() {
        let output = "\
Share                                           Type    Comments
-------------------------------
Media                                           Disk
Backup Drive                                    Disk    Nightly
IPC$                                            Pipe    IPC Service
ADMIN$                                          Disk    Remote Admin
HP LaserJet                                     Printer

5 shares listed
";
        assert_eq!(smb_shares(output), vec!["Media".to_string(), "Backup Drive".to_string()]);
        assert!(smb_shares("").is_empty());
        assert!(smb_shares("smbutil: server rejected the connection: Authentication error").is_empty());
    }

    #[test]
    fn privacy_protected_folders_are_recognised() {
        let home = "/Users/me";
        assert!(is_privacy_protected("/Users/me/Library/Mail", home));
        assert!(is_privacy_protected("/Users/me/Library/Mail/V10", home));
        assert!(is_privacy_protected("/Users/me/Desktop", home));
        assert!(!is_privacy_protected("/Users/me/Library/Mailbox", home));
        assert!(!is_privacy_protected("/Users/me/Projects", home));
        assert!(!is_privacy_protected("/Users/other/Library/Mail", home));
        assert_eq!(full_disk_access_probe("/Users/me/"), "/Users/me/Library/Safari");
    }
}
