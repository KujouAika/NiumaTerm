# Application UI geometry and themes

Keep navigation, conversations, settings, and source review on the same visual
grid. Shared measurements live in `crates/app/src/design.rs`; paired regions
consume the same measurement rather than maintaining separate numeric values.

| Role | Logical pixels |
| --- | --- |
| Spacing steps | 4, 8, 12, 16, 24 |
| Button and input radius | 6 |
| Tab, selection, and preview radius | 8 |
| Composer and dialog radius | 12 |
| Horizontal tab height | 32 |
| Settings tab and navigation width | 240 |
| Both source-review footers | 32 |

Round exposed corners. An attached tab has rounded upper corners and a flat
lower edge; adjoining navigation and content panes have straight shared edges.
Use one divider at each join. Theme changes alter colors while retaining the
application's geometry. Tiny preview glyphs and full-circle indicators scale to
their own sizes.

Tab presses belong to the tab, including movement before a reorder starts.
Only unused title-bar space can initiate a window move. An insertion marker
must not change tab bounds, and a dragged preview retains the source dimensions.

Source review uses independent addition and deletion fills, with a stronger
fill for changed words. Light row fills are `#E1FCE1` and `#FEE4E3`; dark row
fills are `#243D2D` and `#482C30`. Keep line numbers and change signs visible so
color is not the only distinction.

The theme gallery shows one card per explicit light/dark pair, using a grid of
up to four columns. Each 120-pixel card contains a small chrome/code preview.
The mode switch applies the available counterpart and all previews use that
mode where supported. A single-mode theme stays selectable and identifies its
available mode; its mode switch is disabled.

Theme files can opt into pairing with the same `family` string and opposite
`mode` values. Existing variant file identifiers remain the saved selection, so
older configuration files and copied built-ins keep working. Duplicate modes
remain separate entries instead of hiding user files. A theme's failed reload
retains the active colors; directory changes update the gallery automatically.

Claude colors are adapted from the supplied Typora theme. Attribution is in
`assets/licenses/claude-typora-theme.txt`. Application font preferences remain
independent of palette selection.
