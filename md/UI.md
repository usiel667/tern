# UI notes

Look-and-feel ideas, requests and decisions. Use the sections below; move items to
**Done** with a date when they ship.

Where things live: `crates/tern/src/` (`sidebar.rs`, `message_list.rs`, `viewer.rs`,
`compose.rs`, `accounts.rs`), styles in `crates/tern/data/style.css`, icons in
`crates/tern/data/icons`.

## To do

-

## Ideas / maybe

-

## Decisions

- **2026-10-15 — Window opacity:** Tern runs fully opaque on Hyprland for now because
  GTK4 windows can't be blurred there yet (see `DEBUG.md`). Revisit when the upstream
  bug is fixed.

## Done

- **2026-10-20 — Folders attach as zips:** dropping a folder shows a "Compressing “Name”…"
  chip with a spinner in the attachment strip, then a normal `Name.zip` chip. Send is
  refused with a notice while anything is still compressing.
- **2026-10-20 — Drag-and-drop attachments:** dropping files anywhere on a New Message
  attaches them; while dragging, the window gets a dashed accent outline and a "Drop to
  attach" pill near the bottom. Chips show the size; a toast warns past 25 MB.
  Shift+Ctrl+A opens Attach Files (Apple's ⇧⌘A).
- **2026-10-20 — Recipient suggestions:** To/Cc/Bcc show a list under the field (name, then
  email in small dim text) for the address being typed; first row pre-selected, Up/Down,
  Return/Tab or click to pick, Escape closes just the list. Popover is left-aligned to the
  field. Hidden when the window loses focus.
- **2026-10-16 — Message list paging and subtitle:** the list loads 500 rows at a time and
  grows as you scroll; reaching the end of the cache downloads older mail (also grows the
  page so the batch shows at once). The subtitle shows the real cached total and unread count
  for a mailbox or unified view, not the rows paged in (it used to read "500 messages" with
  550 cached). Search, the unread filter and Flagged/Unread still count visible rows.
