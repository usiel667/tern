# UI notes

Look-and-feel ideas, requests and decisions. Use the sections below; move items to
**Done** with a date when they ship.

Where things live: `crates/tern/src/` (`sidebar.rs`, `message_list.rs`, `viewer.rs`,
`compose.rs`, `accounts.rs`), styles in `crates/tern/data/style.css`, icons in
`crates/tern/data/icons`.

## To do

- **Vim key bindings (Settings toggle)** — owner wants to move around the whole app with Vim
  keys once the core features are done: j/k through messages, h/l between sidebar / list /
  message, gg/G, / for search, and so on. Must never fire while typing in a text field.
- **Themes (Settings)** — pick a theme in Preferences, not just follow the system. The usual
  suspects: Tokyo Night (Night/Storm/Moon/Day), Catppuccin (Mocha/Macchiato/Frappé/Latte),
  Rosé Pine (Main/Moon/Dawn), plus probably Gruvbox, Nord, Dracula, Everforest, Kanagawa.
  Each maps its palette onto libadwaita's named colors (window, view, headerbar, sidebar,
  accent, destructive, …) in one CSS file per theme, so new themes are easy to add; light
  variants (Day, Latte, Dawn) switch Adwaita to light. The message viewer's own CSS and the
  compose body follow the theme too.

## Ideas / maybe

-

## Decisions

- **2026-10-15 — Window opacity:** Tern runs fully opaque on Hyprland for now because
  GTK4 windows can't be blurred there yet (see `DEBUG.md`). Revisit when the upstream
  bug is fixed.

## Done

- **2026-10-21 — Undo Send:** Send closes the window and shows "Sending “Subject”…" with
  **Undo** for 10 seconds (Apple Mail's default), then "Message sent". Undo reopens the same
  window with "Not sent — saved to Drafts". Long subjects are cut at 40 characters.
- **2026-10-21 — Drafts:** autosave (no indicator, like Apple Mail); Ctrl+S shows "Saved to
  Drafts". Closing asks "Save this message as a draft?" with Save (default) / Cancel /
  Don't Save (red). Messages in a Drafts mailbox show a "This message is a draft." banner
  with **Edit Draft**; Return or double-click in the list also opens them.
- **2026-10-21 — Size limits and Unsent Messages:** attaching warns against the account's
  real limit; Send shows a "Message Too Large" dialog (size vs. what the server accepts) and
  keeps the window open. A refused send shows a toast with **Review**; Main Menu → Unsent
  Messages… lists refused messages (subject, To, reason) with Edit / Try Again / Delete icon
  buttons. Ctrl+Return now sends from the message body too.
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
