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

- **2026-10-27 — Translucency like Obsidian:** the 10-15 decision below is superseded. GTK 4.24
  asks the compositor for *no* blur (ext-background-effect) unless CSS has
  `backdrop-filter`; with `window.background { backdrop-filter: blur(20px); }` Hyprland blurs
  Tern like any other window. Owner wants HyDE's own 90% focused / 75% unfocused (what
  Obsidian gets), not a more solid Tern: tried 97%/93% and reverted. The `tern_opaque` rule
  in `~/.config/hypr/hyprland.lua` was removed 2026-10-28. Still to do: measure scrolling
  with blur.
  Colors shift with the wallpaper behind the window (Tokyo_Blossom is dark blue in places,
  orange/yellow street lights in others); that's expected, Obsidian does the same.
- **2026-10-27 — Tokyo Night theme, matching the owner's Obsidian:**
  Obsidian uses Tokyo Night *Night* (bg `#1a1b26`, sidebar `#16161e`, highlight `#292e42`,
  text `#c0caf5`, muted `#a9b1d6`, faint `#565f89`, purple `#bb9af7`, red `#ff757f`, amber
  ("yellow") `#e0af68`, blue `#7aa2f7`). Picked in a live side-by-side session (variants
  ① Night, ② purple accent, ③ colorful, ④ model):
  - backgrounds: ①'s two shades — darker mailbox sidebar, list and message view the same
  - selected mailbox: text + icon purple
  - timestamps: amber
  - sender names: purple; **selected** email: sender red, subject purple
  - thread-count bubble ("2"): purple
  - selection and hover: just a rounded bubble (hover fainter), no box-shaped outline
  - open email header: sender purple and the larger line (1.15em bold), subject smaller (1em)
  - open email: subject and plain-text body in Obsidian's text color #c0caf5 (body colors live in format.rs)
  - still from ①: light-blue unread dots and accent
  - account headers (drag to reorder): purple hover bubble, stronger on the drop target,
    solid purple with a white name while pressed or dragged; no GTK drop outline
  The stylesheet is `crates/tern/data/tokyo-night.css` (shipped 2026-10-28). The owner's GTK theme
  (`~/.config/gtk-4.0/gtk.css`, a 7,091-line Tokyo Night) hard-codes widget colors at USER
  priority, so Tern's theme must load above it (`STYLE_PROVIDER_PRIORITY_USER + 1`) or set
  the same widgets. Becomes the Tokyo Night option in the planned Themes setting (PLAN.md).
- ~~**2026-10-15 — Window opacity:** Tern runs fully opaque on Hyprland for now because
  GTK4 windows can't be blurred there yet (see `DEBUG.md`). Revisit when the upstream
  bug is fixed.~~ Superseded 2026-10-27 (above).

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
- 2026-10-28: sidebar account sections: a chevron collapses each account's folders, account
  names are 1.6em, and dragging an account header onto another account (or between its rows)
  reorders them; the order is saved in accounts.toml (`Command::ReorderAccounts`).
- 2026-10-28: the theme model was approved and built in. It lives in `crates/tern/data/tokyo-night.css` as a bundled resource, loaded at USER+1 priority only while the app is dark, so light mode keeps Adwaita. Dark plain-text bodies use `#c0caf5`. HTML mail still renders on white (undecided).
