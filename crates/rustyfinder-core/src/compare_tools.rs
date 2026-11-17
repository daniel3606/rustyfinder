//! External programs that show two files side by side (sync-compare §7).
//! Each is found where its installer puts it; nothing is looked up on
//! `$PATH`, where a same-named program (ImageMagick's `compare`) could
//! stand in for the wrong thing.

/// A compare tool the user can pick in Settings.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CompareTool {
    /// Apple's FileMerge, through `opendiff`. Needs Xcode.
    FileMerge,
    /// Kaleidoscope's `ksdiff`.
    Kaleidoscope,
    /// Beyond Compare's `bcomp`.
    BeyondCompare,
    /// Araxis Merge's `compare`.
    Araxis,
}

pub const ALL: [CompareTool; 4] =
    [CompareTool::FileMerge, CompareTool::Kaleidoscope, CompareTool::BeyondCompare, CompareTool::Araxis];

impl CompareTool {
    /// The key saved in the settings.
    pub fn key(self) -> &'static str {
        match self {
            CompareTool::FileMerge => "filemerge",
            CompareTool::Kaleidoscope => "kaleidoscope",
            CompareTool::BeyondCompare => "beyondcompare",
            CompareTool::Araxis => "araxis",
        }
    }

    /// The tool for a saved key; anything unknown is FileMerge.
    pub fn from_key(key: &str) -> CompareTool {
        ALL.into_iter().find(|t| t.key() == key).unwrap_or(CompareTool::FileMerge)
    }

    pub fn name(self) -> &'static str {
        match self {
            CompareTool::FileMerge => "FileMerge",
            CompareTool::Kaleidoscope => "Kaleidoscope",
            CompareTool::BeyondCompare => "Beyond Compare",
            CompareTool::Araxis => "Araxis Merge",
        }
    }

    /// What to install when the tool is missing.
    pub fn install_hint(self) -> &'static str {
        match self {
            CompareTool::FileMerge => "FileMerge comes with Xcode. Install Xcode from the App Store, then try again.",
            CompareTool::Kaleidoscope => {
                "Install Kaleidoscope and its command-line tool “ksdiff” (Kaleidoscope ▸ Integration), then try again."
            }
            CompareTool::BeyondCompare => {
                "Install Beyond Compare and its command-line tools (Beyond Compare ▸ Install Command Line Tools), then try again."
            }
            CompareTool::Araxis => "Install Araxis Merge in the Applications folder, then try again.",
        }
    }

    /// Where the tool's command-line program can be, most likely first.
    fn candidates(self, home: &str) -> Vec<String> {
        let mut list: Vec<String> = Vec::new();
        let apps = ["/Applications".to_string(), format!("{home}/Applications")];
        match self {
            CompareTool::FileMerge => list.push("/usr/bin/opendiff".into()),
            CompareTool::Kaleidoscope => {
                list.extend(["/usr/local/bin/ksdiff".into(), "/opt/homebrew/bin/ksdiff".into()]);
                for dir in &apps {
                    list.push(format!("{dir}/Kaleidoscope.app/Contents/Resources/bin/ksdiff"));
                    list.push(format!("{dir}/Kaleidoscope.app/Contents/MacOS/ksdiff"));
                }
            }
            CompareTool::BeyondCompare => {
                list.extend(["/usr/local/bin/bcomp".into(), "/opt/homebrew/bin/bcomp".into()]);
                for dir in &apps {
                    list.push(format!("{dir}/Beyond Compare.app/Contents/MacOS/bcomp"));
                }
            }
            CompareTool::Araxis => {
                // Only Araxis's own copy: "compare" elsewhere is usually ImageMagick.
                for dir in &apps {
                    list.push(format!("{dir}/Araxis Merge.app/Contents/Utilities/compare"));
                }
            }
        }
        list
    }

    /// The program to run, if the tool is installed. `exists` says
    /// whether an executable file is at a path; `filemerge_present` whether
    /// Xcode's FileMerge is (opendiff is only a stub without it).
    pub fn locate(self, home: &str, exists: &dyn Fn(&str) -> bool, filemerge_present: bool) -> Option<String> {
        if self == CompareTool::FileMerge && !filemerge_present {
            return None;
        }
        self.candidates(home).into_iter().find(|p| exists(p))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn each_tool_is_found_where_its_installer_puts_it() {
        let present = ["/opt/homebrew/bin/ksdiff", "/Applications/Beyond Compare.app/Contents/MacOS/bcomp"];
        let exists = |p: &str| present.contains(&p);
        assert_eq!(CompareTool::Kaleidoscope.locate("/Users/me", &exists, false).as_deref(), Some(present[0]));
        assert_eq!(CompareTool::BeyondCompare.locate("/Users/me", &exists, false).as_deref(), Some(present[1]));
        assert_eq!(CompareTool::Araxis.locate("/Users/me", &exists, false), None);
        // opendiff alone is not enough: FileMerge must be there too.
        let stub = |p: &str| p == "/usr/bin/opendiff";
        assert_eq!(CompareTool::FileMerge.locate("/Users/me", &stub, false), None);
        assert_eq!(CompareTool::FileMerge.locate("/Users/me", &stub, true).as_deref(), Some("/usr/bin/opendiff"));
    }

    #[test]
    fn araxis_never_picks_up_another_compare() {
        let exists = |p: &str| p == "/usr/local/bin/compare" || p == "/opt/homebrew/bin/compare";
        assert_eq!(CompareTool::Araxis.locate("/Users/me", &exists, false), None);
        let user_app = |p: &str| p == "/Users/me/Applications/Araxis Merge.app/Contents/Utilities/compare";
        assert!(CompareTool::Araxis.locate("/Users/me", &user_app, false).is_some());
    }

    #[test]
    fn keys_round_trip_and_unknown_is_filemerge() {
        for tool in ALL {
            assert_eq!(CompareTool::from_key(tool.key()), tool);
            assert!(!tool.install_hint().is_empty());
        }
        assert_eq!(CompareTool::from_key(""), CompareTool::FileMerge);
        assert_eq!(CompareTool::from_key("nonsense"), CompareTool::FileMerge);
    }
}
