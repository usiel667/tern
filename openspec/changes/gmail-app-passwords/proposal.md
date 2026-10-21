# Proposal

## Why

Adding a Google account only works through browser sign-in (OAuth), which needs a Google
OAuth client of your own (`TERN_GOOGLE_CLIENT_ID`) until Tern's client is verified by Google
(see the 2026-10-21 notes: about six weeks, before the public release). Gmail also accepts
**app passwords** over IMAP/SMTP for accounts with 2-Step Verification, the same way iCloud
uses app-specific passwords. Supporting them makes Gmail work for anyone today with no
Google approval.

## What Changes

- The Google form offers **App Password** as its sign-in method. When a Google OAuth client
  is configured, a "Sign-In Method" choice also offers **Browser**; otherwise only app
  passwords are shown, with no mention of developer setup.
- App-password accounts are stored with `auth = "password"` and the Gmail preset servers;
  everything else (sync, sending, Sent handling, size limits) is unchanged.
- Google shows app passwords as four groups of four letters; spaces are removed before
  saving, so pasting it as shown works.
- If Google rejects the password, the error explains that an app password is needed (not the
  normal Google password) and where to create one.
- README: Gmail setup describes app passwords first; OAuth stays as the developer option.

Out of scope: Google OAuth verification, GNOME Online Accounts, switching an existing
account's sign-in method.

## Capabilities

### New Capabilities
- `accounts`: how accounts are added and signed in to.

### Modified Capabilities

(none)

## Impact

- `tern-core`: `AccountConfig::clean_secret` (pure, tested).
- `tern-auth`: `google::configured()`.
- `tern` (UI): `accounts.rs` Google form, sign-in error hint.
- README.
