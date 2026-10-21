# Proposal

## Why

Dropping a folder on a New Message only showed "folders can't be attached". Email has no
folders, but Apple Mail zips a dropped folder and attaches the archive, and the owner asked
for the same.

## What Changes

- A dropped folder is zipped in the background and attached as `<Folder name>.zip`
  (`application/zip`), keeping its subfolders.
- Hidden entries (`.git`, `.DS_Store`, …) and symlinks are left out of the archive; symlinks
  could otherwise pull in files from outside the folder.
- Already-compressed files (images, video, audio, archives, PDFs, Office documents) are
  stored rather than deflated, which is as small and much faster.
- While a folder is being compressed, a "Compressing “Name”…" chip with a spinner shows in
  the attachment area, and sending is refused with a notice until it's done.
- Folders whose files add up to more than 200 MB aren't zipped (the archive is built in
  memory and no mail server would take it); a notice says why.
- The 25 MB warning applies to the zipped size.

Out of scope: choosing folders in the Attach Files dialog (GTK's file dialog can't pick
files and folders at once; drag and drop covers it), including hidden files on request.

## Capabilities

### New Capabilities

(none)

### Modified Capabilities
- `compose`: dropped folders are zipped and attached instead of refused.

## Impact

- New dependency `zip` 9 (default features off; only `deflate-flate2-zlib-rs` and
  `chrono`), used only by the `tern` crate.
- `tern` (UI): new `folder_zip.rs` (no GTK, unit-tested), `compose.rs` zips directories on
  a blocking-task thread (`gio::spawn_blocking`), shows busy chips and guards `do_send`.
