# Tasks

- [x] 1.1 `AccountConfig::clean_secret` (Gmail app passwords lose all whitespace, others are trimmed) and `google::configured()`; verify unit tests
- [x] 1.2 Google form: method choice (only when configured), app password row, help text, button label; Gmail password error hint; verify in the test display with a fresh profile, without signing in (no keyring writes)
- [x] 1.3 README Gmail section; `md/PLAN.md`; tests, clippy

## Notes

- Checked in the test display (fresh profile, private D-Bus, no keyring writes): picker subtitle, app-password form, method choice with a placeholder client id, Browser hiding the password. Not exercised: signing in to a real Google account.
