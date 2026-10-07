# AngelBot UI

Import controls from `src/ui` (or the corresponding relative path). This entry also loads the shared control stylesheet. The sole theme owner is `src/ui/tokens.css`; `system.css` owns typography, focus and shared surfaces. Load both once at the application entry. Page styles own layout, not control colors.

```tsx
import { Button, TextField, Switch } from '../../ui';

<TextField label="名称" hint="用于识别这个任务" value={name} onChange={(event) => setName(event.target.value)} />
<Switch label="完成时通知" checked={notify} onChange={(event) => setNotify(event.target.checked)} />
<Button variant="primary" busy={saving} onClick={save}>保存</Button>
```

- `Button` preserves native attributes and refs. Its default `type` is `button`, default variant is `secondary`, and `busy` prevents repeat activation without replacing the label. Variants: `primary`, `secondary`, `ghost`, `danger`; sizes: `normal`, `small`.
- `IconButton` requires `label`, uses it as the accessible name and default tooltip, and treats its icon as decorative.
- `Input` and `Select` preserve native attributes, event handlers and refs. Use them when a page already provides a label; add `invalid` for a validation state.
- `TextField` adds a connected label, hint and error. `multiline` selects a native textarea; `rows` defaults to `3`. `SelectField` accepts native children or an `options` array. Both retain caller-provided `id` and `aria-describedby`.
- `Switch` is a native checkbox: use `checked`/`defaultChecked`, native `onChange`, and a required label. It supports keyboard Space, form submission and disabled state without a custom event model.
- `Dialog` is controlled with `open`, `title`, `onClose`, and children. It portals to `document.body`, traps focus, handles Escape/backdrop clicks and restores the trigger. `initialFocusSelector` is scoped to the dialog and should identify an enabled control. Use `role="alertdialog"` for a blocking confirmation; `ConfirmDialog` already chooses safe cancel as its initial focus.

Keep the existing one-modal-at-a-time convention. This layer deliberately does not introduce modal stacking or a new dependency. Settings' old field imports remain value-callback adapters to this one implementation. Legacy `.btn` and button variant/size classes are compatibility selectors in `primitives.css`; use the exported controls for new work.

## Visual language

Warm off-white canvas, amber actions and quiet neutral surfaces; dark mode uses charcoal rather than tinted glass. Windows local fonts only, no emoji or external font requests. Use the 4/8 spacing scale, 400/500/600 weights and caption/body/reading/title sizes. Control/panel/dialog radii are 6/10/12 px; do not add wildcard class-name reskins.

Use proximity, heading hierarchy and small surface differences to group content. `.ui-surface` and notices have no decorative outline; ordinary buttons use a quiet fill instead of an outlined box. Do not give every section, navigation item or list row a divider. Keep visible input boundaries, error cues, selected-state markers and keyboard focus; tables/tree connectors and genuinely separate embedded content can retain necessary lines. Dialogs use the overlay and one restrained elevation treatment, not nested outlined cards. Examples and settings labels should describe their function directly, without promotional slogans.

`--ui-accent` is a **fill**, paired with `--ui-accent-ink`. Links use `--ui-accent-text` and `--ui-accent-text-hover`. Legacy `--color-accent` remains text-only; filled legacy controls use `--color-accent-fill` / `--color-accent-contrast` / `--color-accent-fill-hover`. Semantic danger/success/warning/info colors remain distinct from branding. `.ui-surface`, `.ui-notice`, `.ui-tag`, `.ui-stack` and `.ui-inline` are small layout/state utilities, not a second component layer.

Theme state lives in `stores/theme.ts`: `light`, `dark`, `system`; `initializeTheme()` handles live system changes and returns listener cleanup. Transitions use short token durations and honor reduced motion. Feedback must also be visible without animation; do not invent execution percentages.

## Interactive reference and verification

Run `npm run dev`, then open **http://127.0.0.1:5173/ui.html**. The standalone gallery uses the actual controls and theme state but has no backend, model, file or credential operations. Its examples are explicitly illustrative. It is a development entry, not an additional app route or production settings page.

Use `python scripts/verify.py quick` while editing and `full` before handoff. Library tests cover native behavior, field descriptions, focus and dialog portals; token tests check light/dark text contrast. These tests do not replace visual checks at desktop/narrow widths or real desktop IPC acceptance.

### Integration evidence · 2026-10-01

- `quick` and offline `full` passed: frontend 314 passed / 6 skipped; Rust 934 passed / 1 opt-in ignored. Production build, release preflight and diff checks passed. No live model credentials or user runtime data were used by the full run.
- Browser visual checks covered the gallery in light/dark, its confirmation portal and safe initial focus, plus the actual conversation/settings surfaces and the 900 px narrow shell. The browser-only app preview intentionally has no desktop IPC backend; this is not a fresh real-model or native desktop-operation acceptance run.
- Shared foundations and settings controls are integrated; the existing right workbench remains. The gallery is a reference composition, not a claim that every application page has been individually redesigned. Removed historical token sets, generic button implementations and wildcard radius reskins rather than adding another theme overlay.

### Low-border refinement · later on 2026-10-01

- Removed decorative outlines and repeated dividers from the shared surfaces, navigation, chat suggestions, settings groups and component gallery; meaningful input/error/focus boundaries remain. Gallery labels now describe functions directly. Windows single-line native selects are aligned with text inputs without changing multi-select sizing.
- Final canonical `frontend` passed: 327 tests passed / 6 skipped and production build passed. Browser checks covered light/dark gallery, actual chat/API-settings surfaces and the visible 2 px keyboard focus indicator. The desktop IPC backend is not attached to this browser-only preview.
- This round's `full` did not pass: the computer-operation test `commands::desktop::tests::changing_executable_cannot_carry_fill_authority` failed at `src-tauri/src/commands/desktop.rs:548` (expected error wording). That Rust module is outside this UI change; its work-in-progress edits were preserved, not weakened or reverted. Rust compilation passed on the subsequent check. The earlier successful full run above is historical evidence, not a claim that the current whole worktree passes.
