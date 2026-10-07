# Desktop E2E

Run `npm run test:desktop` to build the real Tauri desktop binary and validate non-blocking first use, Main-Agent reminder creation and recall, workspace continuity, automation handoff, workbench, and trusted-application status flow.

The runner uses a temporary application-data directory and enables the `desktop-e2e` Cargo feature. That feature registers the embedded WebDriver only in the test binary; the normal application build has neither the feature nor the test server. While the test is running, credential reads return empty values, so it never uses a developer's system keychain data.

Child environments exclude credential-like names and force offline Cargo/npm
resolution. Model and Explorer responses are scripted, not live service evidence.
The reminder receipt check suppresses OS notifications and manually triggers the
saved definition; focus refresh verifies its durable receipt in an existing chat.
Actual due-time claiming/cancellation is covered by the offline backend tests.
The expanded multi-reminder check preserves at least 120 CSS pixels of reading
space and an unclipped composer at the tested small-window size. Screenshots are
generated under `src-tauri/target/e2e`, including `compact-reminder-recall.png`.

The same journey first rejects an unsupported image with a visible error and
rejects an unsupported request before persistence. Four long attachment names
must preserve at least 120 CSS pixels of reading space and an unclipped composer
at the tested small-window size (`daily-file-draft.png`). It then selects a bounded
CSV `File` through the real Composer reader, sends an attachment-only Personal
turn, follows up, switches spaces,
and edits/resends the original turn. The scripted provider checks the file name
and unique content marker in its input and rejects a Personal surface containing
`read_file` or `write_file`; persisted messages must retain the unchanged snapshot.
This proves transport/context continuity, not a live model's understanding of a
document. The fixture contains no user file or private data. Its sent snapshot is
`daily-file-snapshot.png`.

This suite deliberately stays separate from `python scripts/verify.py full`: the latter is the fast deterministic unit/integration gate, while desktop E2E is a narrow Windows smoke test. Add a desktop scenario only for a regression that cannot be covered at a lower layer.

For the actual Windows UI Automation observation/preflight/confirmed text-write
path, run `python scripts/native_uia_smoke.py`. It uses an owned temporary WPF
window and the production desktop adapter, without model keys or user app data.
See [the native fixture guide](../scripts/fixtures/README.md) for prerequisites,
safety assertions, cleanup, and the limits of this backend-only smoke.
