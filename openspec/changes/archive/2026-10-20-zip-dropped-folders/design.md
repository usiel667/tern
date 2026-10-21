# Design

## Decisions

- **Zip in memory on a worker thread.** `gio::spawn_blocking` keeps the window responsive;
  attachments are `Vec<u8>` anyway (`OutgoingAttachment.data`), so writing to a temp file
  would only add cleanup. The 200 MB cap bounds memory; it is checked while walking, before
  any compression.
- **Skip hidden entries and symlinks** (using `symlink_metadata`, never following links), so
  a folder can't leak `~/.ssh` through a link or ship a `.git` history by accident.
- **Stored vs deflated by extension.** Deflating JPEGs or MP4s costs time for ~0% gain.
- **Keep Unix permissions and modification times** in the archive (times converted to
  local DOS time via zip's `chrono` feature; out-of-range times are omitted).
- **Busy state in the attachment strip, not a toast**, because toasts queue and expire;
  `do_send` refuses while anything is compressing so a zip can't be silently dropped.

## Risks / Trade-offs

- Debug builds compress slowly (zlib-rs unoptimized): ~10 s for 120 MB of random data.
  Release builds are far faster; the spinner covers either.
