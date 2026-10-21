# Proposal

## Why

The only way to attach a file in Tern is the paperclip button and a file dialog. In Apple
Mail (and every Linux mail client people come from) you drag files from the file manager
straight into the message. This is milestone 3's "drag-and-drop attachments".

## What Changes

- Dropping one or more files anywhere on a New Message window attaches them, including on
  the message body (which otherwise takes dropped text).
- While files are dragged over the window, it shows a dashed accent outline and a
  "Drop to attach" hint.
- Folders aren't attached (a notice says so); unreadable files show the error per file.
- Dropped links that aren't local files (e.g. a link dragged from a browser) are inserted
  into the body as text at the cursor instead of being downloaded and attached.
- Files are read in the background, so dropping a large file doesn't freeze the window.
  The paperclip picker uses the same path.
- Attachment chips show each file's size, and a notice warns once the attachments add up to
  more than 25 MB, which many servers (Gmail included) reject.

Out of scope: pasting files with Ctrl+V, dropping files on the main window or the app icon
to start a new message, inline images in the body.

## Capabilities

### New Capabilities

(none)

### Modified Capabilities
- `compose`: adds attaching files by drag and drop.

## Impact

- `tern` (UI) only: `compose.rs` (drop target, async file loading shared with the picker,
  chip sizes, size notice), `format.rs` (pure helpers with tests), `style.css` (drop
  highlight).
