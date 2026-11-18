# dmgbuild settings for the RustyFinder installer window.
#
# Called by scripts/package.sh with -D app=<path to RustyFinder.app> and
# -D background=<path to background.png>. Positions are icon centres in
# points and must match the background drawn by make-background.swift.
import os.path

app = defines["app"]  # noqa: F821 (dmgbuild provides `defines`)
background = defines["background"]  # noqa: F821
app_name = os.path.basename(app)

format = "ULFO"  # LZFSE: smaller than zlib, opens on every macOS we support
filesystem = "APFS"
files = [app]
symlinks = {"Applications": "/Applications"}

icon_locations = {
    app_name: (165, 195),
    "Applications": (495, 195),
}

# The height adds Finder's title bar so the whole 660x400 background shows.
window_rect = ((200, 120), (660, 432))
default_view = "icon-view"
show_status_bar = False
show_tab_view = False
show_toolbar = False
show_pathbar = False
show_sidebar = False
show_icon_preview = False
icon_size = 128
# Finder's labels are black on the black background: kept small so they fit
# in the gap above the visible labels painted into the background.
label_pos = "bottom"
text_size = 10

# The volume shows the app's own icon in the Finder sidebar and on the desktop.
_icns = os.path.join(app, "Contents", "Resources", "AppIcon.icns")
if os.path.exists(_icns):
    icon = _icns
