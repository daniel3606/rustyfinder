import AppKit

/// Quick Open sections that depend on the selection: Open With (one or
/// several files), Multi-Rename presets (two or more items) and Assign
/// Tag (any local selection).
extension MainWindowController {
    func quickOpenSelectionRows(later: (@escaping () -> Void) -> () -> Void) -> [PaletteRow] {
        guard let list = activeList, !list.isRemote else { return [] }
        let paths = list.selectedPaths
        guard !paths.isEmpty else { return [] }
        var rows: [PaletteRow] = []

        // Open With: apps that can open every selected file.
        let files = list.inspectedSelection.filter { !$0.row.isDir }.map(\.path)
        if files.count == paths.count {
            var apps: [URL]?
            for path in files.prefix(20) {
                let these = NSWorkspace.shared.urlsForApplications(toOpen: URL(fileURLWithPath: path))
                apps = apps.map { current in current.filter { these.contains($0) } } ?? these
            }
            for app in (apps ?? []).prefix(12) {
                rows.append(PaletteRow(
                    title: "Open With › \(FileManager.default.displayName(atPath: app.path))",
                    detail: files.count == 1 ? "" : "\(files.count) files",
                    icon: NSWorkspace.shared.icon(forFile: app.path),
                    activate: later { ExternalApps.open(files.map { URL(fileURLWithPath: $0) }, withAppAt: app.path) }))
            }
        }

        // Multi-Rename presets: rename the selection with a saved preset.
        if paths.count >= 2 {
            for group in AppEnvironment.shared.core.favoriteGroups() {
                for preset in group.items where preset.kind == .renamePreset {
                    rows.append(PaletteRow(
                        title: "Multi-Rename › \(preset.name)", detail: "Preset",
                        icon: FavoriteIcons.image(for: preset),
                        activate: later { [weak self] in self?.openFavorite(preset) }))
                }
            }
        }

        // Assign Tag: toggles the tag on the selection.
        for tag in TagStore.known {
            let icon = NSImage(systemSymbolName: "tag.fill", accessibilityDescription: nil)
            rows.append(PaletteRow(
                title: "Assign Tag › \(tag)", detail: "Tag", icon: icon,
                activate: later { [weak list] in list?.toggleTag(tag) }))
        }
        return rows
    }
}
