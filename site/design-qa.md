# Website design QA

Date: 2026-11-04
Result: passed for local visual preview. Not deployed.

## Source and comparison

- Selected option 1: `design/reference.png`, 1003 × 1568.
- Final desktop: `design/desktop-after.png`, viewport 1003 × 1568.
- Final mobile: `design/mobile-after.png`, viewport 390 × 844.
- Additional views: `design/desktop-wide.png`, `design/download-desktop.png`, `design/download-mobile.png`.
- Source and rendered desktop screenshots were displayed together at matching width for direct comparison. Reviewed the full page, then hero typography, terminal details, command rows and sync illustration within those images.

## Corrections

1. P2: Initial headings and download button were too small; section rules were about 15 px too low. Increased heading size and button width, reduced margins. Final desktop page height matches the reference at 1568 px.
2. P2: Sync illustration was too small. Increased image scale and reduced text spacing to match the source composition.
3. P2: Enlarged transparent image bounds produced 4 px horizontal overflow. Clipped the illustration container; measured scroll width now equals viewport width at 390 and 1003 px.
4. P2: Hover-only command copying was unsuitable for phones. Mobile rows now expose copy buttons alongside their descriptions.

## Visual results

- Layout, hierarchy and section order match: header, two-column hero, commands, Drive sync, footer.
- Ivory background, pine green accents, dark terminal, thin rules and rounded controls retained.
- Heavy Chinese headings, monospace commands, button proportions and page rhythm checked against the source.
- All intended copy retained, including manual Drive pull and local-only SSH private keys.
- Raster terminal and sync assets are illustrations; minor glyph/texture differences from the source remain cosmetic. They are not live app screenshots.
- 390 px phone screenshots show stacked content, visible copy controls and usable macOS/Linux dialogs.
- 320 px and 1440 px widths were also checked; no horizontal page overflow observed.
- No outstanding P0, P1 or P2 visual issues.

## Functional checks and limits

- Opened downloads from both hero and navigation; switched macOS, Windows and Linux tabs.
- Keyboard ArrowRight changes Windows to Linux; Escape closes the dialog and restores focus to the opener.
- Confirmed rendered Mac/Windows links point to their existing Actions artifacts and clearly state unsigned trial / GitHub login requirements.
- Linux and command copy buttons return successful “已复制” feedback. Browser clipboard inspection returned empty, so external paste contents were not independently verified.
- Console warning/error query returned no entries.
- `npm run build` succeeded. These checks cover the website preview, not installation of the desktop tools.
- GitHub Pages publication and public 0.5.0 Release publication are not part of this local preview delivery.

## 2026-11-04: guided interface iteration

Final result: passed. User-approved changes supersede the original second-section command list.

- Added the installed Phosphor GitHub icon before the source link text.
- Replaced subcommands with one `sshm` entry and a three-step add/select/connect demonstration. Persistent a/e/arrows/Enter hints remain visible; examples are explicitly labeled as illustrative. Actual a/e/Enter mappings were checked in src/ui/app.rs.
- Compared prior desktop and updated desktop screenshots together at 1003 px width. Hero/sync/footer visual direction retained; the second section is intentionally taller for the accepted demonstration.
- Verified all three step controls, selected-row state, pause/play controls, and unchanged public source-link destination. Reduced-motion preference and offscreen animation pausing are handled in the component.
- Checked 390 px phone screenshot and 320 px page width; no horizontal overflow. Fixed an offscreen skip-link appearing in full-page captures while preserving keyboard focus visibility.
- Evidence: design/guided-desktop.png and design/guided-mobile.png. Browser warning/error query was empty. npm run build succeeded.
- No software logic, external services, commits or deployment changed in this iteration. Concurrent Rust modifications belong to the separate software-theme chat.

## 2026-11-04: download dialog simplification

Final result: passed. Removed all red-box text from the three user-supplied screenshots (platform/version descriptions and explanatory notes). Installer cards use Phosphor DownloadSimple; Linux copying, installation documentation and all-version links retain their existing semantics and destinations. Inspected all three tabs in the browser and verified the card icons visually. Build passed. Evidence: design/download-clean-macos.png. Prior notes about trial labels describe earlier iterations; the user explicitly requested their removal here.

## Approved hero wording

Replaced the hero with the exact user-approved terminal-interactive SSH copy and updated the document title. Reduced headline size to balance two lines within the existing column. Inspected desktop (1003 px) and phone (390 px) screenshots; no title clipping or horizontal overflow. Build passed. Evidence: design/hero-final-copy.png. Final result: passed.
